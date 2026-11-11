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

**Scope.** The relaxation applies to `reauthorize` and to nothing else. The
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
provides").

**The SS (bootstrap-signature) arm.** `reauthorize` also accepts a
self-signed blob, so a device authority is re-proved the same way install-driven
re-authentication demands (zipline#120). One `reauthorize` carries at most one
blob per namespace, and the namespace set must equal the live actor's
authorities exactly — an actor that authenticated as RSA+OIDC re-proves both
or fails. The SS arm requires all of: the target is a live actor admitted
through the calling node; the actor holds a bootstrap `device.zpr.authority`;
the blob's CN equals its authenticated CN; the signature verifies against the
**pinned new snapshot's** bootstrap key; and the blob's timestamp is within
the maximum clock skew (180 s). There is no monotonic-timestamp rule:
freshness comes from the node-minted, HMAC'd challenge (at most 120 s old),
and only the docking node could replay — a compromised docking node is
already Case 2.

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
is not built yet (*Implementation status*, below). The node's check is its `forwarding`
setting (`[node] forwarding`, `ph node --forwarding`, default off). With it off,
the node refuses every `forwardOnly` visa. The setting cannot be the only
guard, because a subverted node can turn it on; that is why the visa-service
half is required. ([zipline#219](https://github.com/org-zpr/zl-zipline/issues/219))

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
- **Session-bound renewal** — the `reauthorize` bindings in *Silent
  re-authentication* above are implemented and unit-tested per row
  (zipline#43).
- **Caller-scoped disconnect** — `notifyDisconnect` accepts only the calling
  node itself, adapters docked to it, and AAA actors registered to it; see
  *Silent re-authentication* above for the binding (zipline#182).
- **Non-transit, node half** — a node with `forwarding` off refuses
  `forwardOnly` visas: a pushed one is dropped, logged and acknowledged, and a
  bind that needs one is refused ([zipline#219](https://github.com/org-zpr/zl-zipline/issues/219)). See *Design decisions* above.

**Not yet, or verify before relying on it:**

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
- **Non-transit, visa-service half.** Policy can list the transit-capable
  nodes (`Policy.transitNodes`), but path computation does not read the list
  yet and may route through any node
  ([zipline#227](https://github.com/org-zpr/zl-zipline/issues/227)). Until it
  does, keeping traffic off an endpoint rests on that endpoint's `forwarding`
  setting being off, which a subverted endpoint controls.
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
- **Background renewal does not complete end to end.** Every piece is
  implemented and unit-tested, but the node reaches its renewal deadline with
  no way to reach the user's authentication agent — see the *Not yet* note in
  [OIDC.md](OIDC.md). Until that closes, an OIDC actor is disconnected when
  its authentication window ends, which is the safe direction to fail but not
  the intended behaviour.

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
| Per-packet visa enforcement | `zl-zpr-core/libnode2`, and the dock path in `zl-zpr-core/adapter` |
| What policy can express | `zl-zpr-compiler` — see [ZPL.md](ZPL.md) |

`zl-zpr-core/packet_walk.md` traces a flow end to end and is the best orientation
to the data path.

Related: [VISA_SERVICE.md](VISA_SERVICE.md), [ZPL.md](ZPL.md),
[REPOSITORIES.md](REPOSITORIES.md).
