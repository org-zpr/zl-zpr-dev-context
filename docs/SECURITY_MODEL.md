# ZPR Security Model

What ZPR defends against, what it deliberately does not, and where the guarantees
actually come from in the implementation.

Read this before changing anything in the enforcement path: visa issuance or
revocation, authentication, attribute handling, packet integrity, or key
distribution.

## Sources

Distilled from the ZPR RFCs, principally:

| RFC | Title | Bearing on this document |
|---|---|---|
| internal RFC-9 | Threat Resistance to Bad Actors with Access | The threat model. Structures §"Threat model" below. |
| internal RFC-14.1 | The Reasoning Behind Visas | Why the visa is the security primitive. |
| internal RFC-13.1 | Authentication, Identity and Attributes | Trusted services, attribute trust and expiry. |
| internal RFC-17 | ZDP Protocol Definition | Packet-level security: A2A integrity, SAs, replay. |
| internal RFC-1.4 | Zero-Trust Packet Routing | The foundational design. |
| RFC-16 | ZPR's Concept of Identity | Identity as an attribute-lookup key. Published. |
| RFC-12 | Overview of Zero-trust Packet Routing | Published. |
| RFC-4 | ZPR Terminology | Published. |

RFCs 4, 12, 15, 16, and 19 are published in `zl-zpr-rfcs`. The rest are internal
to Applied Invention; cited here by number, they live in the private RFC
repository.

Where an RFC and the code disagree, the code is what runs — §"Implementation
status" records the gaps found while writing this.

---

## The core idea

**Local origin confers no trust.** Traffic that originates inside the network
is treated with exactly the suspicion given to traffic from outside. There is
no trusted interior.

Two consequences shape everything else:

1. **Every flow must be explicitly permitted.** Communication that policy does
   not allow does not happen; there is no default-permit path. Unallowed
   activity is detected and reported.
2. **Permission is bound to authenticated identity, in the network itself.**
   Not at the endpoint, not at a proxy — at the IP layer, at every node.

A ZPR network is, in effect, a distributed firewall that is aware of
authenticated identities, and that does not require the nodes to perform
firewall-style pattern matching.

### The visa is the security primitive

A **visa** is a granted permission for a specific authenticated endpoint to
send to another under specific circumstances. Packets carry a visa identifier
instead of the usual IP header fields.

That single identifier simultaneously certifies that both endpoint identities
were authenticated, that policy permits this communication, and under which
conditions — and it names the secret key used for the integrity check. Because
all of it derives from the same identifier, **the certification is inseparable
from the destination and conditions**. Delivery of the packet is itself
evidence of policy compliance.

Three properties follow that are hard to get otherwise:

- **The expensive work happens once.** Policy compliance is determined at
  issuance. Per-packet enforcement is a table lookup plus a check of local
  circumstances (time, sometimes a traffic count).
- **Nodes never hold the policy.** Every node enforces policy without policy
  being distributed to it.
- **Revocation is immediate.** Unlike TLS or HTTPS sessions, which survive the
  revocation of the credentials that established them, revoking a visa
  terminates in-progress communication in real time.

