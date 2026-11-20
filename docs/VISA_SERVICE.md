# The ZPR Visa Service

The Visa Service examines a description of attempted communication and decides,
from policy and trusted-source attributes, whether to issue a **visa**. Granted
visas are distributed to every node along the communication path.

It is the primary security control of a ZPRnet. Two properties dominate its
design:

- **Correctness** — visa granting *is* the enforcement decision, so the policy
  application logic must be flawless.
- **Performance** — it sits in the path of every new flow and is the obvious
  bottleneck.

Read this before changing issuance, revocation, attribute handling, or
evaluation. Changes to issuance behavior should also be read against
[ZPL.md](ZPL.md) (what policy can express) and ZRFC 12.

## Sources

| Source | Status |
|---|---|
| *ZPR Visa Service Specification / Design Document* (2025-09-03) | The design intent. Explicitly work-in-progress, and parts are marked sketchy or TODO by its authors. |
| `zl-zpr-visaservice/` | What actually runs. Workspace version 0.22.0 on `feature/zodes`. |
| `zl-zpr-visaservice/admin-http-api.txt` | The admin API, current as of 2026-09-15 and changing frequently. |
| `zl-zpr-visaservice/libeval/README.md` | The evaluator's API and staging model. |

The design document predates the current implementation and has drifted; §
"Where the implementation diverges from the design document" records the gaps
worth knowing. When the two disagree, **the code is what runs**.

---

## Responsibilities

Beyond issuing visas, in the design document's priority order: an
administrative interface for installing policy and removing visas or
components; routing and addressing for the ZPRnet; tracking active visas and
connected components; removing them when policy or conditions change;
maintaining state across node reboots and disconnections; testing active visas
against a proposed policy; redundancy for failover; a logging interface; and
signaling for policy-triggered events, kept distinct from logging.

Explicitly **excluded** from this implementation, though present in the ZPR
RFCs: Byzantine-style visa-granting agreements, running multiple policies in
parallel, and federated visa services for load balancing.

---

## Repository layout

| Crate | What it is |
|---|---|
| `vs` | The visa service itself, "v2vs" — the v2 distinguishes it from the prototype. Also builds `vsapikey` for generating admin API keys. |
| `libeval` | The evaluator. Compares described traffic to policy and returns the decision. Used by `vs` and `zpt`. |
| `zpt` | ZPR Policy Tester — a REPL and batch runner over `libeval`, for testing how a policy evaluates without standing up a ZPRnet. |
| `vs-admin` | CLI client for the HTTPS admin API. |
| `zpr-dashboard` | Terminal dashboard for monitoring visa service activity. Go, not Rust — a Bubble Tea TUI over the admin API. |
| `admin-api-types` | Request/response types shared by `vs` and `vs-admin`. |
| `integration-test` | Shell-driven integration tests, including `libeval` evaluation tests through `zpt`. |
| `tools` | Helper scripts, including `zpr-pki` for PKI operations. |

Most of it depends on `zl-zpr-common` for the NODE–VS API structures and the
binary policy format, pulled by Git tag in `Cargo.toml` — no manual setup.
Requires Rust edition 2024, `make`, OpenSSL, and a running Redis/Valkey.

### Inside `vs`

Long-lived **managers** own state; **workers** run the per-request and
background work.

| Component | Responsibility |
|---|---|
| `policy_mgr` | The one true source of the current policy. Updates arrive asynchronously from administrators and ripple outward: visas may stop being valid, actors may be forced to disconnect, node connections may change. |
| `actor_mgr` | Actors, and nodes too. |
| `visa_mgr` | Creation, storage, and retrieval of visas. |
| `topology_mgr` | The graph of nodes and links *as it exists* (policy describes how it is *intended* to be), with pathfinding and route selection. Write-through to state. |
| `router` | The in-memory node graph and route cache. Knows nothing of adapters — resolve actors to their docking nodes first. |
| `net_mgr` | Addressing. |
| `connection_control` | New connections to the ZPRnet, node and adapter: authenticate, then authorize against policy. |
| `login_nonce` | Visa-service-minted OIDC login nonces for a Zode's own login (`getLoginNonce`): bound to actor and VS-API session, single-use, short-lived. In memory. |
| `event_mgr` | High-level events with system-wide consequences, such as actor joins and leaves. Fire-and-forget: it cannot report success back to a caller. |
| `visa_reconciler` | Re-evaluates live visas against a policy snapshot and queues revocations for those that no longer hold. |
| `visareq_worker` | "The beating heart" — one worker per visa request. |
| `vsapi_worker` / `vss_worker` | The node-facing APIs; one VSS worker per node. |
| `db_worker` | Renews the shared database lock that keeps exactly one visa service instance active, and terminates the process if it is lost. |
| `signal_worker` | Emits policy-triggered signals. |
| `deny_log` | A bounded in-memory window of recent denials (500 entries), collapsed on the 5-tuple so a chatty source cannot flush it. Not persisted. |
| `trusted_services/` | Attribute sources and the mapping from their names onto ZPL attribute names. File-backed (`api = "file"`), OIDC (`api = "oidc"`), and networked `api = "zpr-attr/1"` stores — the latter query an external attribute service over HTTPS (see ATTRIBUTE_SERVICE.md). |
| `db/` | State: actors, visas, nodes, links, policy. Valkey/Redis backed, with a fake for tests. |

---

## Lifecycle

### Bootstrap

A ZPRnet needs a visa service and the relay Zode it runs on before any other
Zode can authenticate. Those come up on static configuration, called
**bootstrap** — which is also enough to run an entirely statically
authenticated ZPRnet.

Bootstrap configuration maps Common Names from ZPR link keys to RSA public key
files; that list is exactly the set of actors permitted to authenticate
statically. The visa service reserves the link key name **`vs.zpr`**, and its
own actor needs no entry: see the next section.

The protocol: a Zode connects over VS-API, signs the visa service's challenge
with its RSA private key (`--auth-private-key`), and the visa service verifies
the signature against the configured public key. The visa service still
implements the adapter form too, where a node forwarded a docked adapter's CN
and signature, but since zl-zipline#234 no adapter docks on a Zode.

### The visa service on the relay, and trusted services