A granted visa is never changed. Replacing one means issuing a new visa, and
a Zode keeps per-stream state beside the visa, never in it (ZODE_LINKS.md,
"visas are immutable"). A newer visa for an identical five-tuple supersedes
the older one: the ingress Zode moves a live stream onto it without editing
either visa, and the older one is revoked or expires (ZODE_LINKS.md J11,
[#252](https://github.com/org-zpr/zl-zipline/issues/252)). The rebind is
refused if the two visas' session keys differ, since the stream's endpoints
still hold the old keys.

### Least privilege in the distribution itself

The visa service distributes visa information **only to the nodes that need
it, and only the part each needs**. A forwarding node learns the outgoing link,
the incoming links a packet may legitimately arrive on, and the local
circumstances to check — nothing more. Only the ingress and egress adapters
receive the secret key for the integrity check. **Only the visa service holds
the whole visa.**

This is what limits the blast radius of a compromised node (see Case 2 below).

---

## Identity, attributes, and trust

**An identity is a key for looking up attributes.** It is not a name; it may
have names as attributes, and may be known by different names in different
identity services. Every endpoint, device, user, and service in a ZPRnet has an
identity unique within it.

**Policy is written against attributes, never identities.** That separation is
deliberate: policies become simple statements of what attributes are required,
they seldom change, and they stay distant from the day-to-day churn of
users, roles, and machines. See [ZPL.md](ZPL.md).

An **endpoint** is a 4-tuple: the endpoint identity, the identity of the
network-connected device carrying its flows, an optional user identity, and an
optional service identity. Multiple endpoints may share a device — a server
hosting several services has one endpoint per service.

### What authentication has to prove

Authenticating an identity means proving two things, not one: that the identity
is valid, *and* that it is associated with this endpoint's communication.
Authentication policies specify the required method.

- **Physical devices** typically authenticate through a TPM or hardware
  security module holding serial numbers and keys; **virtual devices** through
  the operating system.
- **Services** are typically authenticated by cryptographic certificates
  binding the service identity to the device and to any port numbers.
- **Users** may authenticate through an identity service, biometrics, card
  readers, or challenge response. A device may have no user, one, or many.

Multiple identity services can coexist in one ZPRnet — necessary across clouds
and customer sites, where the same user may authenticate differently and even
carry a different name in different parts of the network.

### Attributes are only as trustworthy as their source

Attributes come from **trusted services**, and from ZPR itself. Their
trustworthiness is exactly that of their source, which is why the set of
trusted services is named in policy rather than discovered.

- **Attributes expire.** The system tracks lifetimes and will not grant or
  sustain a visa on expired attributes. Expirations are usually set by the
  source; **policy can shorten them but not extend them**.
- **An identity has a lifetime too** — the minimum across all the
  authentications composing it, again reducible by policy. On expiry the
  endpoint must re-authenticate to keep using the network. Bootstrap (RSA)
  authentication is the deliberate exception: it does not expire on a clock,
  because re-proving possession of a static key on a timer demonstrates
  nothing new. What bounds it instead is the policy generation: every policy
  install obliges every connected actor to re-authenticate under the new
  policy within the visa service's `reauth_deadline`, and an actor that
  cannot — its key removed from policy, say — is revoked at the deadline
  (zipline#118).
- **Identities carry provenance.** Each component records the trusted source
  that produced it plus a digital signature attesting authenticity.
- **Identity attributes are immutable and unique.** An authentication service
  that returns a `machine_id` must return the same value for the same machine
  every time.
- **Attribute changes propagate.** Because sources are dynamic, changed values
  feed back into policy checking and can invalidate live visas.
- **The authority marker belongs to the credential verifier.**
  `user.zpr.authority` (and its device twin) names the trusted service that
  *verified the credential* establishing the identity — the OIDC provider that
  validated the token, or `zpr-bootstrap` for a device key. A service that
  merely *adds* attributes to an already-identified actor (a `file` store
  decorating on an identity attribute) never displaces it: the visa service's
  `derive_user_authority` refuses to mint an authority over an existing one
  held by a different source (zipline#25/#26). The one deliberate exception is
  re-derivation *by the same source* — the guard is `existing != source`, not
  `existing.is_some()` — because the authenticator must re-stamp the marker's
  expiry against its own user record on every refresh; blocking that would
  freeze the expiry and let the authority lapse mid-session. Without this rule
  a decorating store could silently take ownership of an identity it never
  verified, and the authenticator's `vouched_here` gate would then prune the
  real identity attributes on the next refresh.
- **A renewed authentication is bound to the session, not to a fresh
  challenge.** An identity's lifetime can be *extended* without the human
  re-authenticating, which is a deliberate relaxation and is scoped to one
  entry point. See *Silent re-authentication* below.

Trusted-service API calls are themselves signed with an HMAC over the function
name, an RFC3339 timestamp, and the canonically serialized arguments; the
service rejects a bad HMAC or a stale timestamp. The API being reachable only
over the ZPRnet is not treated as sufficient — the connection's owner is not
assumed to be the caller. The implemented `zpr-attr/1` attribute-service API
deliberately departs from this: bearer token over pinned TLS, no HMAC — see
ATTRIBUTE_SERVICE.md, "Departure from RFC-13.1", for the reasoning. No
implemented trusted-service API uses the HMAC.

### Silent re-authentication: what replaces the nonce

An OIDC user authentication is renewed in the background so a human logs in
once per session ceiling rather than once per renewal cadence
(`docs/OIDC.md`, *Credential lifetimes and re-authentication*). Renewal needs
a documented relaxation, because **the connect path's nonce check cannot
apply to it**. OIDC Core §12.2 says an `id_token` returned from a refresh
grant

> SHOULD NOT have a `nonce` Claim, even when the ID Token issued at the time
> of the original authentication contained `nonce`; however, if it is
> present, its value MUST be the same as in the ID Token issued at the time
> of the original authentication

— so the claim is *absent or original, never fresh*. Google omits it. Either
way there is no new authorization request and therefore no new nonce to bind,
so a comparison against a freshly issued challenge cannot succeed. A check
that cannot pass is not a control; pretending otherwise would mean either a
permanently failing renewal or a nonce comparison quietly reduced to a no-op.
Both branches must be accepted: the visa service's reauth path performs no
nonce check at all rather than requiring the claim's presence, which is what
keeps a spec-preferred provider working.

So the visa service's `reauthorize` entry point binds the credential to the
**live session** instead. All of these must hold, and each is a rejection:

| Bound to | Why it is the right binding |
|---|---|
| the same `sub` as the admitted actor | the renewal may not change who the actor is |
| a strictly **increasing** `iat` | replaying a captured renewal token is refused; `iat` is the only monotonic per-credential claim |
| an **unchanged** `auth_time` | this is what makes the session ceiling enforceable: a different login is a different session and must reconnect, not renew |
| a `zprAddr` that is a **live actor on the calling node** | the renewal is for an endpoint already docked here, not an address a caller names |
| the **same trusted service** that admitted the actor | one provider cannot renew another's session |
| everything else — signature, `iss`, `aud`, `exp`, `kid`, `alg`, `hd`, `email_verified` | unchanged from the connect path |

**Scope.** The relaxation applies to renewals on `reauthorize` and to nothing
else. A Zode's own login on `reauthorize` is not a renewal and keeps a nonce
check, against a visa-service nonce (*A Zode's own login* below). The
connect path is untouched, and the two paths deliberately do not share a
validation function taking a "skip the nonce" boolean — the nonce expectation
is an enum, so the connect arm cannot be constructed with checking off by
accident.

**The same caller binding scopes `notifyDisconnect` (zipline#182).** A live
node's disconnect notice is accepted only for itself, an adapter docked to
it, or an AAA actor whose registered docking node is the caller; any other
target — another node, an adapter docked elsewhere, an unknown address — is
refused with `invalidOperation`, logged with the caller CN and target
address, and leaves all state unchanged. Without this, one subverted node
could disconnect any actor or node in the ZPRnet, breaking the Case 2
containment ("a compromised component can deny only the services it
provides"). The adapter and AAA arms are the visa service's and stay; since
zl-zipline#234 no adapter docks on a Zode.

**The SS (bootstrap-signature) arm.** `reauthorize` also accepts a
self-signed blob, so a device authority is re-proved the same way install-driven
re-authentication demands (zipline#120). One `reauthorize` carries at most one
blob per namespace. For an adapter the namespace set must equal the live
actor's credential-backed authorities exactly — an actor that authenticated as
RSA+OIDC re-proves both or fails. A node renewing its **own** actor may also
change its `user` namespace (see *A Zode's own login* below); the `device`
blob is mandatory either way and the `device` namespace can never be gained or
dropped. Any other set mismatch is a `paramError`. The SS arm requires all of: the target is a live actor admitted
through the calling node; the actor holds a bootstrap `device.zpr.authority`;
the blob's CN equals its authenticated CN; the signature verifies against the
**pinned new snapshot's** bootstrap key; and the blob's timestamp is within
the maximum clock skew (180 s). There is no monotonic-timestamp rule:
freshness comes from the node-minted, HMAC'd challenge (at most 120 s old),
and only the docking node could replay — a compromised docking node is
already Case 2.

**A Zode's own login: the nonce comes from the visa service.** On a Zode the
machine's actor is the node's own actor, and a user login adds the `user`
namespace to it through `reauthorize` (logout drops what the login added;
device-keyed enrichment may restore an equivalent user namespace, below). The connect path
binds an OIDC token to a nonce derived from the docking node's link challenge;
that protects against a replayed `id_token` only because the docking node is a
separate party. A Zode is its own docking node, so a node-minted nonce would
let a subverted Zode log in as anyone whose `id_token` (and nonce) it holds,
until the token expires. So a login must carry a nonce the visa service
issued through `getLoginNonce`:

| Bound to | Why |
|---|---|
| the calling node's actor address | a nonce issued to one node never serves another |
| its VS-API session generation (`zpr.session`) | a re-authenticated node starts with no usable nonces |
| one use, spent as soon as the request is classified as a login — before its own binding and expiry checks, the device proof and the token | a login that fails on any of them cannot be retried with it |
| a short lifetime (300 s) and at most 4 outstanding per node session | bounds what the visa service holds; `quotaExceeded` past it |

Only the namespace-set check runs before the nonce is spent; a request with
the wrong blob set leaves it unspent. After the nonce, the device proof and
the token are checked, the token exactly as on the connect path. Only the
node renewing its own actor may gain or drop the namespace; an adapter keeps
the exact set. A device-only
self-renewal drops any user authority the actor holds, including one whose
OIDC provider policy no longer declares, and a node's reauthorizations of its
own actor are serialized, so two concurrent logins cannot both succeed; a
node's re-authentication persists its new `zpr.session` under the same lock,
and a self-renewal is refused if that session changed while it was in flight.
A logout clears the previous login's state: every `user.*` attribute —
whatever store vended it — and the login's session anchors. The policy re-run
then re-enriches the actor from its device identity, which can legitimately
restore `user.*` attributes (and a trusted-service-derived user authority)
that a store vends for that device. Every login and logout re-checks all live
visas against the resulting actor, so a visa is revoked when it no longer
passes policy — in particular one that needed the logged-out user's
attributes. What remains: a subverted Zode can still use a login of its own
user, as any endpoint can.

**Who may log in or out on a Zode.** Anyone who can open ph's control
channel: the socket owner or the `control_group` on Linux and macOS, the
owner, Administrators or the `control_group` on Windows (the named pipe's
DACL). There is one logged-in user per Zode, so any such local user logs in
or out for the whole machine. ph keeps no tokens: the refresh token stays in
the `ph-cli auth-agent` process, and ph holds only the issuer, the login
nonce and the expiry, and logs neither tokens nor nonces.
([zl-zipline#233](https://github.com/org-zpr/zl-zipline/issues/233))

**A replaced session drops the login.** Every node authentication (a VS-API
reconnect, a visa-service restart, a rejoin, a K2 re-authentication) builds
a new, device-only actor, and only `open` keeps the stored one, which ph does
not use. The login cannot come back silently: a login needs a fresh
visa-service nonce in the token, and a refresh-grant `id_token` carries none.
So ph marks the login logged out (`SessionReplaced`) and asks a resident
`auth-agent` for a new interactive login. A policy install's
`requestAuthentication` for a logged-in Zode is answered with a renewal,
which re-runs authorization under the new policy and keeps the user; it
falls back to K2, which drops the user, only when no renewal can be had. When
nothing renews a login, ph logs the user out itself at the expiry: the visa
service has no expiry sweep for a node's own actor.

**Known gap: live visas are not re-checked when a session replacement drops
a login.** A node `authenticate` calls no visa revalidation, so visas granted
on the user's attributes live until they expire. An explicit logout and the
expiry logout do re-check them (above); `zode-login-test.sh` sees a logout
revoke the user flow's visa. Same class as "ordinary renewals with
changed claims skip visa revalidation". A visa-service follow-up:
[zl-zipline#265](https://github.com/org-zpr/zl-zipline/issues/265).

**What this hands to an attacker, and why it is not a new threat.** The
binding is enforced by the visa service against its own session record, and
the only party that can present a renewal is one that can both mint a
refresh-grant `id_token` for that `sub` and speak as the actor's docking
node. A **compromised node** can therefore keep one of its own already-docked
actors authenticated for as long as the ceiling allows. That is Case 2, and it
adds nothing: a compromised node already forwards that actor's traffic, and
the node is the party that relays every connect and renewal in the first
place. It cannot extend past `auth_time + max_auth_age_seconds`, cannot renew
an actor docked elsewhere, cannot change the actor's `sub` or its attributes,
and cannot replay a token twice. The cryptographically stronger alternative —
a user-held keypair bound to `sub` at first login — is recorded as deferred in
`docs/OIDC.md` (*Deferred*) and is the thing to revisit if this relaxation
fails a later review.

**Running as root: the mitigation holds, conditionally.** The refresh token
that makes renewal possible is held in memory by `ph-cli auth-agent` for the
life of that process — never on disk, never logged, never returned over the
RPC. `docs/OIDC.md` argues this is acceptable partly because the token "lives
in the *user's* session rather than the root daemon". Whether that holds
depends only on how `ph` was started, because the control socket is handed
off after bind (`adapter/ph/src/sys/posix/socket_access.rs`,
`adapter/admin-api/src/socket_owner.rs`):

- `ph` started via `sudo`/`pkexec`: the socket is chowned to the invoking
  user at `/var/run/zpr/<uid>/control.sock` (mode 0600), `ph-cli auth-agent`
  runs unprivileged, and the refresh token lives in a process owned by the
  invoking user's uid. **The mitigation holds.**
- `ph` started by systemd with a `control_group` configured and present on
  the host: the shared socket is in that group (mode 0660), group members
  run `ph-cli` unprivileged, and the token lives under the member's uid.
  **The mitigation holds.** (On Windows the same setting adds the group to
  the control pipe's DACL, with the same effect for a service-run `ph`.)
- Residual case — no recorded owner and no usable `control_group` (none
  configured, or configured but absent; since zipline#154 there is no
  implicit `zpr` group), under systemd or a direct
  root login on an unpackaged host: the socket stays root-only, `ph-cli`
  runs under `sudo`, and the refresh token sits in a root-owned process for
  hours. The marginal risk is small — root already owns the tun device and
  could harvest an `id_token` at any interactive login, so what is added is
  persistence, not access — but it is recorded here rather than inherited,
  and deployments that will not accept persistence leave
  `allow_offline_access = false`.

---

## Threat model

From internal RFC-9, loosely following STRIDE. The stated assumption is strong:
**the adversary has unlimited ability to modify and control the software on the
endpoint they have access to**, whether obtained legitimately, by compromising
a device or agent, or by compromising the identity management system.

### Case 1 — a bad actor with network access

An authorized-but-compromised endpoint; no ZPRnet component compromised; not an
administrator.

| Threat | What ZPR provides |
|---|---|
| Spoofing | Source and destination of every packet are authenticated. |
| Tampering | Communication between endpoints cannot be modified without detection. All unexpected communication is disallowed. |
| Repudiation | The source of all communication is authenticated. The source of every configuration change is authenticated and recorded. |
| Information disclosure | ZPR headers are encrypted across links; payloads associated with a permission are encrypted across links and within forwarders when policy specifies. The only information exposed about policies, devices, and connectivity is what can be inferred from allowed communication, from the failure of attempted communication, or what policy explicitly exposes. Caps on allowances limit exfiltration. |
| Denial of service | Endpoints can use network services only within quantitative limits set by policy. Internal services are protected because communication with them is capped the same way. ZPR is immune to traffic injection on links; where links are virtual, an attack on the substrate can only degrade the virtual link. Policy assertions can require minimum topological redundancy. |
| Elevation of privilege | Privileges are limited by network-enforced policy. Compromising a single device cannot elevate privilege. Global limits — total exfiltrated data, connection counts — are enforceable. |

The worked example: a web server bridging the public Internet and a ZPRnet is
fully compromised from the Internet side. On the ZPRnet the attacker can send
packets only to the addresses and ports policy allows that web service, within
policy's resource limits. They **cannot**:

- send packets claiming a different or unused ZPR address — detected at the dock;
- port-scan or probe for unknown systems — blocked at the dock;
- discover ZPRnet configuration beyond the addresses they may already talk to;
- discover policy, except by inference from what is and is not allowed;
- participate in a DoS beyond policy-compliant traffic within its limits.

If an endpoint becomes untrusted, an administrator can revoke all its active
visas and cut off its communication immediately.

### Case 2 — a bad actor who also compromises a component

ZPR assumes conventional protections (supply chain, physical security, trusted
platforms, cryptographic protection against reprogramming) are **not
infallible**. Docks get the most attention: they are the most exposed, because
their function requires accepting connections from non-ZPR networks.

The design goal shifts from prevention to **containment, detection, and
traceability**:

| Threat | Limit |
|---|---|
| Spoofing | Misattribution is confined to endpoints sharing the compromised dock. |
| Tampering | Confined to communication passing through the compromised dock. |
| Repudiation | Anonymity is limited to what spoofing achieves. |
| Information disclosure | Disclosure is limited to what an endpoint discloses to that dock; a component learns policy only for the specific communications active through it. |
| Denial of service | A compromised component can deny only the services it provides; the network has other means of providing them. |
| Elevation of privilege | Because all administration happens through the network, ZPR resists privilege elevation via physical access. |

The worked example: total control of a forwarding node, with all its state. The
attacker can drop, copy, spoof, or modify traffic through that node until
detected, and can measure its traffic statistics. They **cannot**:

- read the contents of the packets they forward;
- see the IP source or destination — the forwarder holds only the fragment of
  visa state needed to forward, insufficient to recover addresses;
- exfiltrate by sending packets to themselves through the network;
- forge packets under another endpoint's permissions — the next node or the
  egress dock detects and blocks them;
- probe for other systems or ports — the connected nodes block it;
- discover ZPRnet configuration beyond the nodes they directly connect to;
- discover policy beyond the specific flows passing through them;
- mount a DoS beyond duplicating policy-compliant packets within the flows'
  limits, which is detected and reported as anomalous.

### Case 3 — a malicious administrator

Administrator status is just an attribute, and only administrators may change
policy and configuration. Because administration happens *through the network*,
ZPR can defend against rogue administrators in ways conventional networks
cannot.

The structural answer is **k-of-n concurrence**: policy can require that
consequential configuration changes — including policy changes — be
authenticated by several administrators. The RFC's example requires every
packet reaching the visa service's policy-changing ports to be authenticated by
three different administrators, on specific authorized equipment, in three
different protected locations.

Since the concurrence requirement is itself network policy, the network
protects itself from single-administrator error or malice. **A k-of-n policy
reduces Case 3 to Case 1 or 2.**

Administrators remain the likeliest route to component compromise, because they
have physical access. ZPR components are therefore designed not to be
reconfigurable through physical access.

---

## Packet-level protection

### Adapter-to-adapter (A2A) integrity

Endpoint packets are protected end to end by an integrity check — the Message
Integrity Check Value (MICV) — computed by the ingress adapter over the
**entire IP packet before compression**, so the original addresses and ports
are covered.

The key material is generated and distributed **by the visa service as part of
installing the visa**, reaching the adapters through their docks. The egress
adapter looks up the security association, decompresses, recomputes, and
compares.

The critical property: **docks and intermediate forwarders never hold the A2A
key material.** They cannot check the MICV and cannot generate one — which is
precisely what stops a compromised dock or forwarder from injecting undetectable
endpoint-to-endpoint data.

A failed A2A lookup or a bad MICV means the packet is silently discarded and
the event reported as a potential security issue. Silent discard is
deliberate — an error response would itself be an information leak.

Where the protection is unnecessary, such as a physically protected network, a
zero-length MICV and no encryption are permitted to save computation.

### Link security and security associations

A security association fixes the session key for encrypting the ZDP header, the
session key for its MICV, the algorithms, the MICV size, which of its bits ride
in the header, and the block size. SAs come from the key management protocol or
are set directly by the admin service.

Multiple SAs may be active at once, and a transmitter should choose randomly
among them. An SA being retired goes inactive: it may not be used to send, but
receivers must still process packets under it so in-flight traffic can drain.

### Replay

All ZPR packets carry per-SA sequence numbers for replay protection. They must
be wide enough never to roll over within the SA's lifetime — 64 bits
recommended — and a near-wrap forces a new SA with new keys and a reset
counter. Only the low-order bits ride in the header (16 in the baseline
configuration), but **the whole sequence number is covered by the MICV**. The
receiver must reject a repeated sequence number, typically with a sliding
window as in RFC 4303.

---

## What ZPR does not defend against

Stated plainly in internal RFC-9: not every security property a ZPR deployment
depends on is enforced by the network. These remain external:

- **Identity management and authentication systems.** ZPR depends on them and
  is not part of them. It can help maintain their integrity when they are
  reachable through the ZPRnet, but it cannot ensure it.
- **Credential lifecycle** — issuing, validating, distributing, revoking, and
  controlling access to credentials, and informing administrators of changes.
- **Supply chain integrity** for hardware and for the software and firmware
  initially loaded onto it.
- **Control of the startup channel.**
- **Trust evaluation.** A deployment may use an external service to compute
  something like a trust score; how it does so is external policy.
- **Integrity of administrator tooling** and of the external databases policy
  references.
- **Configuration management discipline** — for instance, how much testing a
  new configuration gets before activation. ZPR supports testing a
  configuration before it goes live; requiring it is external policy.

Internal RFC-9 is explicit that it states goals rather than delivering a
comprehensive threat analysis, and that a full analysis must account for all of
the above.

---

## Design decisions

**Non-transit has two halves, and the node's half is defence in depth.** A
machine whose owner is not trusted with other people's traffic must not relay
it: payloads are protected only link by link today (A2A confidentiality is not
implemented, below), so a relay can read what it forwards. The primary check
belongs in the visa service: path computation must never route through a node
that policy does not list as transit-capable (`Policy.transitNodes`). That check
is built ([zipline#255](https://github.com/org-zpr/zl-zipline/issues/255),
*Implementation status*, below), and it fails closed: an unset or empty list
means no node may carry other nodes' traffic. The node's check is its `forwarding`
setting (`[node] forwarding`, `ph zode --forwarding`, default off). With it off,
the node refuses every `forwardOnly` visa. The setting cannot be the only
guard, because a subverted node can turn it on; that is why the visa-service
half is required. ([zipline#219](https://github.com/org-zpr/zl-zipline/issues/219))

**Zode links are mutually authenticated, and the peer's name must come from the
topology.** Both ends of a Zode-to-Zode link require the peer to present a
certificate signed by the network's CA, inside its validity window, whose key is
the peer's Noise static key; a certificate-less peer, another CA's certificate,
an expired or not-yet-valid certificate, or a key mismatch aborts the
handshake. A Zode without a CA certificate starts no Zode links at all rather
than linking unverified. A CA signature only proves "some registered Zode of
this network", so the CN is checked too, when keying finishes and before
Hello, and the check depends on the direction. The answering end keeps an
inbound link only if the CN equals a `Link.peerCn` in the topology the visa
service last pushed and that peer owns the link; a relay owns no link, so it
accepts any peer in its topology. The dialing end keeps a link only if the CN
is the one it dialed for: the peer's `peerCn`, or for the initial link the
relay CN in its own config. A `Link` with an empty `peerCn` is rejected. Every
topology update closes the links whose peer left it, except the Zode's
configured initial link, which its config, not the topology, authorizes. That
is what makes deleting a Zode effective without a revocation list: its
still-valid certificate names a peer no topology lists, so no relay or peer
accepts it. Deleting a relay takes reconfiguring the Zodes that name it as
their initial link.
([zipline#220](https://github.com/org-zpr/zl-zipline/issues/220),
[#250](https://github.com/org-zpr/zl-zipline/issues/250); see
[ZODE_LINKS.md](ZODE_LINKS.md) rules 3 and 12)

**ZIPLINE issues each Zode's link certificate at registration, from a
per-customer CA whose key no container can read.** Each ZIPLINE network has
its own RSA CA, and a network belongs to one customer, so the CA is that
customer's trust root. The ZIPLINE web server keeps the CA's private key at
`${ZIPLINE_DATA_DIR}/ca/<networkId>/ca.key` (directory 0700, file 0600). It is
deliberately not in the network's data directory, which is bind-mounted into
the network's root containers: a compromised relay or visa service holding the
key could mint a certificate for any CN. The key never appears in a log, an
error or an API response.

- **The request.** `zipline register` generates the Zode's X25519 link key on
  the device and sends only the public half. The register request is signed
  (RSA-SHA256, PKCS#1 v1.5) with the registration key, the RSA key the Zode
  later authenticates to the visa service with, over
  `zipline-register\n<hostname>\n<os>\n<pubkey>\n<linkPubkey>`.
- **The certificate.** When the user confirms the registration, the server
  signs a certificate with CN = the Zode's name (`zode-` plus six base-36
  characters), the link key as its SPKI, and keyUsage keyAgreement. It is
  valid until the CA itself expires, at most ten years, and v1 has no renewal.
  Deleting a Zode is made effective by the CN-in-topology rule above, not by
  expiry or a revocation list.
- **What the signature proves.** It binds the link key to the registration
  key, so nothing on the path can swap in a link key of its own. It does not
  prove that the device holds the X25519 private key, because X25519 cannot
  sign. That is acceptable: a certificate for a key you don't hold cannot
  complete a Noise handshake, so it authenticates nobody.
- **No fallback.** A request without the link key or the signature is
  refused. Every network has a stored CA key: one provisioned before the
  change got one at the Zodes flag day
  ([org-zpr/zipline#254](https://github.com/org-zpr/zipline/issues/254)), so a
  missing key at registration is a server fault (a logged 500), not a client
  error.

Rejected: a one-year certificate with renewal (renewal code on both sides,
where deleting the Zode already handles a leaked key); the server generating
the link key (it would then hold every Zode's private key); keeping the CA key
in the database, encrypted with a server secret (key management for a key
that re-provisioning replaces anyway).
([org-zpr/zipline#251](https://github.com/org-zpr/zipline/issues/251), gate
g113 q3, q6 and q7; the 2026-11-13 entries in zipline's
`docs/decisions/adapters.md`)

**On ZIPLINE an endpoint is a node, and the node role gives it nothing an
adapter didn't have** (Zodes plan §7 check 4). Every ZIPLINE machine
authenticates as a node, so it matches its own node join policy. An adapter
matches every join policy except the ones flagged node.
`Policy::join_services_for_role` gives a node all of them, and only while it
still matches its node join policy. So what a Zode gains over an adapter with
the same CN and tags is exactly what its node join policy grants (compiler
`set_connects` and `add_node`):
- `/zpr/<cn>`, a placeholder service on TCP port 1. No communication policy
  grants it, and no ZPL statement can name it.
- `zpr/<cn>/vss`, the node's visa support service. The compiler's only rule for
  it is the built-in "allow visa service to access node visa support", so only
  the visa service reaches it.

The node join policy also pins the CN to the node's `zpr_address`, so the visa
service refuses a Zode that asks for another address. The compiler flags every
node join policy `vsdock` (its TODO "Not all nodes are VS docks"), but the visa
service only parses that flag and never acts on it. A ZIPLINE compile test pins
all of this against the compiled policy with zpdump. The node role also enables
things that aren't services: node authentication, a self-renewing `reauthorize`
(zl-zipline#232) and a router vertex (never transit unless listed). It also
enabled adapter docking until zl-zipline#234; no Zode accepts a dock now.
([org-zpr/zipline#253](https://github.com/org-zpr/zipline/issues/253))

**A direct link is proposed only between Zodes that an `allow` rule selects.**
ZIPLINE derives each direct link from the selectors of an `allow` rule
(ROUTING.md, "Implementation status"). A link carries nothing without a visa,
so one link too many grants no access. What a link does disclose is each end's
substrate address to the other (`Link.peer`). Deriving links from `allow`
selectors limits that disclosure to the pairs those selectors name. It is not
limited to pairs that may actually talk: `never` rules are ignored, so a pair
that an `allow` rule selects learns each other's address even when a `never`
rule denies all of its traffic. Meshing every endpoint was rejected because
every endpoint would then learn every other's address.
([org-zpr/zipline#253](https://github.com/org-zpr/zipline/issues/253))

**A joining node's bootstrap visas are requested on demand, and only by the
relay.** Bootstrap visas let a node that has no actor yet reach VSAPI, so
whoever carries them relays that node's traffic. `requestBootstrapVisas` is
the trust boundary for them (ZODE_LINKS.md J2–J4, VISA_SERVICE.md "Bootstrap
visas, on demand"):

- **Who may ask.** Only the node the visa service docks on (the relay), over
  its own live VS-API session. Any other caller is refused with
  `invalidOperation` before its `peerCn` is even decoded, so it learns nothing
  about which CNs exist.
- **For whom.** Only a peer the relay's topology names by `peerCn`, which the
  relay takes from the joiner's verified link certificate. The joiner's ZPR
  address comes from that topology, never from anything the joiner claims.
  Anything else is `notFound`.
- **For what.** Only the joiner's VS-API flow, both directions, for 5 minutes,
  with no session keys. The peering declared in policy is the authorization;
  policy evaluation cannot answer for a node with no actor.

Every other neighbour of an unconnected node gets the link and no bootstrap
visas, so the visa service's half of the join path cannot turn an endpoint into
a transit node. The node's half is built too: the joiner accepts a hand-off
only on its initial link, from the relay CN in its own config, after Hello
(never inside it), with no VS-API session yet, and only exactly its own two
VS-API visas, unexpired (ZDP.md, "Bootstrap Visas"). A relay that lies can
then affect only the joiner's VS-API flow, which it carries anyway. Each
request is a join: it revokes every visa between the joiner and the visa
service, and ends any session the visa service still holds for the joiner.
([#221](https://github.com/org-zpr/zl-zipline/issues/221),
[#252](https://github.com/org-zpr/zl-zipline/issues/252))

**Control traffic between a node and the visa service carries no A2A keys; it
relies on TLS, and that TLS does not authenticate either end yet.** A node's
core visas (its VS-API pair and the visa service's VSS visa to it, with their
renewals) are minted with an empty `KeySet`, like the bootstrap visas they
supersede (gate g100). The bootstrap visas cannot carry keys, because the
joiner has not authenticated, and a Zode refuses to rebind a live stream onto
a visa whose keys differ, so keys on the core visas would stop the supersede
and force a rejoin every 5 minutes. The traffic on those flows is the VS-API
and VSS connections, which are TLS. **That TLS authenticates neither end
today:** the client side of each (the node for VS-API, the visa service for
VSS) accepts any server certificate (`NoVerification`) and presents none. So
it gives confidentiality against a passive observer only. The node proves
its identity to the visa service at the application layer (`authenticate`,
an RSA challenge), but nothing proves the visa service's identity to the
node, or the node's to the visa service on VSS, beyond the ZPR path itself:
Noise-keyed Zode links with CA-checked CNs, and visas the visa service
issued. Authenticating both ends with ZIPLINE-provisioned per-customer
certificates is [#259](https://github.com/org-zpr/zl-zipline/issues/259).
ZIPLINE registration
([org-zpr/zipline#251](https://github.com/org-zpr/zipline/issues/251)) issues
only the link certificate; #259 will reuse its per-network CA-key store and
signing helper for those TLS certificates (gate g113 q2).
Rejected: letting the rebind cross a key change for node-to-VS flows only (a
special case in J11's guard).
([#252](https://github.com/org-zpr/zl-zipline/issues/252))

**On the relay host, traffic between the relay and the visa service is not
visa-enforced.** The relay hosts the visa service on its own TUN, so `vs.zpr`
is a local address of the relay, and the kernel carries the relay's VS-API
session and the visa service's VSS session to the relay over `lo`. Neither
reaches `ph`, so no visa covers them. The relay host is the visa service's
trusted base: whoever controls it controls the visa service anyway. Traffic
between `vs.zpr` and every other Zode still crosses the relay's local actor and
needs visas. With two addresses on one interface, the kernel may source a
connection from the wrong one, and the wrong source has no visa (or is refused
by the visa service). So every client on the relay host that talks to the
ZPRnet binds its source: the relay's VS-API connection binds the node's own
address, and the visa service's VSS and proxied JWKS connections bind
`vs_addr`. A new client there must bind too. There is no `preferred_lft`
handling. ([zl-zipline#229](https://github.com/org-zpr/zl-zipline/issues/229),
[#228](https://github.com/org-zpr/zl-zipline/issues/228),
[#240](https://github.com/org-zpr/zl-zipline/issues/240); ZODE_LINKS.md, "The
relay hosts the visa service (#229)")

**A visa is enforced until its expiry on every packet, not only when a flow
starts.** The visa service bounds a user flow's visas by the login's expiry
and forgets a visa when it expires, so it never revokes an expired one: the
Zodes must stop it themselves. Each forwarding entry carries the bound
visa's expiry and the fast path checks it per packet, so access ends at the
visa's expiry on flows that were already open too, which the login's
expiry bounds from above. A packet is dropped once the fast-path worker's
clock sample reaches the expiry: the clock is read once per wakeup, so a
batch that began just before the expiry still passes, and a skewed clock
moves the cutoff
([#281](https://github.com/org-zpr/zl-zipline/issues/281)) (operator, gate g184;
[#274](https://github.com/org-zpr/zl-zipline/issues/274)). The sweep that
frees expired visas reclaims memory and enforces nothing. Rationale and the
rejected options: ZODE_LINKS.md, "Design decisions" (pd plan p53).

**The visa service mints a Zode's login nonce.** Every endpoint is a node, so
"a compromised docking node" now includes every laptop; a nonce the endpoint
mints itself cannot stop it replaying a stolen `id_token`. The operator chose
a visa-service nonce (`getLoginNonce`) over accepting that risk for v1. See
*A Zode's own login* above.
([zl-zipline#232](https://github.com/org-zpr/zl-zipline/issues/232))

**A departed Zode is removed on link evidence that a single peer cannot
forge.** The visa service removes a Zode other than the relay, with the
ordinary disconnect teardown (reason `Timeout`), once every link of it has
been down for `ZODE_DEPARTURE_GRACE` (10 s) in the same VS-API session
([#284](https://github.com/org-zpr/zl-zipline/issues/284); VISA_SERVICE.md,
"A departed Zode is removed after a grace (#284)"). A link counts as up only
while both of its ends report it up (ZODE_LINKS.md rule 14). So a subverted
peer can take down only its own links to a Zode. It can get the Zode removed
only if it is an end of every link the Zode has, and then it could cut the
Zode off anyway. That keeps Case 2 containment. A removal destroys nothing
the Zode cannot get back: the Zode rejoins on its own as `reset`, losing what
any node `authenticate` drops (a user login, and a managed address that has
no static grant). A graceful stop deregisters through the caller-scoped
`notifyDisconnect` above, for the caller only, so it adds no new authority.
Removal also releases a managed address, which may be handed to another
actor; DNS answers for the old holder end within the cache TTL (DNS.md).

## Implementation status

What the code does today, checked against the RFCs while writing this.
`zl-zpr-core` is a **pre-release reference implementation** whose README states
that the full suite of end-to-end security features is not yet implemented — do
not read the RFCs as a description of current guarantees.

**Implemented:**

- **A2A integrity** — keyed BLAKE3 over the packet body, 8-byte MAC. The key is
  derived with `blake3::derive_key` under a versioned context string
  (`ZDP_A2A_MICV_KEY_CONTEXT`), from the secret the visa service distributes.
  The admin API shows the distribution side: a visa carries a `session_key` with
  separate base64 `ingress_key` and `egress_key`.
- **Link security** — three modes selected per security association: null,
  HMAC-only (keyed BLAKE3, 8-byte MAC appended after the A2A MAC), and full
  encryption through a codec.
- **Link key management** — Noise `Noise_XX_25519_ChaChaPoly_BLAKE2s` (the
  `snow` crate), with X.509 and Ed25519 certificate exchange.
- **Enforcement at the dock** — the dock verifies each packet against the visa
  authorizing the flow, and drops and counts mismatches.
- **Bootstrap authentication** — CN-to-RSA-public-key mapping with challenge
  signing, for the components that must connect before trusted services exist.
- **Revocation** — visa revocation over the VSS-API, plus administrative
  revocation of visas, endpoints, and trusted services without a policy
  install. See [VISA_SERVICE.md](VISA_SERVICE.md).
- **Visa expiry on every use of a visa** — every Zode on a stream's path
  drops its packets from the visa's expiry on, by its own clock: each
  forwarding entry carries a copy of the bound visa's expiry, checked on
  every packet at the ingress, at transit and at egress delivery, and
  counted as `Visa Expired`. A J11 rebind moves the next hop and the
  expiry together, atomically, so a renewed stream lives as long as its
  new visa and one whose rebind failed stops at the old visa's expiry. A
  Zode never matches a new flow to an expired visa (#271), and a bind or
  a J11 commit refuses one, including a visa just fetched by ID. A sweep
  on the link keep-alive (at least 10 s apart) then frees expired
  visas: at the ingress on its first run after the expiry, withdrawing
  the local streams, so once the actor has processed the withdrawal its
  traffic is matched afresh (a new visa request, unless the lookup finds
  another live visa); at a hop a stream enters over a Zode link, only
  60 s later (`STREAM_ID_REUSE_GRACE`), so its stream ID is not reused
  while an upstream Zode may still send on it. That grace is the clock
  skew between Zodes it is designed to tolerate, not a bound the code
  enforces: an upstream lagging by more than 60 s could still send on a
  reused ID. Expiry is an absolute instant compared with
  each Zode's clock, so a skewed clock stops a stream early or late
  ([#281](https://github.com/org-zpr/zl-zipline/issues/281)). See
  [#271](https://github.com/org-zpr/zl-zipline/issues/271),
  [#274](https://github.com/org-zpr/zl-zipline/issues/274), and
  ZODE_LINKS.md, "Visa expiry on every use (#274)". Before #271 `ph` never
  checked expiry when binding a flow (a joiner already refused an expired
  bootstrap hand-off: ZODE_LINKS.md J4, ZDP.md "Bootstrap Visas"), and
  since the visa service never revokes an expired visa, a Zode kept
  admitting new flows under it; before #274 a stream bound before the
  expiry kept forwarding after it.
- **Authentication expiry is enforced, not just recorded.** A periodic sweep
  in the visa service revokes the authentication of every connected adapter
  whose expiration has passed and drops the actor, batched one
  `revokeAuthentication` per docking node and removed only on a positive ack,
  so a transport outage defers rather than half-removes. The node honors the
  revocation through its normal disconnect path. The same sweep enforces
  install-driven re-authentication (zipline#123): a policy install records the
  new generation and a deadline, the sweep re-sends `requestAuthentication` to
  actors still authenticated under an older generation, and once the deadline
  passes it revokes them — adapters through the same batched
  `revokeAuthentication`, nodes by disconnect. This is what makes the
  identity-lifetime rule above an enforced bound rather than a stored value.
  Since zl-zipline#234 no adapter docks on a Zode, so a Zode acknowledges a
  `revokeAuthentication` without acting on it (`process_revoke_auth`), and
  it acts on a `requestAuthentication` only for its own actor.
- **Session-bound renewal** — the `reauthorize` bindings in *Silent
  re-authentication* above are implemented and unit-tested per row
  (zipline#43).
- **Caller-scoped disconnect** — `notifyDisconnect` accepts only the calling
  node itself, adapters docked to it, and AAA actors registered to it; see
  *Silent re-authentication* above for the binding (zipline#182).
- **Non-transit, node half** — a node with `forwarding` off refuses
  `forwardOnly` visas: a pushed one is dropped, logged and acknowledged, and a
  bind that needs one is refused ([zipline#219](https://github.com/org-zpr/zl-zipline/issues/219)). See *Design decisions* above.
- **Non-transit, visa-service half** — path computation passes only through
  nodes listed in `Policy.transitNodes`, and an unset or empty list means none
  ([zipline#255](https://github.com/org-zpr/zl-zipline/issues/255)). A policy
  that takes a node off the list stops new routes through it at once, before
  any step of the policy pass that can fail. The visas already through it are
  then marked for revocation, and VSS housekeeping revokes them on the nodes,
  so they stay usable until that reaches them; a failed revocation write is
  quarantined and retried. Startup marks restored visas the same way, so a
  pass cut short by a restart does not leave them unrevoked. A policy entry
  that is not an IP address fails the policy load. See
  ROUTING.md, "Path computation" and "Transit changes".
- **A reported substrate address is checked, and steers only dials** — a
  Zode's `connect` or `open` reports the substrate address its peers should
  dial. An unspecified IP or port 0 is refused, and so is any address other
  than the one policy declares for the node; with none declared, the report
  stands (`accept_reported_substrate`,
  [zipline#255](https://github.com/org-zpr/zl-zipline/issues/255)). The address
  only goes into peers' `Link.peer`: the link still needs the node's CA-signed
  certificate with the CN policy gives it. Whether a Zode can misuse the address
  it reports, for example to make peers dial a third party, is
  [zipline#264](https://github.com/org-zpr/zl-zipline/issues/264).
- **A Windows Zode accepts direct links through Windows Firewall.** The
  ZIPLINE MSI installs an inbound rule, "Zipline (direct links)": `ph.exe`
  only, UDP 5000, every profile, removed with the package
  ([zipline#268](https://github.com/org-zpr/zipline/issues/268), confirmed by
  the #255 demo).
  - **Why it is needed.** Without the rule, the dial from a peer with a lower
    ZPR address was dropped and the pair stayed on the relay (ZODE_LINKS.md
    rules 1 and 13a).
  - **What it exposes.** Anyone on the substrate can now reach `ph`'s
    substrate listener, as on Linux and macOS, where nothing filters it.
    Only the Zode link responder answers there: `mgmt/dispatch.rs` refuses
    and counts (`UnknownPeer`) any first handshake that is not a Zode
    handshake. Before zl-zipline#234 an unmarked handshake reached a legacy
    adapter-docking responder, as it still does on a ZIPLINE MSI whose
    service runs `ph node` (the 2.0.x Zode packages before 2.0.6). 2.0.6 ships the `ph zode`-only core
    ([org-zpr/zipline#256](https://github.com/org-zpr/zipline/issues/256)).
  - **Why that is acceptable.** A Zode link survives keying only with a
    certificate signed by the network's CA whose CN is in the Zode's current
    topology, and only from the link's owner (rule 3). Everything else on the
    machine stays closed.
  - **Every profile.** Laptops on public Wi-Fi are the common case.
- **A Zode's own login, visa-service half** — `getLoginNonce` and the
  `reauthorize` gain/drop for a node's own actor are implemented in the visa
  service and unit-tested per binding row
  ([zl-zipline#232](https://github.com/org-zpr/zl-zipline/issues/232)). The
  node and `ph-cli` side is built too
  ([zl-zipline#233](https://github.com/org-zpr/zl-zipline/issues/233)): login,
  logout, silent renewal, logout at expiry, the session-replaced prompt and
  the renewal answer to a policy re-authentication, unit-tested against a
  fake visa service, and end to end in netns by `zode-login-test.sh`. The
  visa service does not re-check live visas when a session replacement drops
  a login ([zl-zipline#265](https://github.com/org-zpr/zl-zipline/issues/265)).

- **Stream binding restricted to the previous hop (gap 7)**
  ([zipline#253](https://github.com/org-zpr/zl-zipline/issues/253)): a node
  accepts a `Stream ID Request` for a visa only on the link F1 would use to
  send to the visa's `Visa.prevHop`, whose peer's topology ZPR address (by
  verified CN) is `prevHop`, or, on a joiner until a `setTopology` names its
  relay, the initial link whose hand-off recorded `prevHop` as the relay's
  address. A visa without `prevHop` (every ingress visa) never accepts one,
  and there is no fallback. So a neighbour cannot pull a flow onto a path it
  is not on. `Visa.prevHop` is in the schema
  ([zipline#243](https://github.com/org-zpr/zl-zipline/issues/243)) and the
  visa service sets it ([zipline#245](https://github.com/org-zpr/zl-zipline/issues/245)).

**Not yet, or verify before relying on it:**

- **A backward clock step can reopen an expired stream.** The fast path
  compares each packet with the Zode's current wall clock and records
  nothing once an entry has expired, so if the clock steps back before
  the sweep frees the visa, its streams forward again until the clock
  passes the expiry once more. Skew between Zodes moves the cutoff, and an
  upstream lagging by more than the 60 s stream-ID grace can still send
  on an ID the downstream Zode has freed and reused
  ([#281](https://github.com/org-zpr/zl-zipline/issues/281)).
- **Clean-up after a visa is removed.** A freed local stream ID is not
  quarantined before reuse
  ([#282](https://github.com/org-zpr/zl-zipline/issues/282)), and the
  destination's DLT entry is not reclaimed
  ([#283](https://github.com/org-zpr/zl-zipline/issues/283)), whether the
  visa expired, was revoked, or lost its link.
- **Attribute lifetimes near their end.** The visa service refreshes an
  attribute only after it has expired, and in the attribute's last 30
  seconds denies every visa request that needs it, unless a refresh of
  the attribute's source intervenes
  ([#275](https://github.com/org-zpr/zl-zipline/issues/275)). A flow that a
  held, unexpired visa already covers is unaffected. This fails closed, but
  quietly: the deny is counted and listed, with `NoReason`, by
  `GET /admin/visas/denies`; only its cause is logged, at DEBUG.
- **A2A confidentiality.** Internal RFC-17 says outright that A2A currently
  protects message integrity only and that encryption could be added later. The
  Case 1 table's payload-encryption row is a design goal, not a current
  guarantee.
- **Anti-replay enforcement.** ZDP headers carry sequence numbers, but no
  receiver-side sliding window or duplicate rejection appears in `zl-zpr-core`;
  searches for anti-replay logic came up empty. Treat replay protection as
  specified but unverified, and confirm before depending on it.
- **Quantitative limits.** Caps on bandwidth, connections, and transferred data
  carry much of the DoS and exfiltration story in the threat model, but ZPL
  conditions and limits do not compile — see [ZPL.md](ZPL.md).
- **`over`-clause path constraints.** Link constraints are recorded in policy
  and routability is checked by the topology manager, but route-aware
  evaluation in `libeval` is a scaffold.
- **Reverse binding for symmetric visas** stays undefined
  ([zipline#246](https://github.com/org-zpr/zl-zipline/issues/246)): the
  previous-hop rule (under *Implemented*) refuses a `Stream ID Request` from a
  visa's next hop.
- **k-of-n administrative concurrence.** No mechanism for it exists in the visa
  service admin API, which authenticates a single API key per request with a
  read or read/write permission.
- **Networked attribute sources** are implemented as of 2026-09-22: the
  `api = "zpr-attr/1"` trusted service queries an external attribute service
  over HTTPS with a bearer token, and `POST /admin/services/{id}/changed`
  gives it push invalidation, so the attribute-expiry and push-invalidation
  machinery is exercised against a live source (netns end-to-end test,
  zipline#81) as well as local JSON. `zpr-attr/1` departs from the RFC-13.1
  per-call HMAC — see ATTRIBUTE_SERVICE.md.
- **Topology-bound link certificates (Zodes).** The verification and the
  CN-in-topology rule are implemented for Zode-to-Zode links
  ([#220](https://github.com/org-zpr/zl-zipline/issues/220); *Design
  decisions*, above), and the visa service fills `Link.peerCn` from the peer's
  node join policy ([#221](https://github.com/org-zpr/zl-zipline/issues/221)).
  Zodes start links from the topology and from their configured initial link
  ([#250](https://github.com/org-zpr/zl-zipline/issues/250)). Forwarding over
  Zode links is implemented and unit-tested
  ([#251](https://github.com/org-zpr/zl-zipline/issues/251), ZODE_LINKS.md
  F1-F8): a lost link revokes its visas locally, and the `down` report goes out
  before any visa request that follows. The relay join is implemented
  ([#252](https://github.com/org-zpr/zl-zipline/issues/252); *Design
  decisions*, above): in netns, `zode-link-test.sh` carries a joiner's VS-API
  flow across its Zode link to the relay. Since
  [#254](https://github.com/org-zpr/zl-zipline/issues/254), flows between two
  Zodes cross Zode links in netns too (`zode-line-test.sh`,
  `zode-reroute-test.sh`), and `zode-wrong-ca-test.sh` shows a Zode whose
  certificate comes from another CA refused in the handshake.
- **ZIPLINE issues link certificates at registration**
  ([org-zpr/zipline#251](https://github.com/org-zpr/zipline/issues/251);
  *Design decisions*, above). `zipline register` writes the Zode's link key,
  its certificate, the CA certificate, its ZPR address and the relay's CN and
  substrate address, and `zipline service-run` starts `ph zode` with them.
  Since [org-zpr/zipline#256](https://github.com/org-zpr/zipline/issues/256),
  ZIPLINE package 2.0.6 pins core `13d27db` (the zl-zipline#234 merge,
  `ph zode` only) and runs `ph zode` on Linux and
  Windows Zodes and in its relay and DNS containers. The real-machine runs
  are in ZODE_LINKS.md, "On real machines".
  The network's generated policy lists every registered Zode in the relay's
  topology and pins its address
  ([org-zpr/zipline#253](https://github.com/org-zpr/zipline/issues/253)).
  Networks provisioned before the change were cut over at the Zodes flag day
  ([org-zpr/zipline#254](https://github.com/org-zpr/zipline/issues/254)): at
  the web server's first start after the upgrade, such a network gets a new
  CA (a stored one is kept) and a new data dir and containers, keeping its
  id, DNS zone, port and policy rows. Its machines, which had no Zode
  identity, were removed with the rules and services pinned to them, and
  register again as Zodes (`zipline register --force`). Every ZIPLINE machine
  is now a Zode with a pinned address and a CA-signed link certificate, which
  the database enforces (NOT NULL).
- **TLS on VS-API and VSS authenticates neither end.** Both are encrypted, but
  neither client verifies the server's certificate and neither presents one.
  Since the core visas carry no A2A keys (*Design decisions*, above), this is
  the only end-to-end protection of a node's control traffic with the visa
  service. ZIPLINE-provisioned per-customer certificates are
  [#259](https://github.com/org-zpr/zl-zipline/issues/259).

---

## Where the enforcement lives

| Guarantee | Enforced in |
|---|---|
| Policy decision | `zl-zpr-visaservice/libeval` |
| Visa issuance and revocation | `zl-zpr-visaservice/vs` — `visareq_worker.rs`, `visa_reconciler.rs`, `policy_mgr.rs` |
| Endpoint admission | `zl-zpr-visaservice/vs/src/connection_control.rs`, `auth.rs` |
| Attribute sourcing and expiry | `zl-zpr-visaservice/vs/src/trusted_services/` |
| A2A integrity | `zl-zpr-core/adapter/ph` — `zdp.rs`, `fastpath.rs`, `adapter_tables.rs` |
| Link crypto and key management | `zl-zpr-core/adapter/ph` — `km_noise.rs`, `km_cert_exchange.rs`, `pki.rs` |
| Per-packet visa enforcement | `zl-zpr-core/libnode2`, and the node side of `ph` in `zl-zpr-core/adapter/ph` |
| Visa expiry on every packet; the expiry sweep | `zl-zpr-core/adapter/ph` — `fastpath.rs` (`forward`), `forwarding_tables.rs` (`PftPep`), `visa_table.rs` (`bind`, `commit_supersede`, `handle_expirations`), `visa_mgmt.rs` |
| What policy can express | `zl-zpr-compiler` — see [ZPL.md](ZPL.md) |

[ZDP.md](ZDP.md), "Flow setup", traces a flow's first packet and is the best
orientation to the data path.

Related: [VISA_SERVICE.md](VISA_SERVICE.md), [ZPL.md](ZPL.md),
[REPOSITORIES.md](REPOSITORIES.md).