The visa service runs on the relay Zode, with no adapter (zl-zipline#229). It
runs beside the relay's `ph zode` on the relay's TUN, where `vs.zpr`
(`fd5a:5052::1`) is a second address: the relay lists it in `--zpr-addr`
after its own. The visa service authenticates itself at startup, and every
Zode, the relay first, authenticates to it over VS-API as above. A relay or
Zode accepts only Zode links, so nothing docks on a Zode. See "The relay
hosts the visa service, with no adapter" under *Design decisions*, and
ZODE_LINKS.md.
ZIPLINE runs it this way ([org-zpr/zipline#252](https://github.com/org-zpr/zipline/issues/252)): each network's
`zipline-relay-<id>` container holds the relay Zode, valkey and the
visa service, and its policy has no `vs.zpr` bootstrap key or rule. The visa
service needs neither: it authenticates itself by construction
(`authenticate_visa_service`), and `approve_connection` approves an actor
that matches no join policy.

A trusted service can only join once the visa service is up — Zodes cannot
authenticate connections by themselves. Each time a new
trusted service connects, the visa service refreshes attributes for every
connected actor.

### VS-API calls added for Zodes

`vs.capnp` on `feature/zodes` adds three `VSHandle` calls a node makes over its
existing session, and the visa service implements all three:
`reportLinkStates` ([#217](https://github.com/org-zpr/zl-zipline/issues/217),
[#224](https://github.com/org-zpr/zl-zipline/issues/224)), `getLoginNonce`
([#232](https://github.com/org-zpr/zl-zipline/issues/232)) and
`requestBootstrapVisas` ([#252](https://github.com/org-zpr/zl-zipline/issues/252)).
A fourth, `updateSubstrateAddr`, was removed in #252 without ever being
implemented: a node's substrate address is fixed for the life of its VS-API
session, and a new one arrives with a rejoin's `connect` or `open`. The members
after it were renumbered as a flag day (`zl-v0.32.0-zodes.4`).

| Call | Contract | Errors | Implemented by |
|---|---|---|---|
| `reportLinkStates @8 (links :List(LinkStateReport)) -> (res :OkOrError)` | Snapshot of the node's link states, replacing its previous report; the router uses a link only while both ends list each other `up`, and an end that has not reported counts as down ([ZODE_LINKS.md](ZODE_LINKS.md) rule 14; [#242](https://github.com/org-zpr/zl-zipline/issues/242), done by [#250](https://github.com/org-zpr/zl-zipline/issues/250)). A link going down revokes the visas crossing it. A **new** `failed` entry (one the node's previous snapshot did not list as `failed`) marks the pair, and the visa service leaves a marked pair out of both ends' `setTopology` until either end (re)connects its VS-API session, which clears the pair and re-sends topology to the other end (rule 13a). A `failed` report never itself sends topology. See [ROUTING.md](ROUTING.md), "Link-state reports" | `authRequired` on a dead handle; `paramError` for more entries than the node's policy-declared links, an empty `peerCn`, or a `peerCn` listed twice. A visa whose revocation fails is quarantined and retried by the visa service; the report still succeeds. An entry whose CN matches no declared peer is kept, not an error, and cannot make a link usable under the current policy | implemented, [#224](https://github.com/org-zpr/zl-zipline/issues/224), [#250](https://github.com/org-zpr/zl-zipline/issues/250); node side [#250](https://github.com/org-zpr/zl-zipline/issues/250) |
| `getLoginNonce @9 () -> (res :Result(LoginNonce))` | A single-use, expiring nonce for the node's own OIDC login; `LoginNonce { nonce :Text, expiresAt :UInt64 }` (unix seconds). The binding to the node's actor and session lives in the visa service, not in the schema | `authRequired` on a dead handle; `quotaExceeded` when the node holds the maximum number of outstanding nonces | [#232](https://github.com/org-zpr/zl-zipline/issues/232) |
| `requestBootstrapVisas @10 (peerCn :Text, observedAddr :SockAddr) -> (res :Result(BootstrapVisas))` | The relay asks for the bootstrap visas of a Zode joining over an initial link to it, naming the joiner by the CN on its verified link certificate ([ZODE_LINKS.md](ZODE_LINKS.md) J2, J3, J6), and reports `observedAddr`, the source address it saw on that link. A request that passes its checks records it as the joiner's observed substrate address before the previous join is ended and the visas minted, so an `internal` answer may come after both the record and the teardown (see "Lifecycle"; [#264](https://github.com/org-zpr/zl-zipline/issues/264)). The joiner's ZPR address is the one the relay's topology gives that CN. `BootstrapVisas { peer, relay }` holds two visas in each list, the joiner's VS-API flow in both directions, forward first: `peer` for the relay to hand on in ZDP message 146, `relay` for it to install. Every call is a join: see "Bootstrap visas, on demand" below | Checked in this order, so a caller that may not ask learns nothing about the CN: `authRequired` on a dead handle; `invalidOperation` when the caller is not the node the visa service docks on, whatever `peerCn` it sent; `notFound` when `peerCn` is not valid UTF-8 or names no peer in the caller's topology (an unknown CN, the caller's own, an empty one, or a pair left out under rule 13a); `paramError` when `observedAddr` is unparseable or undialable (nothing is recorded or minted); `internal` when the mint fails. `vs.capnp` lists the same errors in the same order | [#252](https://github.com/org-zpr/zl-zipline/issues/252) |

How the nonce is checked on `reauthorize`, and the exception to the exact-set
rule it serves, are under *A Zode's own login and logout* below.

### A Zode's own login and logout

A Zode's machine actor is the node's own actor (RSA only). A user login adds
the `user` namespace to it through `reauthorize`; a logout removes the
login-derived namespace (device-keyed re-enrichment may restore an equivalent
one, below). This is
the one exception to the exact-set rule, and only for a node renewing its own
actor (the `reauthorize` target is the calling node's address):

- **Gain (login):** the device blob plus an OIDC blob. Right after the
  namespace check, before the device proof, the visa service spends the blob's
  nonce in its login-nonce store (`vs/src/login_nonce.rs`): it must have been issued by
  `getLoginNonce` to this actor in its current VS-API session (`zpr.session`)
  and be unexpired. A nonce the store rejects (not issued, another actor or
  session, expired, already spent) is a generic `authError`. Then the token
  is validated exactly as on the connect path; a token whose `nonce` claim
  differs from the blob's nonce fails there as `invalidSignature`. If the
  policy generation moved before the login is persisted, it is refused with
  `temporarilyUnavailable` and the node logs in again with a new nonce (see
  *One commit-time recheck* below).
- **Drop (logout):** the device blob only — on any actor that holds a user
  authority, including one whose OIDC provider policy no longer declares (a
  trusted-service-derived authority is re-derived). The renewed actor keeps only
  attributes the visa service stamped itself (never one in `user.`); the policy
  re-run re-fetches trusted-service attributes from the device identity,
  which may legitimately include `user.*` attributes a store vends for that
  device. The login's OIDC session anchors are forgotten. A valid logout from
  the node's current session is not refused because of an enrichment failure
  or a policy change, since that would leave the login in place. (A malformed
  request is still a `paramError`, and a logout from a stale handle -- an
  earlier VS-API session -- is still an `authError`.) If the policy re-run
  fails (a trusted service is down) or the policy generation moved before the
  commit, the visa service persists the *reduced* actor instead: the loaded
  actor with only internal-source attributes and nothing in `user.`, still a
  node, with the services a node gets under the current policy, and a freshly
  minted identity token. It then invalidates the actor's trusted-service
  revisions (each cleared and its generation bumped), so the next request
  re-fetches every source and is denied (`AttributesIndeterminate`) while one
  is still down. Dropped data is never treated as known-absent, so a removed
  `device.blocked` cannot lift a deny.
- **Device blob always.** An OIDC blob without it, or a namespace change on any
  actor other than the caller's own, is a `paramError`.
- **Visas follow.** After every login gain or drop (a namespace change) the
  visa service re-checks every live visa against the resulting actor (the attribute-change sweep). A visa
  is revoked when it no longer passes policy: after a logout that is one that
  needed the login's attributes, unless device-keyed re-enrichment restored
  an equivalent attribute. The same sweep runs after a node authentication
  that replaces a stored actor holding attributes the new one lacks (a
  session replacement drops the login), and after a renewal that keeps the
  login but lost an attribute value, such as a claim missing from the renewed
  token (zl-zipline#265; see "Design decisions").
- **One commit-time recheck.** Every writer of a node's own actor commits
  under the node commit lock (`PolicyMgr::node_commit_lock`) and calls one
  shared helper, `recheck_before_commit`, just before the write. It checks
  that the stored `zpr.session` is the one expected (else `authError`) and
  reports whether the policy generation moved since the write was prepared.
  Two kinds of writer differ in what the lock covers. `reauthorize` of the
  node's own actor, `open`, `getLoginNonce` and `disconnect` hold it from
  the read their write is based on until the write, so their preparation is
  serialized too. The node `authenticate` persist and the attribute refresh
  prepare *outside* the lock (authentication, the network fetch of trusted
  attributes) and take it only to commit; there, the recheck -- plus, for a
  refresh, the identity token -- rejects a preparation that went stale, so
  their reads and fetches are not serialized with a login or logout, but a
  stale result is never written:

  | Writer | Session checked | On a moved generation |
  |---|---|---|
  | `reauthorize` login or renewal of the node's own actor | the calling handle's, at load and at commit | refused, `temporarilyUnavailable` |
  | `reauthorize` logout (and its outage fallback) | the calling handle's, at load and at commit | the reduced actor is persisted (above) |
  | node `authenticate` persist (a `Reset` connect's teardown of the old node state runs only after the recheck, in the same locked commit) | — (it writes the new session) | refused, `temporarilyUnavailable`; the node authenticates again, its old state untouched |
  | `open` (session resume) | load and write both under the lock | — (no re-authorization) |
  | `getLoginNonce` mint | the handle's | — |
  | attribute refresh of a node | the copy's (and its identity token) | discarded, `AttributesIndeterminate` (denied, retried) |
  | `disconnect` | the actor record is removed under the lock | — |
  | policy install | — | publication itself takes the lock |

  So two concurrent logins cannot both classify as a gain; a stale handle
  cannot act on, or mint for, a newer session; a reconnect, refresh or
  disconnect cannot land between a self-renewal's check and its write; and
  nothing prepared under a superseded policy is committed. Because policy
  publication takes the same lock, an install lands either before a commit's
  recheck (which then sees the new generation) or after its write (which the
  install's reconciliation and re-authentication obligation then cover). The
  identity token is minted after the recheck has chosen the actor to persist.
  If a login's or logout's actor write fails part-way (the attributes may
  already be committed), the visa service rolls a login back to the pre-login
  actor (disconnecting the node if even that fails, so no login without
  session anchors stays active), invalidates a reduced logout's revisions,
  releases the lock, and queues an attribute-change reconcile before it
  reports the error.
- **Role kept.** The node role is fixed at connect: if the policy re-run no
  longer matches a node join policy (say the only one needs the user attribute
  a logout drops), the actor stays a node and gets the services a node would
  get, which may be none.
- **Current generation, no lost work.** The visa sweep after a gain or drop
  runs after the node commit lock is released (it needs none of the actor
  serialization), under the policy snapshot current when it starts. It
  repeats, up to three passes (`NAMESPACE_SWEEP_ATTEMPTS`), until one finishes
  with no failed re-check or verdict write under a generation that did not
  move; an actor-store read error during a pass counts as a failure, not as
  an absent actor. If none does, the work goes to the event manager as an
  attribute-change reconcile instead of being dropped; that reconcile queues
  itself again every 30 s (`RECONCILE_RETRY`) while its own sweep still fails,
  so a visa whose revocation failed does not stay usable until it expires.

The login-nonce store is in memory. A nonce is 32 random bytes (base64url),
usable until `expiresAt` (300 s, `LOGIN_NONCE_TTL_SECS`), at most 4 unexpired
per actor session (`LOGIN_NONCE_MAX_PER_ACTOR`; `getLoginNonce` answers
`quotaExceeded` past it). Expired entries are swept on every mint and spend,
and a mint also drops the node's entries from an earlier session. After a
restart there are none, and the node asks again.

**A node `authenticate` replaces the actor, and the login with it.** Every
node authentication -- a VS-API reconnect (`HasState`), a reset, a
visa-service restart, a rejoin, the K2 in-place re-authentication -- builds
a new, RSA-only actor and stamps a new session (`authenticate_node`), so a
user who was logged in is gone on the visa-service side. Only `open`
(`resume_node_actor_for_open`) keeps the stored actor, and ph does not use
it. ph knows this and marks its login `SessionReplaced`, prompting a
resident `ph-cli auth-agent` for a new login
([zl-zipline#233](https://github.com/org-zpr/zl-zipline/issues/233)). When
the replacement drops attributes the stored actor held, the visa service
runs the attribute-change sweep, so visas granted on the dropped user's
attributes are marked for revocation (see "A lost attribute re-checks the
live visas (#265)" under Design decisions). A policy install's
`requestAuthentication` to a logged-in Zode is answered with a `reauthorize`
renewal, which keeps the user, instead of K2.

### Loading policy

A policy is required at startup; later updates arrive over the admin API. The
policy carries a compiler version and the visa service rejects versions it does
not support. On `feature/zodes` it requires **compiler 0.21.x**: the major and
minor must equal `POLICY_MIN_COMPILER_*` (0.21.0) and the patch must be at least
its patch (`vs/src/config.rs`; BUILD.md, "Versions and tags"). 0.21 is the
compatibility boundary set together with routing's enforcement of
`Policy.transitNodes`
([#256](https://github.com/org-zpr/zl-zipline/issues/256),
[#255](https://github.com/org-zpr/zl-zipline/issues/255)), so a 0.20.x policy
is refused. A policy without that list would load with no transit-capable
node, and every multi-hop route would be denied `NoRoute`.

A policy can be **tested without loading it**: the service reports which
existing visas the proposed policy would deny. That is only possible because it
retains enough state to re-run the requests behind active visas.

A policy may also change the network topology.

### Authenticating actors

Every authentication-capable trusted service is named in policy and must be
connected for authentication to succeed; if none is reachable and the actor
cannot use bootstrap authentication, authentication fails.

The visa service passes authentication-service addresses to nodes; a Zode
uses them for its own login (zl-zipline#233). A trusted service returns a token carrying the
attributes policy asks for, some of which policy designates as **identity
attributes**. The visa service then queries other trusted services with those
identity attributes to complete the actor's profile.

**Attribute stores are queried by identity attributes only — plus one reserved
exception — and that is structural, not a policy choice.** The lookup set is
computed from the authenticated claims *before* any store is queried
(`vs/src/trusted_services/mod.rs`, `lookup_identities`), so an attribute that
exists only inside another store's cache can never be a lookup key. Keying a
`file` store on `user.email` therefore cannot work by construction — `email`
is not an identity attribute — and the store's JSON must be keyed on a
declared identity attribute such as `user.sub` (see "Design decisions",
*Attribute chaining runs on identity attributes*). The one
exception is the reserved authority marker: `lookup_identities` feeds
`user.zpr.authority` into every lookup unconditionally, whether or not any
service declares it, because identities such as an OIDC `sub` are unique only
within the issuing service and stores need the authority to scope them
per-issuer. A `file` store *can* therefore key on the authenticated authority
marker even though `zpr.`-namespaced attributes can never appear in
`identity_attributes`. This is a property worth preserving: it makes the set
of query keys a function of what was actually authenticated, never of what
some store happens to vend.

With attributes in hand it evaluates the actor against policy: may it
communicate on the ZPRnet at all, may it host services, does it get special
privileges such as a static ZPR address, and is it explicitly denied? An actor
that passes receives a ZPR address and joins. Since zipline#97/#98 a static
address is **granted, never asserted**: a peer-requested `zpr.addr` is honoured
only when a matched join policy pins it with a `zpr.addr eq` condition — and
since zipline#109 the only source of such a pin is a node's `zpr_address` in
the `.zplc` topology (an authored `["zpr.addr", ...]` provider attribute is a
compile error; adapters get static addresses via the `device.zpr_addr` grant
below) — and
`authorize_connection` then rejects any static address that is outside
`fd5a:5052::/32`, equal to the visa service address, inside a managed pool
(static addresses live in a separate address space from the pools — see
`SYSTEM_OVERVIEW.md`, "Addressing"), or already held by a live actor. An
address the actor already holds passes through unchanged on re-authorization
(session renewal, node reconnect).

Since zipline#99 a trusted service can make that placement decision too, by
vending an authenticated **`device.zpr_addr`** attribute — a `file` store keyed
on the device CN, say. Once authentication has produced the actor, the grant is
read from the accumulated authenticated claims — never from self-asserted peer
claims, which are dropped before this point, so a peer cannot steer its own
address by claiming a grant — and promoted to the actor's `zpr.addr` *before*
the pre-set-address checks run, independently of any address the peer did or
did not request. It therefore flows through exactly the arms a
static pin does: inside `fd5a:5052::/32`, not the visa service's own address,
outside the managed pools, not held by a live actor, and the
renewal/reconnect proof arms. A grant that cannot be understood is an operator
error surfaced loudly with `AuthenticationFailed` naming `device.zpr_addr` —
never a silent fall-through to the pool, which would quietly bring the device
up somewhere other than where the operator placed it. Rejected this way: a
grant resolving to more than one distinct address (multi-valued, or
conflicting grants from two trusted services — the claims are inspected before
the actor's keyed attribute insert collapses them, so a conflict stays
visible), a value that does not parse as an IP address, and a grant that
disagrees with an address already pinned by a matched join policy. Duplicate
grants that agree are redundancy, not ambiguity; a grant agreeing with a pin
is logged and the address keeps its static source. Diagnostics carry the
address source — a rejected pin reads "static", a rejected grant reads
"granted" — so the log tells the two apart
(`vs/src/connection_control.rs`, `authorize_connection`).

A node's **substrate address**, the address its peers dial, never comes from
the node ([#264](https://github.com/org-zpr/zl-zipline/issues/264)). The
relay reports the source address it saw on the node's initial link as
`observedAddr` on `requestBootstrapVisas`, and the visa service records it
when it grants the request (`Assembly::observed_substrate`, canonicalized from
IPv4-mapped IPv6; an unspecified IP or port 0 is a `paramError`). `connect`
and `open` then record the node's `zpr.substrate_addr` claim through one rule,
`substrate_addr_for` (`vs/src/connection_control.rs`): policy's address, else
the observation, else the claim already stored on the node's actor, else none
(no claim, no direct links). A differing observation under a policy address
is logged at `info`. `connect` refuses any parameter it does not know, and
`open` takes none. A rejoin's bootstrap request updates the stored claim at
once, and a node's observation is forgotten when it disconnects. The stored
claim counts as known only while the node has a live VSS worker. It decides
which links the node's peers are offered and what they dial (ZODE_LINKS.md
rule 13b; ROUTING.md, "Substrate address changes"). So the
`zpr.substrate_addr` claim, which policy can read, is now policy's or the
relay's, never the node's word.

The visa service's own address is the one neither source supplies: it is
fixed by configuration, so an actor whose *authenticated* CN is `vs.zpr` is
assigned `fd5a:5052::1` before policy is consulted, with no pin and regardless
of what it requested. The visa service's startup self-authorization writes
that record. A merely claimed `vs.zpr` gets nothing (zipline#102). The visa
service still lets an authenticated `vs.zpr` adapter replace the record, but
since zl-zipline#234 no Zode docks one.

Authentication lifetimes depend on how the actor authenticated. **Bootstrap
(RSA) authentication does not expire**: re-proving possession of a static key
on a timer demonstrates nothing new, so bootstrap `device.zpr.authority` is
stamped far-future, and the `zpr.vs.bootstrap.ident` identity attribute
follows the actor's authority expiry instead of gating on its own
(zipline#119). OIDC lifetimes come from the trusted service's tokens and are
renewed silently through the resident auth agent (`docs/OIDC.md`). The visa
service's own identity attributes are pinned ~100 years out so it can never
expire itself.

What forces re-authentication is a **policy install**, not a clock. Every
install records the new policy generation and a deadline (`now +
reauth_deadline`), then asks each connected node over the VSS-API
(`requestAuthentication`) to re-authenticate itself and its docked adapters
(a Zode has none since zl-zipline#234, and answers for its own actor) under
the new generation; the sweep re-sends to actors still owing, and once
the deadline passes it revokes whoever has not re-authenticated —
adapters through the batched `revokeAuthentication`, nodes by disconnect
(zipline#123). Authenticating under the newest generation satisfies all older
obligations, so a stream of installs cannot postpone enforcement. This is how
a bootstrap key removed from policy takes effect for a connected actor:
within one `reauth_deadline` of the install. Node culling at visa-service
startup is by last-seen age (the 4-hour default lifetime), since with
non-expiring bootstrap authentication expiry can no longer be the criterion.

### Granting a visa

Nodes request visas. Before evaluating, the visa service gathers both actors
and their attributes; a visa is denied outright if an actor is unknown, and
expired or nearly expired attributes are refreshed from trusted services
first (see *Attributes are refreshed ahead of expiry (#275)* below). An
expired authentication token is a denial.

The evaluator receives the current policy, both actors with their attributes,
and a description of the attempt: source and destination ZPR addresses, ports
(or ICMP type and code), protocol, and a request flag —
`bidirectional_request`, `unidirectional_request`, or `re_request`.

`visareq_worker` enumerates the outcomes: an actor missing or disconnected; no
route between them; attributes needing refresh; expired authentication; an
existing visa still valid under unchanged policy; denied by policy; allowed but
not over any available route, which is also a denial; or permitted — and even
then, if policy changed underneath the request, the request fails and the
caller is expected to retry.

Once permitted, the path is chosen, the affected nodes are computed, the visa
is queued for installation on each, and it is returned to the caller.

### Distributing visas

The visa returns to the requesting node, the ingress Zode, and propagates to
the nodes along the path. The ingress and egress Zodes bind it (a stream ID)
to their own actor's traffic.

Each node gets its own copy of the visa, cut to its role on the path (the table
in [ROUTING.md](ROUTING.md)). Intermediary and egress copies carry
`Visa.prevHop`, the previous node's ZPR address, so a node can refuse a
`Stream ID Request` from any other neighbour (gap 7). The ingress copy has none.
`actualize_visa_for_target_node` sets it on each copy ([zipline#245](https://github.com/org-zpr/zl-zipline/issues/245)). This
includes the bootstrap visas of a joining Zode: the relay's copy of the forward
visa names the joiner, and the joiner's copy of the reverse visa names the relay.
A visa whose path has fewer than two nodes has no `prevHop`. Core enforces it
since [zipline#253](https://github.com/org-zpr/zl-zipline/issues/253), so a
copy without `prevHop` is never bound from a Zode link.

A node that gets a `Stream ID Request` for a visa it does not hold fetches it
by ID (`visaRequestById`). The visa service returns that node's own copy,
actualized for it, of each requested visa that is still grantable to it, when
the VSS push would deliver that copy now or already has
(`VisaMgr::visas_for_node_by_ids`,
[#262](https://github.com/org-zpr/zl-zipline/issues/262)). So it never hands
out an ingress copy that ingress-last delivery still holds back, nor a copy
off the node's path, revoked or quarantined. The fetch is read-only: it never
marks a copy installed, and the later VSS push of the same copy is a no-op on
the node.

The service records active visas and the requests that created them — the state
that makes policy testing possible.

### Expiration, revocation, renewal

Every visa carries an expiration timestamp, embedded and visible to every
recipient. Bounds are administrative settings: **30 seconds minimum, 24 hours
maximum** in this build. The minimum exists because node installation plus the
round trip would otherwise consume the whole lifetime, and because the visa
store's second-granularity TTL truncates sub-second lifetimes to zero.

Nodes determine expiry locally, so **no "visa expired" message is ever sent**.
On expiry a node stops forwarding and tears down the tethers to its adapters;
further traffic looks new and triggers a fresh request. Nodes can be configured
to re-request before expiry.

*Built* ([#271](https://github.com/org-zpr/zl-zipline/issues/271),
[#274](https://github.com/org-zpr/zl-zipline/issues/274)): as designed, on
expiry every Zode on the path stops forwarding, by its own clock: each
forwarding entry carries the bound visa's expiry, and every packet is
checked against it. A new flow that matches an expired visa at its ingress
Zode ejects that visa and looks again, and a `Stream ID Request`, a bind or
a J11 rebind commit for an expired visa is refused, including a visa
fetched by ID. A sweep on the link keep-alive, at least 10 s apart, then
frees expired visas and, at the ingress, withdraws their streams from the
Zode's actor, so further traffic looks new and is matched afresh: it
triggers a fresh request unless the lookup finds another live visa.
(A joiner's check of its bootstrap hand-off, J4, is separate and older.)
Two differences remain. A hop whose stream enters over a Zode link holds
the expired visa, and its stream ID, for a 60 s grace before freeing it,
so the ID is not reused while an upstream Zode with a lagging clock may
still send on it (the skew it tolerates; a larger lag is not caught); its
packets are dropped meanwhile. And no Zode
re-requests a visa before it expires: a Zode's core visas are renewed by
the visa service (J9), and other flows ask again after the expiry. The visa
service forgets a visa when it expires, so it never revokes an expired one;
before #271 a Zode kept using expired visas for good, and before #274
streams bound before the expiry kept forwarding. See ZODE_LINKS.md, "Visa
expiry on every use (#274)".

A visa whose lifetime would be under `MIN_VISA_LIFETIME` (30 s) is denied.
To keep trusted-service attributes from causing that deny
([#275](https://github.com/org-zpr/zl-zipline/issues/275)), the visa service
re-queries a configured trusted service once one of its attributes is within
`ATTRIBUTE_REFRESH_AHEAD` (45 s) of expiry, not only once it has expired.
The deny still happens when nothing the visa service can do extends the
limit: an actor's authentication about to expire, an OIDC claim (renewed by
the client's silent re-authentication, not by the visa service), or a
`zpr-attr/1` `expires_at` that the service keeps vending unchanged. That deny
is counted and listed, with `NoReason`, by `GET /admin/visas/denies`, and is
logged at WARN with the requesting node, the 5-tuple, the time left and what
limited it: the attribute key (an OIDC claim's too), `source_actor_auth` or
`destination_actor_auth`.

Revocation is different — it is an administrative action or a push from a
trusted service. Installing a policy can revoke visas by newly disallowing a
communication, by changing trusted services in a way that invalidates prior
authentications, or by changing topology so an existing path is no longer
valid. The visa service sends revocation messages over the VSS-API listing
revoked visa IDs; nodes stop forwarding immediately and tear down the sessions.

A trusted-service change revokes visas too, with no policy install. Its sweep
refreshes the actors behind live visas and re-evaluates those visas. A refresh
that changes an actor's attributes also recomputes the services the actor
provides (`zpr.services`) from the join policies the new attributes match, the
same computation connect uses. So a service hosted by attribute
(`define ssh as a service with device.tags:'server'.`) follows that attribute:
an actor that loses it stops providing the service and its visas are revoked,
and one that gains it provides the service from its next refresh (zipline#181).

For urgent security events the admin API can revoke visas, actors, or trusted
services without installing a policy. **The effective policy is the installed
policy plus any administrative disconnects**, and those disconnects clear when
the next policy is installed.

Renewal covers visas the service can refresh without a new request — node-to-VS
and VS-to-trusted-service traffic — and visas a node asks to renew with the
re-request flag. A renewal is sent over the VSS-API as a replacement carrying
the new visa and the ID of the one it replaces. **Granted visas are
immutable**: a renewal or upgrade is always a new visa under a new ID, and a
Zode treats an ID it already holds as the same visa (ZODE_LINKS.md, "Moving a
live connection onto newer visas").

**A Zode's core visas** are the three it needs to talk to the visa service at
all: its VS-API pair (`node:0 -> vs:VSAPI` and back) and the visa service's
VSS visa to it (`vs:0 -> node:VSS`). The visa service issues them itself
([#252](https://github.com/org-zpr/zl-zipline/issues/252), ZODE_LINKS.md J5a,
J5b, J9; `vs/src/core_visas.rs`):

- **J5a.** A new VS-API pair on every `authenticate` and every `open`. If the
  first try fails (on a first join the node's link to the relay is not
  routable until both ends have reported it), it is retried every second, for
  that session only, until it succeeds, the session is replaced, or the
  bootstrap lifetime (5 minutes) has passed.
- **J5b.** A new VSS visa on every `registerVss`, even when an older one is
  still installed. The answer to `registerVss` carries only the node's copy of
  it.
- **J9.** On every VSS heartbeat (800 ms), for each core five-tuple, the
  newest visa still grantable to the node is checked, and once it has passed
  2/3 of its lifetime a new one is minted. The check runs without the
  issuance lock and takes it only when a renewal is due. A failed mint or push
  is retried on the next heartbeat. A node with no VS-API session gets none.
  The new visa is pushed as an ordinary install. Nothing names the visa it
  replaces: the Zode matches the two by five-tuple (J11).

All three mint through the ordinary visa-request pipeline, evaluated by
policy, never built by hand. Each is requested as if by the ingress Zode of
its path and **delivered ingress last**: every other Zode on the path gets its
copy by VSS push first, and the ingress copy is pushed only once all of them
have acked (`VisaMgr::deliverable_visas_for_node`). One copy cannot wait for a
push: the registering Zode's own copy of its new J5b VSS visa rides in the
`registerVss` answer, because the VSS connection needs it before any push can
arrive. The ingress then
supersedes the older visa and rebinds the live stream (ZODE_LINKS.md J11),
and every other hop already holds the new visa. **Issuing or renewing a core
visa never revokes the one it supersedes**: superseded visas expire on their
own. Otherwise core visas are revoked like any visa (a policy that denies
them, a link going down under them), and all of a node's core visas are
revoked when its join ends, by a rejoin's teardown or a disconnect.
**Core visas carry no session keys** (an empty `KeySet`), because the
bootstrap visas they supersede cannot carry any and J11 refuses a rebind
across a key change (see "Design decisions").

---

## Evaluation (`libeval`)

Given source and destination actors plus packet details, `libeval` compares
them to a compiled policy. A policy holds **communication rules** and **join
policies** — the latter govern whether an actor may connect and what attributes
and services it receives on joining. The services are recomputed whenever an
attribute refresh changes the actor's attributes (`Policy::join_services`).

### Order of decision

1. **All deny statements first.** Any match returns every matching statement in
   policy order with its signals. The visa service takes the first and denies.
2. **Then allow statements.** Every match is returned in policy order, each
   carrying its path constraints, communication properties (addresses, ports,
   reverse-pinhole flag), signals, and the expiration a resulting visa should
   use.
3. The visa service takes the first allow match that the topology manager also
   permits. If none is routable, the visa is denied.

Matching is required to be **deterministic**: the same policy, actors, and
packet must always select the same policies in the same order.

### Service-centric matching

Actors are clients or servers, and a server must have a service registered
through policy. If neither actor hosts a service there is no match. For TCP and
UDP, a destination port mapping to a service on the destination actor is
**forward** communication, and a source port mapping to a service on the source
actor is **reverse**. For ICMP the TYPE field selects the candidate services.

Supported today: TCP with SYN/ACK flag awareness, UDP, and ICMPv6 (source port
carries the type, destination port the code).

### Two-stage API

Evaluation is split so route-aware decisions are possible:

| Stage 1 — `eval_request` returns | Meaning |
|---|---|
| `Deny(FinalDeny::Deny(hits))` | Denied by a matching deny policy, regardless of route. |
| `Deny(FinalDeny::NoMatch(msg))` | No policy matched at all. |
| `AllowWithoutRoute(hits)` | Allowed regardless of route. |
| `NeedsRoute(RouteResidualEvaluator)` | A route is needed to decide. |

Stage 2, `RouteResidualEvaluator::eval_route`, takes a `Route` and returns the
final result. **It is a scaffold and not yet implemented** — `over` clauses
(ZPL link constraints) therefore do not yet reach a route-aware verdict inside
`libeval`, though the visa service does check routability through the topology
manager.

---

## Running and operating

```bash
vs /path/to/policy.bin2        # vs.toml in the working directory is picked up
vs -c my-config.toml -v
```

Needs a running Valkey/Redis (`redis://127.0.0.1:6379` by default) and a
compiled `.bin2` policy. TLS credentials for the admin API default to
`admin-tls-cert.pem` / `admin-tls-key.pem`.

### Configuration (`vs.toml`, `[core]`)

`vs_addr`, `vsapi_port`, `admin_port`, `admin_cert`, `admin_key`, `vk_uri`,
`identity` (ties this instance to its state in the database), `api_keys`,
`file_ts_dir` (where `<service-id>.json` files for `api = "file"` trusted
services live), and `ts_secrets_dir` (where `<service-id>.token` bearer-token
files for `api = "zpr-attr/1"` trusted services live). Unknown keys are
rejected.

`vs_addr` (default `fd5a:5052::1`, see below) is the visa service's own ZPR
address, and it is used in both directions. The VS-API and admin listeners
bind it, and every outbound VSS connection to a node binds it as its source
address before connecting. VSS traffic therefore always leaves from `vs.zpr`,
even when the interface also carries another address. That is the case on a
relay Zode that hosts the visa service: one TUN carries the relay's address
and `vs.zpr`, and the relay grants the visa service's visas only for
`vs.zpr`. A `vs_addr` that is not a local address makes the connect fail; it
never falls back to a source the kernel picks.

**`vs_addr` must be local before `vs` starts.** If it is not, the
`v0.22.0-zodes.9` release logs an error, ends the VS-API and admin listener
tasks and keeps running without them (`vsapi_worker::launch`); since
visaservice#70 (1037ac7,
[zl-zipline#266](https://github.com/org-zpr/zl-zipline/issues/266)) on its
`feature/zodes`, `vs` exits with status 1 instead. On a relay Zode that hosts the visa
service, start order is therefore: the TUN with both addresses, then `ph zode`,
then valkey and `vs` (ZODE_LINKS.md, "The relay hosts the visa service
(#229)").

An OIDC JWKS fetch that goes through the policy's `jwks_proxy_service` binds
`vs_addr` too. That `CONNECT` proxy is always an actor on the ZPRnet (see
[OIDC.md](OIDC.md)): the visa service resolves it to the providing actor's ZPR
address. So on a relay Zode the fetch leaves from `vs.zpr`, which the proxy
visa covers, not from the relay's address. A direct, unproxied JWKS fetch and
the HTTP clients for `api = "zpr-attr/1"` attribute stores are **not** bound:
they reach substrate URLs, and binding a ZPR IPv6 address would break them.
([zl-zipline#240](https://github.com/org-zpr/zl-zipline/issues/240))

### Constants that must stay in sync with the compiler

Changing either side alone will break a ZPRnet, because the compiler writes
policy that assumes these values:

| Value | Default |
|---|---|
| Visa service ZPR address | `fd5a:5052::1` |
| Visa service link-key CN | `vs.zpr` |
| VS-API port (nodes → VS) | 5002 |
| Admin HTTPS port | 8182 |
| Minimum policy compiler version | 0.21.0 (minor must match) |

Other operational bounds: 4-hour default authentication lifetime (the
fallback where no authority sets one — bootstrap authentication itself does
not expire, see *Authenticating actors*), a 300-second default
`reauth_deadline` for re-authentication after a policy install, 180-second
maximum clock skew during node authentication, 20 visas per request, 1024
request workers and queue depth, a 7-second VSS ping with 3 failures before the
VSS worker exits, and a 30-second database lock refresh against a 90-second
timeout. A VSS worker that exits removes nothing: link state decides when a
departed Zode is removed, 10 seconds (`ZODE_DEPARTURE_GRACE`) after it is left
with no link up (*A departed Zode is removed after a grace (#284)* below).

### Admin API

HTTPS on port 8182, JSON in and out, TLS required. Every request carries an
`X-API-Key` header; a missing or unknown key is 401. Keys carry read,
read/write, or notify permission; write endpoints reject a read-only key with
403, and a notify key can call only `POST /admin/services/{id}/changed` for
its own trusted-service id (it can neither read nor change anything else).
Generate keys with `vsapikey`.

Endpoints cover actors (`/admin/actors`, plus their visas), services and their
caches (including `POST /admin/services/{id}/changed`, the push-invalidation
callback an attribute service uses to trigger a reconcile pass), visas
(including `/admin/visas/denies`), the network view, statistics,
policies (`GET`/`POST /admin/policies`, `/admin/policies/curr`), and
authentication revocation. Actor endpoints are keyed by **ZPR address**, not
CN: `GET /admin/actors` returns `{zpr_addr, cn}` entries where `cn` is a
display label that may be null (an OIDC-only connect has no CN), and the
per-actor paths take the address (`/admin/actors/{zpr_addr}`; a malformed
address is a 400). Some — `DELETE` visas and `DELETE` actors — still return
placeholder data; `admin-http-api.txt` marks
each one and is the endpoint reference. `DELETE /admin/actors/{zpr_addr}`,
when implemented, kicks the actor's live session; credential revocation is
`/admin/authrevoke`.

Note that unless you are on the same host, reaching the admin API requires
**policy permission** like any other service on the ZPRnet.

### Building and testing

```bash
make            # build the Rust workspace and the Go dashboard
make test       # unit tests
make check      # fmt --check plus warnings as errors
make release    # tarball of vs, vs-admin, zpt into build-release/
```

The root `Makefile` drives both toolchains (`build-rs`, `build-go`), so a
Rust-only change is faster from inside the crate.

Use `zpt` to test policy evaluation without a ZPRnet: load a compiled policy,
set attributes on named actors, and evaluate. It runs as a REPL or over an
instruction file, with `-j` for JSONL output.

---

## Design decisions

Rationale carried over from the completed master plans, which were retired once
shipped. Full plan text is in git history:

- `git show a35b224:docs/plans/2026-09-14-trusted-service-interplay.md` —
  umbrella [zipline#22](https://github.com/org-zpr/zl-zipline/issues/22)
- `git show a35b224:docs/plans/2026-09-24-static-address-grants.md` — umbrella
  [zipline#95](https://github.com/org-zpr/zl-zipline/issues/95)
- `git show b817c70:docs/plans/2026-09-25-retire-authored-address-pins.md` —
  umbrella [zipline#106](https://github.com/org-zpr/zl-zipline/issues/106)
- `git show 43d56bd:docs/plans/2026-09-28-reauth-on-policy-install-plan.md` —
  umbrella [zipline#118](https://github.com/org-zpr/zl-zipline/issues/118)
- `git show 0686fd8:docs/plans/2026-11-11-zodes.md` in org-zpr/zipline —
  umbrella [zl-zipline#216](https://github.com/org-zpr/zl-zipline/issues/216)

### Trusted-service interplay (an `oidc` authenticator plus a `file` overlay)

**The compiler never prunes a trusted service that vends identity attributes.**
Such a service is a query key for every attribute store, so whether ZPL
references *its own* attributes says nothing about whether the visa service
needs it; reference-based pruning had left the OIDC provider out of the policy
entirely. Naming it in policy does not help either, because both authority keys
resolve to the default trusted service. The accepted cost: a genuinely unused
identity vendor is now woven, and the compiler logs an `info` line for each one
retained this way. Attribute overlays (`file`) are still pruned when
unreferenced — unless they vend a visa-service-interpreted attribute
(`device.zpr_addr`, `device.hostname`); see the retention rule under
*Static addresses are granted, never asserted* below
([zipline#105](https://github.com/org-zpr/zl-zipline/issues/105)).
([zipline#23](https://github.com/org-zpr/zl-zipline/issues/23))

**Attribute chaining runs on identity attributes, and needed no change.** An
OIDC arm stamps `user.sub`, `sub` is a declared identity attribute, and a `file`
store keyed on `user.sub` answers on both the connect and refresh paths. Keying
on a non-identity attribute such as `user.email` is rejected on the merits
(mutable, reusable addresses would transfer a departed user's access) and is
impossible anyway, because the lookup set is fixed before any store is queried.
([zipline#22](https://github.com/org-zpr/zl-zipline/issues/22))

**The authenticating service keeps `user.zpr.authority`.** A decorating store
vending any `user.*` attribute used to derive a competing authority, and the
last writer won in store order, which follows compiler emission order, not ids.
The displaced authority made the OIDC store's `vouched_here` gate answer empty,
pruning `user.sub` and with it the overlay's attributes mid-session. Join
matching failed too: `JPolicy::matches` checks every value of a repeated key, so
an `authority:google` join policy was vetoed and the connection was refused as
`policyDenied`. Since the fix, `derive_user_authority` takes the existing
authority and yields `None` if it names a *different* source. The `!=` is
load-bearing: the authority's own source must still re-stamp its expiry on each
refresh. Side effect: two `file` stores with no IdP present now resolve
first-wins instead of last-wins, which is still arbitrary.
([zipline#24](https://github.com/org-zpr/zl-zipline/issues/24)–[#26](https://github.com/org-zpr/zl-zipline/issues/26))

### Static addresses are granted, never asserted

**The peer's requested `zpr.addr` is a check, not a grant.** Previously any
matched join policy committed it, so any service provider could self-assign any
address, overwriting a live actor keyed on it or colliding with the pool.
Precedence is policy pin, then trusted-service grant, then pool — and since
zipline#109 the only policy pin left is a node's `zpr_address`; for adapters
the precedence starts at the grant (see *Authored `zpr.addr` pins retired*
below). An unconfirmed
request is **scrubbed and the actor gets a pool address, rather than denied**.
That reuses the existing no-match path, and both peers already refuse a
substituted address with a clear message (the adapter's
`GrantedAddressMismatch`). A deny would look like a policy denial, which is the
worse diagnostic for what is usually a misconfiguration.
([zipline#97](https://github.com/org-zpr/zl-zipline/issues/97))

**Static addresses live outside the pools rather than being reserved in them.**
Reserving in-pool needs a take, an undo path and a release path that tells
reserved from allocated, and the adapter path had none of these. A separate
address space needs a single rejection and no bookkeeping, and nothing in the
tree pinned an in-pool address. Pool ranges stay defined only in the visa
service, so an in-pool pin is a join-time error rather than a compile-time one.
([zipline#98](https://github.com/org-zpr/zl-zipline/issues/98))

**The grant is `device.zpr_addr`: a device attribute, from any declared trusted
service.** It is a `device` attribute because nodes and headless servers have
addresses and no user, and a roaming user does not carry the adapter's address.
The name avoids the reserved `zpr.` sub-namespace, which declared services may
not return. That makes it deliverable with no compiler or schema change,
following the `device.hostname` precedent, and `zpr_` keeps it from being
confused with an operator's own `addr` field. `zpr.addr` stays the committed
address, the request and the pin. Any service may grant, not only `file`,
because the mapping is declared in the signed `.zplc`, and attributes already
decide access with ingress enforcement pinning an actor to its address
(zipline#83 added it on the dock side; since zl-zipline#234 a Zode binds only
traffic from its own addresses, `owns_address`). Choosing an address is therefore no new power; the static-range
checks apply to every source alike.
([zipline#99](https://github.com/org-zpr/zl-zipline/issues/99))

**A store vending a visa-service-interpreted attribute is retained by the
compiler.** A `file` store that vends only `device.zpr_addr` vends no identity
attributes and no ZPL-referenced attributes, so reference-based pruning used to
drop it — and the grant silently never happened (the device came up on a pool
address). The weaver now retains any trusted service whose `returns_attributes`
maps to a visa-service-interpreted attribute (`device.zpr_addr`,
`device.hostname` — `zpl::KATTR_VS_INTERPRETED` in `zl-zpr-compiler/src/zpl.rs`,
kept in sync with the visa-service consumers), the same way it retains identity
vendors, emitting an `info` diagnostic per retention. Any trusted-service API
qualifies, because any declared service may grant. The accepted cost is that an
unused networked store vending one of these attributes is woven — the same
trade-off as identity vendors. No ZPL reference is needed to keep such a store
woven.
([zipline#105](https://github.com/org-zpr/zl-zipline/issues/105))

**The visa service's own address was missed by the plan.** Nothing could supply
`fd5a:5052::1`, and the non-evicting actor add refused the adapter's re-add over
the startup record, which broke every netns test. The fix is the `vs.zpr`
exception described under "Authenticating actors".
([zipline#102](https://github.com/org-zpr/zl-zipline/issues/102))

### Authored `zpr.addr` pins retired: adapter static addresses come only from grants

Carried over from the retired plan
(`git show b817c70:docs/plans/2026-09-25-retire-authored-address-pins.md`),
umbrella [zipline#106](https://github.com/org-zpr/zl-zipline/issues/106).
An adapter's static ZPR address has exactly one authored source: a trusted
service vending `device.zpr_addr`. The `[["zpr.addr", ...]]` provider entry in
a `.zplc` is a compile error.

**Hard compile error, not a deprecation warning.** Early-release code with no
compatibility burden, and a warning trips `--Werror` anyway. The error names
`device.zpr_addr` and the `file`-store shape, so the fix is in the message.
([zipline#109](https://github.com/org-zpr/zl-zipline/issues/109))

**Nodes keep `zpr_address`, and the visa service keeps `fd5a:5052::1`.** Both
are fixed by the network's own configuration, not granted to an endpoint. Node
addresses feed the compiler's topology planning; the visa service address is a
compile-time constant on both sides ("Constants that must stay in sync").
Neither goes through `vec_to_attributes`, so the compile error does not reach
them. ([zipline#106](https://github.com/org-zpr/zl-zipline/issues/106))

**No visa-service change and no `POLICY_MIN_COMPILER_MINOR` bump.** An old
policy carrying an authored pin is still a signed policy granting an address,
so honouring it is not a hole; bumping the floor to refuse it would buy
nothing. The pin-versus-grant precedence and conflict checks in
`authorize_connection` stay, because the node path still produces pins.
([zipline#109](https://github.com/org-zpr/zl-zipline/issues/109))

**Adapters that bring up a static TUN keep `zpr_addr` in their config.** The
request is scrubbed (no pin matches), the grant supplies the same address, and
the adapter's `GrantedAddressMismatch` check passes. Request and grant must
agree; that is the operator's contract, and the adapter already reports a
mismatch loudly.
([zipline#107](https://github.com/org-zpr/zl-zipline/issues/107))

**One store per deployment, keyed on the adapter CN.** `device.zpr.adapter.cn`
is authenticated for every bootstrap adapter. Where a deployment already has a
device-keyed `file` store, extend it rather than adding a second — dns-demo's
`machines` store was the naming *and* addressing authority until its Zode port
(zl-zipline#235) pinned each Zode's address by its `[nodes]` entry instead.
([zipline#108](https://github.com/org-zpr/zl-zipline/issues/108))

**Depends on zipline#105 and does not work around its absence.** Without the
retention rule an address-only store is pruned unless a ZPL statement
references it, and every fixture would need a throwaway reference — the trap
this work was meant to end.
([zipline#105](https://github.com/org-zpr/zl-zipline/issues/105))

### Policy-install re-authentication; bootstrap authentication does not expire

Carried over from the retired plan
(`git show 43d56bd:docs/plans/2026-09-28-reauth-on-policy-install-plan.md`),
umbrella [zipline#118](https://github.com/org-zpr/zl-zipline/issues/118).

**Bootstrap authentication does not expire; the policy install is the clock.**
Re-proving possession of a static key on a timer demonstrates nothing new, so
bootstrap `device.zpr.authority` is stamped far-future and
`zpr.vs.bootstrap.ident` follows the actor's authority expiry instead of
gating on its own (fixing the latent pin where an RSA+OIDC actor was swept at
connect + 4 h however often OIDC renewed). An earlier draft renewed RSA on a
timer with a policy lifetime knob; it was dropped because timer renewal of a
static key adds nothing, while install-driven re-auth enforces what actually
changes. ([zipline#119](https://github.com/org-zpr/zl-zipline/issues/119))

**Every install asks every connected actor to re-authenticate — no diffing.**
Diffing policy to re-auth only affected actors was rejected: one uniform rule
is easy to audit; revisit only if install-time load matters.
([zipline#123](https://github.com/org-zpr/zl-zipline/issues/123))

**Enforcement is by deadline, not by reply.** The install records `(V = new
policy generation, T = now + reauth_deadline)`; once `now > T`, any actor
whose `zpr.vinst < V` is revoked — adapters through the batched
`revokeAuthentication`, nodes by disconnect. A rejection does not revoke
early: the actor stays until `T` and a retry may still succeed. With several
installs the earliest unmet deadline applies, and authenticating under the
newest generation satisfies all older ones, so a stream of installs cannot
postpone enforcement. `zpr.vinst` — stamped by `approve_connection`, which
only connect and reauthorize run — is the whole tracking state; no new
per-actor bookkeeping exists.
([zipline#123](https://github.com/org-zpr/zl-zipline/issues/123))

**Every authority is re-proved, and for an adapter the set must match
exactly.** `reauthorize` takes one blob per namespace, and for an adapter
the namespace set must equal the live actor's authorities — all-or-nothing,
so an OIDC actor whose agent cannot refresh silently is revoked at the deadline and logs in again. The SS
arm's freshness comes from the node-minted, HMAC'd challenge (at most 120 s
old) plus the VS skew check; a monotonic-timestamp rule was rejected because
only the docking node could replay, and a compromised docking node is already
SECURITY_MODEL Case 2.
([zipline#120](https://github.com/org-zpr/zl-zipline/issues/120))

**A node may change only its own `user` namespace.** The exact-set rule has
one exception, for a Zode: a node renewing its own actor may gain the `user`
namespace (login) or drop it (logout), with the device blob mandatory. Letting
a node change a docked adapter's namespaces was rejected — the node would then
decide who an adapter is. A gain on an actor whose user authority came from
trusted-service enrichment is refused: one user authority per actor.
([zl-zipline#232](https://github.com/org-zpr/zl-zipline/issues/232))

**The login nonce is minted and spent by the visa service.** On a Zode the
endpoint is its own docking node, so a nonce it mints itself would not stop a
replayed `id_token`. The nonce is bound to actor and VS-API session, single-use
(spent before its own checks and the token's, so a failed login cannot retry
it), and short-lived; the store is in memory and needs no disconnect hook. Past the per-node limit
minting is refused rather than evicting the oldest, as the vs.capnp contract
says. ([zl-zipline#232](https://github.com/org-zpr/zl-zipline/issues/232))

**One commit-time recheck, and a valid logout is not refused for an outage or
a policy change.** Each round of
review on the Zode login found another path that read a node's actor or a
policy snapshot, awaited (enrichment, persistence), and then wrote state the
read no longer justified. Rather than a check per path, every writer of a
node's own actor commits under the same lock and calls one helper just before
the write; the helper checks the session and the policy generation. A login,
renewal or node authentication under a moved generation is refused and
retried. A valid logout from the current session instead commits a reduced
actor, because refusing it would leave the user's login in place; a stale
handle's logout is still refused by the session check. Policy publication takes the same lock, so
an install can never land between a commit's recheck and its write. The
trade-off is that an install waits behind any in-flight login or logout,
including its attribute fetch from a possibly slow, networked attribute
service ([org-zpr/zl-zipline#247](https://github.com/org-zpr/zl-zipline/issues/247)).
([visaservice#63](https://github.com/org-zpr/zl-zpr-visaservice/pull/63),
[zl-zipline#232](https://github.com/org-zpr/zl-zipline/issues/232))

**Login and logout re-check every live visa.** Dropping the user namespace
must revoke visas that needed it (unless device-keyed re-enrichment restores
an equivalent attribute), and gaining it can trip a `never allow`. The
existing attribute-change sweep re-checks every live visa inline, after the
actor is persisted and the node commit lock released, and an incomplete
sweep is retried and then handed to the event manager. This costs one full sweep per login or logout; a
per-actor sweep was not built because logins are rare. A renewal that keeps
the login sweeps only when it loses an attribute value (#265, below); one
that only gains claims runs no sweep.
([zl-zipline#232](https://github.com/org-zpr/zl-zipline/issues/232))

**A node re-authenticates in place.** It re-runs
`connect(Reconnect)`/`challenge`/`authenticate` on its existing `vs_service`
capability and swaps the handle only on success — no TCP restart, and visas,
docked adapters and router links are undisturbed.
([zipline#121](https://github.com/org-zpr/zl-zipline/issues/121))

**Node culling at visa-service startup is by last-seen age.** `refresh_state`
culled nodes by authentication expiry, which with non-expiring bootstrap auth
would keep dead nodes forever; it now culls nodes not seen for the 4-hour
default lifetime, preserving the old behavior.
([zipline#119](https://github.com/org-zpr/zl-zipline/issues/119))

**No compatibility shims.** An old node answers `requestAuthentication` with
capnp "unimplemented" and an old adapter answers the renewal request without
SS; both are revoked at the deadline and reconnect fresh — acceptable under
the early-release rule, and the dependency-graph ordering kept the tree from
ever doing it to itself.
([zipline#118](https://github.com/org-zpr/zl-zipline/issues/118))

**Findings worth keeping.** `libeval`'s `NEVER_EXPIRES` carries an extra `60`
factor (~6000 years); harmless. `default`'s `cert_path` is written to policy
but never read by the visa service.
([zipline#118](https://github.com/org-zpr/zl-zipline/issues/118))

### Outbound VSS connections bind `vs_addr`

From the Zodes epic ([zl-zipline#216](https://github.com/org-zpr/zl-zipline/issues/216)),
pd plan p13, [zl-zipline#228](https://github.com/org-zpr/zl-zipline/issues/228).

**Bind the source address, not `preferred_lft 0`.** With the relay's address
and `vs.zpr` on one TUN, RFC 6724 source selection has nothing that prefers
`vs.zpr`, and a connection from the relay's address has no visa. The
alternative was to mark the relay's own address not-for-automatic-selection
(Linux `preferred_lft 0`). Binding wins because it works on any platform and
does not depend on how a separate process (`ph`) configures the TUN.
`vs_addr` is local whenever the visa service is serving: VSS connections
exist only once a node has connected over VS-API, and the VS-API listener binds
`vs_addr`. If that bind fails, the listener task ends while the process keeps
running (see "Configuration" above), and with no VS-API there are no VSS
connections to bind. The fix is on the visa service side because the node's
VSS listener does not check the peer address; only the relay's visa table
would drop the traffic.
([zl-zipline#228](https://github.com/org-zpr/zl-zipline/issues/228))

**Attribute-store clients stay unbound** (operator decision on the plan). No
attribute service is reached over the ZPRnet today, and binding a ZPR address
would break a store at a substrate URL. Bind them only when one is on-net, and
only for ZPR-address URLs.
([zl-zipline#228](https://github.com/org-zpr/zl-zipline/issues/228))

**A JWKS fetch binds `vs_addr` only when it goes through the proxy.** The
`jwks_proxy_service` proxy is on the ZPRnet by construction (the visa service
resolves it to an actor's ZPR address), so that leg has the same
source-selection problem VSS had on a relay Zode, and binding fixes it. No
address-range check is needed. A direct fetch goes to a substrate URL and stays
unbound, like the attribute-store clients. Review of the #228 docs found the
gap.
([zl-zipline#240](https://github.com/org-zpr/zl-zipline/issues/240))

### Bootstrap visas, on demand

A Zode that has not connected reaches VSAPI on bootstrap visas the visa service
mints from the policy-declared peering (`vs/src/visa_bootstrap.rs`; ROUTING.md,
"Bootstrap routing"). Since [#252](https://github.com/org-zpr/zl-zipline/issues/252)
they are minted only when the relay asks, with `requestBootstrapVisas`, as the
joiner's initial link comes up (ZODE_LINKS.md J2, J3). Topology carries none
(`Link.visas` is gone), and nothing is minted ahead of time or from a joiner's
traffic: a VSAPI packet about a peer with no actor is evaluated like any other
and denied. The rules:

- **Only the relay may ask**, the node the visa service docks on, and only for
  a peer its topology names by `peerCn`. The joiner's address is the one that
  topology gives the CN, never anything the joiner claims. A non-relay
  neighbour of an unconnected node never gets bootstrap visas, so an endpoint
  never relays a joiner's traffic.
- **Only the joiner's VS-API flow**, one visa per direction, with a 5-minute
  lifetime (`BOOTSTRAP_VISA_LIFETIME`, gate g89 q3) and no session keys, even
  if the joiner's A2A key is still stored from an earlier join: it may have
  restarted with a new one. The path is stitched from the peering,
  `[joiner, relay]`, not routed. Each visa is recorded installed on both Zodes
  at once, so ingress-last delivery never holds the relay's copy back.
- **Every request is a join** (J6) and mints new visas, with no dedup. It first
  ends the joiner's previous join (`end_previous_join`): every visa between the
  joiner and the visa service is revoked (bootstrap pairs, VS-API pairs, VSS
  visas on any port).
- **The rejoin rule.** If the joiner still has a live VS-API session, the
  relay's request is the proof that it is gone, because a peer has one link at
  a time (ZODE_LINKS.md rule 5). The visa service ends that session: a new
  session id is stamped, so the old handles fail, and its VSS worker is
  stopped. Its link-state reports are forgotten, so its links are down until it
  reports again, and the visas crossing a link that went down are revoked.
  Every other visa installed on the joiner is marked not installed, so a
  `reconnect` gets it pushed again. The node actor stays, so
  `reconnect` still works (gate g89 q5).
- **A rejoin defeats a pending departure check.** The new session that the
  rejoin stamps is what keeps a Zode that comes back within
  `ZODE_DEPARTURE_GRACE` (#284): the departure check removes only the session
  it was scheduled for. A joiner the visa service already removed has no
  actor (`has_session` is false), so its request simply mints the hand-off.
  Its `reconnect` is then answered `outOfSync` and it connects as `reset`
  (ZODE_LINKS.md J10).

The checks, the teardown and the mint run as one step under the core-visa
issuance lock and then the topology lock, so they are atomic against connect,
disconnect, core-visa issuance and policy passes. The old VSS worker is stopped
after both locks are released. `peerCn` itself comes from the peer's node join
policy (`Policy::node_cn_for_addr`). A peer whose address has no CN in policy —
no node join policy pins that address together with a `device.zpr.adapter.cn`,
or two pin it with different CNs — is left out of the topology, so a request
for it is `notFound`.
([#221](https://github.com/org-zpr/zl-zipline/issues/221),
[#252](https://github.com/org-zpr/zl-zipline/issues/252))

**A sweep rechecks a bootstrap visa against its peering, not its route**
(gate g94 q1). A policy or attribute sweep asks what the mint asked: the visa's
holder is still the relay, and the relay's topology still names the joiner by
the CN policy gives the joiner's address. It does not route the visa. A rejoin
forgets the joiner's link reports, so a routed recheck in that window would
revoke the very pair the rejoin needs. A policy that drops the peering or the
joiner's CN revokes the pair. Known limit: the visa does not record its CN, so
a policy that only renames the joiner keeps the pair for at most its 5 minutes.

**A node's core visas carry no session keys** (gate g100). Every visa between
the visa service and a node actor, in either direction, is minted with an empty
`KeySet` (`VisaMgr::session_key_for`). The bootstrap visas cannot carry keys,
because the joiner has not authenticated and so has no A2A key registered, and
a Zode refuses to rebind a live stream onto a visa whose keys differ (J11). So
the J5a, J5b and J9 visas that supersede them carry none either; otherwise a
joiner would rejoin every 5 minutes. Visas between two nodes keep their keys.
That traffic is protected by the VS-API and VSS TLS alone, and that TLS does
not authenticate either end yet (SECURITY_MODEL.md, "Design decisions";
[#259](https://github.com/org-zpr/zl-zipline/issues/259)).

### The relay hosts the visa service, with no adapter

From the Zodes epic ([zl-zipline#216](https://github.com/org-zpr/zl-zipline/issues/216)),
pd plan p41, [zl-zipline#229](https://github.com/org-zpr/zl-zipline/issues/229).

**No visa-service change.** The visa service runs beside the relay's `ph zode`
on the relay's TUN, and `vs.zpr` is a local actor of the relay (ZODE_LINKS.md,
"The relay hosts the visa service (#229)"). It authenticates itself at startup
(`self_authorize`). With no adapter, the `vs.zpr` actor's docking node is the
first node to authenticate (`hack_set_vs_docking_node`,
zpr-visaservice#195). That is always the relay: a joiner can authenticate only
over bootstrap visas the relay requested, and `is_relay` needs the docking node
to be set. The relay's held-back `ConnectRequest` for the adapter
(`deferred_vs_connect`) was a second registration of `vs.zpr`; without an
adapter there is none, and nothing in the visa service depended on it.

**Since zl-zipline#234 this is the only layout.** Every Zode is either a relay
or dials one over its initial link, and a relay or Zode refuses any first
handshake that is not a Zode handshake, so no visa-service adapter can dock
anywhere. Before #234 a node that was not a relay could still dock the visa
service's adapter (the `one-node-*` netns tests); that layout and those
tests are gone.

([zl-zipline#229](https://github.com/org-zpr/zl-zipline/issues/229))

### A policy pass never waits on a node's VSS

The policy pass runs on the single event-queue worker, so anything it waits on
delays the next policy install. Until #252 it waited for each node's
`setTopology`, `setServices` and `requestAuthentication`. A policy that removes
a joiner's link to the relay cuts the joiner off, and each call to it then
waited out the 10 s RPC timeout: the next install, even one restoring the link,
was applied 21–50 s later, or in some runs not for minutes. #252 made this
reachable, because a joiner now has a VSS connection. The invariant now (gate
g101; `vs/src/event_mgr.rs`):

- **Each node's pushes run in a detached task of their own**, in order:
  topology, then the services list, then (once the obligation is recorded) the
  re-authentication request. A slow or unreachable node delays only its own
  pushes. Overlapping passes are harmless: a `setTopology` or `setServices`
  command makes the worker re-send the current state at its next heartbeat.
- **No push to a node the pass cut off.** A node at the end of a link the pass
  took down, with no route left from the node the visa service docks on
  (`nodes_cut_off`), gets nothing; its VSS worker is stopped. It finds its
  session broken and rejoins (ZODE_LINKS.md J10).
- **Stopping a VSS worker never waits on it**
  (`ConnectionControl::stop_vss_worker` queues the stop from a detached task),
  for the policy pass, a disconnect and a rejoin's teardown alike. No lock is
  held across a VSS call.

### A by-ID fetch returns the node's pending copy

[#262](https://github.com/org-zpr/zl-zipline/issues/262): a joining or
restarted Zode refused its first `Stream ID Request`s for a few seconds. The
cause was not ingress-last delivery, which holds back only an ingress copy. For
every on-demand visa the requester is the ingress: it gets its copy in the
reply and starts traffic at once, while every other hop gets its copy by a
background VSS push. A Zode whose VSS connection is not up yet (it starts
`VSS_START_DELAY` after registration, and a restart starts a new one) is asked
for a stream before that push can reach it. It fetches the visa by ID, and
`visaRequestById` returned only copies already recorded `Installed`, so the
answer was empty and the Zode refused the request. The schema already said
that an empty answer means the visa does not exist for that node; only the
filter disagreed. The decisions (gate g121):

- **The fetch returns every copy the push would deliver now, or already has**
  (`VisaMgr::visas_for_node_by_ids`): the node's own copy, still grantable,
  under the same ingress-last rule as the push. It does not widen what a node
  can learn, and the ordering guarantee holds: a `PendingInstall` ingress copy
  is fetchable only once every other hop has acked, exactly as the push
  delivers it. An `Installed` copy has already been delivered, so it is
  returned whatever the other hops' state. A quarantined copy, even an
  `Installed` one, is no longer returned.
- **The fetch is read-only** (q3). It does not mark the copy `Installed`. The
  VSS push stays the only acknowledged delivery, so ingress-last waits on real
  acks. The cost is one redundant push per fetched copy, a no-op on the node
  (`insert_visa` of a held ID).
- **Rejected:** answering "pending" and having core retry (b), which needs a
  schema round and a core retry loop and adds latency to wait for a copy the
  visa service already has; and core treating the refusal as retryable (c),
  which masks the bug and adds a retry path to the security-critical bind.
- **Not done:** in a restart, a `Stream ID Request` that reaches the Zode
  before its new VS-API session is up still fails, because there is no
  session to fetch over. It was not seen in the logs or the acceptance runs.

### A departed Zode is removed after a grace (#284)

From the Zodes epic, pd plan p55,
[zl-zipline#284](https://github.com/org-zpr/zl-zipline/issues/284),
org-zpr/zl-zpr-visaservice#72 and org-zpr/zl-zpr-core#138. Before #284
nothing removed a Zode's actor when the Zode went away: a link-down report
only revoked the visas over that link. So a crashed Zode, and the services
it provided, stayed registered for good, and CoreDNS kept resolving them.
The decisions (gates g191, g193, g195):

- **The visa service removes any Zode except the relay once every link of it
  has been down for `ZODE_DEPARTURE_GRACE` (10 s)** (`vs/src/config.rs`).
  This covers transit Zodes too, since "no link up" is the same evidence for
  them. The grace starts when a link report leaves the Zode with no up link.
  The check is scheduled from `reportLinkStates`, for both ends of a link that
  went down and for a peer that the reporter newly reports `down`
  (`schedule_departure_check`).
- **The removal is exact without history.** After the grace,
  `remove_if_departed` takes the topology guard. It removes the Zode only if
  its check is still pending, its `zpr.session` is the one recorded when the
  check was scheduled, and no link of it is up. All three checks and the
  removal run under that one guard, so they are atomic against
  `requestBootstrapVisas`, `connect`, `notify_disconnect` and policy passes.
  A link coming up (a report, an install or a policy pass) cancels the
  pending check. Checks are coalesced per session, and a further loss does
  not extend the grace.
- **The removal is the ordinary disconnect teardown**, `disconnect_locked`
  with `DisconnectReason::Timeout`. It removes the actor record (and so its
  services), trusted-service revisions, the router node, the VSS worker, the
  visas of the node and its actor, and the managed address. No removal code
  is new, and like any disconnect it takes the node commit lock.
- **A graceful stop deregisters at once.** ph sends `notify_disconnect` with
  reason `NodeShutdown` before it tears its links down. It waits at most 2 s,
  and failure is ignored. There is one deregistration path.
- **Why 10 s:** a crash is detected in about 9-12 s, and DNS caches for up to
  30 s, so a crashed service stops resolving within about 52 s. A grace that
  is too short costs only a rejoin as `reset`, which drops what any node
  `authenticate` drops.
- **Known limits.** These are accepted, and the operator declined the first
  and third at g195 q2:
  - an actor-store error while scheduling or checking drops that check;
  - a Zode that leaves while the visa service is down is never removed, since
    a restart schedules no checks;
  - links lost without a report start no check (a rejoin's teardown, a
    policy change).
- **Rejected:** removal when the VSS worker gives up on pings, which is a
  second, later trigger that cannot see rejoins; and a periodic sweep with
  timestamps.

ZODE_LINKS.md rule 11b and "Zode departure (#284)" have the detail and the
tests.

### A lost attribute re-checks the live visas (#265)

From the Zodes epic, pd plan p61,
[zl-zipline#265](https://github.com/org-zpr/zl-zipline/issues/265). Before
it, only a namespace change (a node's login or logout) re-checked live visas.
A session replacement, which persists a fresh device-only node actor and so
drops a logged-in user's login, and a renewal that kept the login but carried
fewer claims, ran no re-check: visas granted on the lost attributes stayed
valid until they expired. The decisions (gate g221, approach A):

- **One check, at both writers of a node's actor.** `attributes_dropped(old,
  new)` (`vs/src/connection_control.rs`) is true when an attribute that does
  not survive a logout lost a value: anything the visa service did not stamp
  itself, and everything in `user.`, that is missing from the new actor or has
  a value the new one lacks. The VS-stamped `zpr.*` attributes are ignored,
  because `zpr.session` changes on every authentication; comparing them would
  sweep on every reconnect. Gains are ignored. `add_node_serialized` reads the
  stored actor under the node commit lock and returns the check's result;
  `reauthorize_actor` sweeps when the namespace changed or the check is true.
- **It reuses the full attribute-change sweep**
  (`sweep_visas_after_attribute_change`, with its bounded retry and its
  fallback to a queued reconcile) rather than a per-actor sweep. It runs only
  on a real loss, so a reconnect that loses nothing costs nothing.
- **Where `authenticate` runs it.** After the topology guard and the node
  commit lock are released, and before the session's later fallible steps
  (the `ActorJoins` enqueue, the denylist re-check). An undo after a failed
  step removes the actor, and a later sweep would then skip its visas.
- **A failed write still sweeps.** `add_node_serialized` sets the flag before
  any write, because an actor update commits the attributes before its other
  writes: a reconnect whose write fails part way may already have dropped the
  login, so `authenticate` sweeps on that error path too.
- **A started sweep survives a cancelled RPC.** The sweep runs on its own
  task, which the RPC awaits, so a client that cancels `authenticate` or
  `reauthorize` mid-sweep cannot drop it half way.
- **Known ceilings.** Gains are not re-checked (a `never allow` on a newly
  gained claim waits for the next policy or attribute event). The obligation
  to sweep is not durable, so it can be lost in a window, as it can for a
  logout. The window covers a visa-service crash between the actor write and
  its sweep, and an RPC cancelled after the write but before the sweep starts
  (in `authenticate`, the docking-hack, router and link-install awaits; in
  `reauthorize`, the post-write metadata and denylist steps). It also covers a
  Reset whose replacing write fails after deleting the old actor, because the
  sweep skips visas whose endpoint no longer resolves while peers may still
  hold them. A retry compares against the already-reduced actor and sees no
  loss. The VS-docking hack in `authenticate` (visaservice#195) can also undo
  the actor before the sweep runs. Durable, retried revocation obligations
  (covering logout too) are
  [zl-zipline#289](https://github.com/org-zpr/zl-zipline/issues/289).

### Attributes are refreshed ahead of expiry (#275)

From the Zodes epic, pd plan p63,
[zl-zipline#275](https://github.com/org-zpr/zl-zipline/issues/275). Before
it, the visa service re-queried a trusted service only once one of its
attributes had expired. A visa cannot outlive the attributes it is granted
on, and one that would live under `MIN_VISA_LIFETIME` (30 s) is denied, so
every visa request that needed an attribute was denied in that attribute's
last 30 s, with the cause logged only at DEBUG. The decisions (gate g225,
approach A):

- **Refresh-ahead inside the request-path refresh.** `refresh_expired_attributes`
  (`vs/src/actor_attributes.rs`) treats a source as due once one of its
  attributes is expired, or is within `ATTRIBUTE_REFRESH_AHEAD` of expiry
  (`due_for_refresh`) and the source is a configured trusted service. A
  background refresher (B) was rejected: it would refresh idle actors, cost
  grows with the actor count, and it is more code. A refresh-and-retry on the
  deny (C) was rejected: it acts only at deny time and adds a second path next
  to the requester-binding checks.
- **45 s, below `MIN_ATTRIBUTE_TTL`.** `ATTRIBUTE_REFRESH_AHEAD`
  (`vs/src/config.rs`) is `MIN_VISA_LIFETIME` + 15 s: a visa issued after the
  refresh gets more than the floor, and the 15 s cover the refresh and the
  request round trip. It must stay below `MIN_ATTRIBUTE_TTL` (60 s, now one
  constant in `vs/src/trusted_services/mod.rs`), because a file store reloads
  its snapshot only once less than that remains; a refresh earlier would get
  the same expiry back. A compile-time `assert!` enforces it.
- **Only configured sources are looked up early.** An attribute from `zpr`,
  or from a service policy no longer declares, has nobody to ask; looking it
  up would log a warning on every request in the window. The expired path is
  unchanged for every source.
- **"Changed" means a real difference.** A source that re-vends exactly what
  the actor holds (an OIDC store's cached admission, a file snapshot not yet
  reloaded) causes no actor write, so a repeated early lookup is cheap. The
  revision is still recorded on the revision path.
- **Pruning on a TTL refresh.** A near-expiry attribute the source stopped
  vending is dropped now; one it did return is kept even with a short expiry
  (a `zpr-attr/1` `expires_at` 40 s away). Visas granted on a dropped
  attribute already end at its expiry, at most 45 s later.
- **The deny stays `NoReason` on the wire, but is logged at WARN** with what
  limited the lifetime (`compute_expiration` returns it). A `DenyCode` for it
  would be a vsapi change in core, not needed to diagnose it.
- **Known ceilings.** Authentication expiry and OIDC claims are renewed by the
  client, not the visa service, so their last 30 s still deny. The WARN names
  `source_actor_auth` or `destination_actor_auth` for an authentication; for an
  OIDC claim it names the claim's attribute key, which also wins a tie with the
  authentication expiry. A source that keeps vending a short expiry (a
  `zpr-attr/1` `expires_at`) is re-queried on every request in its last 45 s;
  there is no per-source back-off or coalescing of concurrent lookups.

### Deferred

No tracker issues are filed for these unless linked. The adapter item at the
end is visa-service code that no Zode reaches since zl-zipline#234, because
no adapter docks on a Zode.

- **Multi-valued `user.zpr.authority`.** More faithful, but it touches
  evaluation semantics and every consumer of the key. Revisit only if a
  deployment needs two concurrent user authorities.
- **Compile-time pool check.** Once the pool ranges are stable enough to share
  through `zl-zpr-common`.
- **A visa granted from a pre-logout actor copy.** A visa request holds the
  endpoint actors across awaits; if it inserts its visa after a logout's sweep
  has taken its snapshot, that visa escapes the logout's revocation and lives
  until it expires or the next sweep re-checks it. Closing this needs the
  grant checked atomically against concurrent revocation.
  ([org-zpr/zl-zipline#248](https://github.com/org-zpr/zl-zipline/issues/248))
- **Refresh vs reauthorize for adapters.** For a node, an attribute refresh
  fetches outside `PolicyMgr::node_commit_lock` and takes it only to commit,
  discarding its write if the node was re-authorized since its copy was
  loaded (the `zpr.vs.bootstrap.ident` token differs) or the session or
  policy generation moved, so a refresh cannot undo a Zode's logout. Adapters keep the older
  race: `refresh_and_persist_actor` can write back an adapter copy loaded
  before a concurrent renewal. The fix is a shared per-actor lock or a
  revision-checked update.
  ([#63 review](https://github.com/org-zpr/zl-zpr-visaservice/pull/63))

---

## Where the implementation diverges from the design document

- **Cap'n Proto, not Thrift.** The design document specifies Thrift for the
  binary policy and the node APIs. The implementation uses Cap'n Proto
  throughout (see `zl-zpr-policy/policy.capnp`, `zl-zpr-vsapi/vs.capnp`); there is no
  Thrift anywhere in the tree.
- **Trusted-service attribute stores were file-backed only** until 2026-09-22:
  the factory accepted `api = "file"` and `api = "oidc"` and rejected
  everything else. The networked `api = "zpr-attr/1"` store landed with
  zipline#78 (see ATTRIBUTE_SERVICE.md). The RFC's network trusted-service API
  was never implemented, and the compiler rejects any `api` other than `file`,
  `oidc` and `zpr-attr/1`.
- **Renewal is a new visa matched by five-tuple, not a replacement message.**
  The design sends a renewal as a replacement naming the visa it replaces.
  For a Zode's core visas the visa service pushes the new visa as an ordinary install, and the Zode
  supersedes the older visa with the same five-tuple (ZODE_LINKS.md J9, J11;
  [#252](https://github.com/org-zpr/zl-zipline/issues/252)).
- **A transit hop frees an expired visa late, and no node re-requests
  early.** As designed, every Zode stops forwarding a stream at its visa's
  expiry ([#271](https://github.com/org-zpr/zl-zipline/issues/271),
  [#274](https://github.com/org-zpr/zl-zipline/issues/274)). But a hop whose
  stream enters over a Zode link keeps the expired visa and its stream ID
  for a 60 s grace, and no node re-requests a visa before it expires (see
  *Expiration, revocation, renewal*).
- **No actor docks on a node.** The design has adapters dock on nodes, and
  the visa service still implements the adapter roles: adapter
  authentication through a node, pool and granted addresses,
  and `revokeAuthentication`. Since
  [zl-zipline#234](https://github.com/org-zpr/zl-zipline/issues/234) every
  node is a Zode, which accepts only Zode links, so in v1 every actor other
  than `vs.zpr` is a Zode's own actor and these paths are unused. The AAA
  prefix push is still live: the visa service sends `Configure([AAA_PREFIX])`
  and the Zode validates it and fills its pool
  (`vss_worker::process_configuration`), though nothing allocates from that
  pool in v1 ([ZODE_LINKS.md](ZODE_LINKS.md), "AAA kept, unused"). The visa
  service has no adapter either: it runs on the relay (see *The visa service
  on the relay, and trusted services*).
- **Authentication and OIDC claims are not refreshed ahead of expiry.** In
  the last 30 seconds of an actor's authentication, or of an OIDC claim that
  bounds a visa, visa requests that need it are denied with `NoReason` (logged
  at WARN, naming the limit) until the client re-authenticates. Trusted-service
  attributes are refreshed ahead of expiry
  ([#275](https://github.com/org-zpr/zl-zipline/issues/275)).
- **Route-aware evaluation is a scaffold.** Stage 2 of `libeval` is defined but
  not implemented.
- **Configuration moved to TOML.** `config-example.yaml` at the repository root
  is prototype-era — YAML, gRPC admin connection, RSA certs "left over from
  prototype". The current service reads `vs.toml`; treat the YAML sample as
  historical.
- The design document leaves several things open, and they remain open: how the
  end-to-end HMAC secret reaches ingress and egress adapters; how path
  constraints interact with deny statements; how administrative disconnects are
  logged when admins act concurrently; when renewal is automatic rather than
  requiring a fresh request; and how nodes are told which other nodes to link
  to. Its sections on state and resiliency, logging, visualization, and
  replication are headings only.
- **Which links a node should have now has a contract**, outside the design
  document: `setTopology` names each peer with its link-certificate CN
  (`Link.peerCn`), and the node answers with `reportLinkStates` (see
  ROUTING.md) ([#217](https://github.com/org-zpr/zl-zipline/issues/217)). On
  `feature/zodes` the visa service fills `peerCn` from policy
  ([#221](https://github.com/org-zpr/zl-zipline/issues/221)) and acts on
  `reportLinkStates` ([#224](https://github.com/org-zpr/zl-zipline/issues/224)),
  including the failed-pair rule 13a; Zodes accept links by `peerCn` and send
  `reportLinkStates` ([#250](https://github.com/org-zpr/zl-zipline/issues/250),
  [ZODE_LINKS.md](ZODE_LINKS.md)). Since
  [#255](https://github.com/org-zpr/zl-zipline/issues/255) `Link.peer` is the
  peer's policy address or, since
  [#264](https://github.com/org-zpr/zl-zipline/issues/264), the address the
  relay observed on its initial link, or unset on a relay link
  when neither is known, and a direct link is offered only when both ends'
  addresses are known (ZODE_LINKS.md rule 13b).

---

## Where the code lives

| Concern | Location |
|---|---|
| Decision logic | `libeval/src/eval.rs`, `eval_result.rs`, `eval_route.rs` |
| Join policy | `libeval/src/joinpolicy.rs` |
| Policy loading | `libeval/src/pio.rs`, `policy.rs` |
| Per-request path | `vs/src/visareq_worker.rs` |
| Policy lifecycle | `vs/src/policy_mgr.rs`, `visa_reconciler.rs` |
| Connection admission | `vs/src/connection_control.rs`, `auth.rs`, `login_nonce.rs` |
| Routing, transit | `vs/src/topology_mgr.rs`, `router.rs`; `libeval/src/policy.rs` (`transitNodes`) |
| A node's substrate address (`substrate_addr_for`); the relay's observations (`Assembly::observed_substrate`, recorded in `request_bootstrap_visas`) | `vs/src/connection_control.rs`, `vs/src/visa_bootstrap.rs` |
| Which links a node is offered (`proposed_links`, `known_substrate_addr`) | `vs/src/vss_worker.rs` |
| Bootstrap visas (`requestBootstrapVisas`, the rejoin teardown, the sweep recheck) | `vs/src/visa_bootstrap.rs`, `vsapi_worker.rs`, `visa_mgr.rs` (`mint_bootstrap_visa`) |
| A node's core visas (J5a, J5b, J9), ingress-last delivery, the by-ID fetch | `vs/src/core_visas.rs`, `visa_mgr.rs` (`deliverable_visas_for_node`, `visas_for_node_by_ids`) |
| Removing a departed Zode (`schedule_departure_check`, `remove_if_departed`; the pending checks in `LinkStateTable`) | `vs/src/vsapi_worker.rs`, `link_state.rs`, `topology_mgr.rs` |
| Policy-pass pushes to nodes | `vs/src/event_mgr.rs` (`push_policy_to_nodes`, `nodes_cut_off`) |
| Attribute sources | `vs/src/trusted_services/` |
| State | `vs/src/db/` |
| Admin API | `vs/src/admin_service.rs`, `admin_apikeys.rs`, `admin-http-api.txt` |
| Configuration and constants | `vs/src/config.rs` |
| Wire formats | `zl-zpr-vsapi/vs.capnp`, `zl-zpr-policy/policy.capnp` |

A change to what policy can express usually spans three repositories — see
[ZPL.md](ZPL.md) and [REPOSITORIES.md](REPOSITORIES.md).
