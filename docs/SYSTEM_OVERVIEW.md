# ZPR System Overview

Zero-trust Packet Routing (ZPR, pronounced "zipper") moves IP packets through a
network that enforces communication policy **itself**, at the IP layer, on
every packet, at every hop — rather than leaving security to endpoints and
firewalls.

This is the orientation document. It explains what the system is, what the
pieces are, and how a packet gets from one application to another. The
companion documents go deeper on each part; the map is at the end.

## Sources

The design comes from the ZPR RFCs, principally internal RFC-1.4
(*Zero-Trust Packet Routing*), published RFC-12 (*Overview*), internal RFC-7
(*Topology, Address Assignment and Forwarding*), internal RFC-9 (*Threat
Resistance*), and internal RFC-10.2 (*ZPR as Software-Defined Networking*).
RFCs 4, 12, 15, 16, and 19 are published in `zl-zpr-rfcs`; the rest are internal
to Applied Invention and cited here by number.

Implementation details are read from the code in the workspace. Where design
and code disagree, the code is what runs — §"Implementation status" collects
the gaps.

---

## The approach

Conventionally a network's real policy is an emergent property of accumulated
firewall rules and whatever the endpoints happen to enforce. ZPR instead states
policy in one auditable place and has the network guarantee it.

Policy is written in [ZPL](ZPL.md) against the *attributes* of the
communicators — **never their addresses, and never their identities**. It is
compiled, installed, and enforced by the network itself, as a layer of security
independent of whatever the endpoints and firewalls already do. Because policy
names attributes rather than addresses, a service can move from one cloud to
another with no policy change.

### Four paranoid design principles

Everything below follows from these (RFC-12 §4.1):

1. **Every packet must have an authenticated sender and receiver.**
2. **The network enforces policy continuously and throughout**, not only at
   entry and exit or at session establishment. Every packet is discarded unless
   policy explicitly allows it.
3. **Nothing is trusted to behave** — not users, not the devices implementing
   the network, not the administrators. Policy must hold even when one of them
   turns out to be unreliable or malicious.
4. **The network is configured and managed through the network itself.**

The fourth is the least obvious and among the most important. Forbidding
out-of-band management, and requiring that management traffic itself be
policy-compliant, is what lets a ZPRnet defend against its own administrators —
for example by requiring three authenticated administrators in three different
facilities to activate a configuration. The only out-of-band step is the
initial startup of a component, when it is preloaded with the cryptographic
material that lets it join.

---

## Core concepts

### ZPRnet

A **ZPRnet** is the network — or group of co-managed interconnected networks —
over which one policy is enforced. It consists of **nodes** connected by
**links**, plus internal services: a **visa service** and an **admin service**.

ZPR runs at the Internet layer, so TCP, UDP, and everything above them work
unmodified. Links may be physical, or virtual over an IP **substrate** — in
which case ZPR is a software-defined overlay network, which is how it is
deployed today.

### Visas

A **visa** is a granted permission for one authenticated endpoint to send to
another under specific conditions. It is the system's central object.

Transit packets between nodes **carry no source or destination address**. They
carry a *visa identifier*, and the visa guides them. That one identifier
simultaneously certifies that both endpoints were authenticated, that policy
permits this communication, and under what conditions — and names the key used
for the packet's integrity check.

Visas are **unidirectional**: the reply needs its own.

Two consequences make the design work:

- **The expensive work happens once.** Translating policy into enforcement
  happens at issuance. Forwarding a packet costs a table lookup on the visa
  identifier plus a check of local conditions — no longest-prefix match, no
  firewall-rule evaluation, no hop-count update.
- **Nodes never receive the policy.** Every node enforces policy without
  holding it. Each is told only what it needs: which link to forward to, which
  links a packet may arrive on, what local conditions to check. Only the
  ingress and egress Zodes get the integrity key. **Only the visa service
  holds the whole visa.**

### Compliant flows

A **compliant flow** is the tamper-resistant channel ZPR establishes for any
policy-compliant communication. Every packet on it is covered by a valid visa,
and the visa's procedures are applied *at every forwarding step* — so
enforcement tracks changing conditions and attributes, unlike a conventional
session that is checked once at setup.

Flows are unidirectional and normally used in pairs. A flow may outlive any one
visa, and may hold several over time or at once. Packets are best-effort:
compliant flows defend against tampering but provide no flow control or
retransmission, so TCP is layered over them as usual.

Policy often chains them. Remote users reaching a web service traverse one flow
to the load balancer and a second from the load balancer to a server — which
avoids needing a distinct visa per user/server pair.

Crucially, **the control plane runs over compliant flows too.** Node-to-visa-service
traffic is not out-of-band; it is policy-compliant traffic on the ZPRnet, which
is principle 4 in practice.

### Configurations

Policy is part of a **configuration**, so changing policy is reconfiguration —
and reconfiguration is where conventional networks open holes.

ZPR changes configuration while running. Packets continue under the
configuration they entered on, so **multiple configurations operate
concurrently** during a changeover: `UNDEFINED → DEFINED → [TESTING] → ACTIVE`,
with the outgoing one moving to `DEACTIVATING` until its in-flight packets
drain. Exactly one configuration is active, and new packets enter on it.

This also lets packet field widths be fixed per configuration rather than in
the protocol — the only header field that cannot vary between configurations is
the one naming the configuration.

---

## Components

| Component | What it does |
|---|---|
| **Zode** | The one ZPR component that carries traffic (`ph zode`). It is a node: it forwards packets and enforces policy on every packet passing through. It also carries its own actor: its host's applications reach the ZPRnet through its TUN interface, which converts between IP packets and transit packets. Each Zode is either a **relay**, which never dials, or dials its relay with its initial link. See [ZODE_LINKS.md](ZODE_LINKS.md). |
| **Link** | A connection between two Zodes; physical, or virtual over an IP substrate. Carries attributes that ZPL `over` clauses can constrain. A Zode accepts only Zode links. |
| **Visa service** | Issues, distributes, and revokes visas; holds policy; admits endpoints. See [VISA_SERVICE.md](VISA_SERVICE.md). |
| **Admin service** | Generates and distributes configurations, including topology and forwarding rules. |
| **Trusted services** | External sources of authentication and attributes — LDAP, Active Directory, cloud services. Named in policy; the only sources ZPR will consult. |

### Node and adapter inside a Zode

The RFCs describe two components: a **node**, which forwards and enforces, and
an **adapter**, which lets unmodified IP applications reach the ZPRnet over a
**docking session** to a node's **dock**. In the implementation they are one
process. A Zode's node side and its actor side (the RFC adapter) talk over an
internal link pair inside `ph`, and there is no adapter process and no docking
session over the network (zl-zipline#234; see *Design decisions*, "One mode:
`ph zode`").

The adapter's trusted role survives as the actor side's: it proves the identity
of its device, and of any user logged in on it, to the ZPRnet. That role
**does not enforce policy and does not know what the policy is**: the visa
service tells the Zode only what it needs to forward and enforce.

---

## Life of a flow

Concretely, from writing policy to delivering a packet.

**1. Author and compile.** An operator writes a `.zpl` policy and a `.zplc`
configuration (TOML: nodes, links, protocols, services, trusted services,
bootstrap keys). `zplc` compiles both together into a signed binary policy
(`.bin2`). The signing key must match the one the visa service is configured
with. See [ZPL.md](ZPL.md).

**2. Bootstrap.** A ZPRnet needs a visa service, a relay Zode, and a trusted
authentication service before ordinary authentication can work. Those come up
on static configuration: the policy's `[bootstrap]` block maps certificate CNs
to RSA public keys, and the holder proves possession by signing a challenge.
The visa service reserves the name `vs.zpr`.

**3. Links come up.** A Zode dials its relay over its initial link, and the two
establish a link with a Noise handshake, exchanging certificates signed by the
deployment's CA. A Zode accepts only a Zode handshake: any other first
handshake message is refused. The relay hosts the visa service itself, on its
own TUN at `fd5a:5052::1` (`vs.zpr`; see *Design decisions*).

**4. An endpoint joins.** A Zode's actor authenticates to the visa service
with the machine's bootstrap key; no identity provider is needed for that.
A person who logs in on the Zode later adds an OIDC token, a separate step
([OIDC.md](OIDC.md)). The visa service authenticates the endpoint via
trusted services, which return tokens carrying policy-specified attributes.
Some are marked *identity attributes*, and the visa service uses them to query
other trusted services and complete the profile. It then asks policy: may this
endpoint communicate at all, may it host services, does it get a static
address, is it explicitly denied? A Zode does not receive its ZPR address
here: it is configured with it before it starts (`--zpr-addr`, the first
address; `ph` refuses to start without one) and uses it as the source of its
VS-API session. The visa service checks the claim against policy (the node's
`zpr_address`) and the static-address rules under *Addressing*, and on
success the Zode joins.

**5. First packet.** An application sends to the Zode's TUN. The Zode's actor
side finds no entry for the 5-tuple in its lookup table and issues a **bind
request**, including the packet, to its own node side over the internal link.

**6. Visa request.** If the Zode has no matching visa it forwards a visa
request to the visa service, packet included. The visa service gathers both
endpoints and their attributes — refreshing expired ones, denying if an
endpoint is unknown or its authentication has expired — and hands the policy,
the endpoints, and a description of the attempt to the evaluator.

**7. Decision.** The evaluator checks *all* deny statements first, then allow
statements, returning every match in policy order. The visa service takes the
first allow match the topology manager can also route. No routable match is a
denial.

**8. Distribution.** The visa goes to the requesting Zode and to each Zode
along the chosen path — each receiving only its own fragment. The ingress Zode
installs it, binds a stream ID on each link along the path hop by hop
(`Stream ID Request`), and answers its actor side's bind request with
instructions for compressing packets on this flow.

**9. Steady state.** Subsequent packets hit the lookup table directly. The
ingress Zode computes the end-to-end (A2A) integrity check over the whole IP
packet, compresses the packet — the fields the visa pins, including addresses
and ports, are removed — and sends it with a ZDP transit header. Each Zode on
the path verifies against the visa and forwards. The egress Zode reconstructs
the original IP packet from the visa's field values, verifies the integrity
check, and delivers it to its TUN.

Compression removes what is constant over a flow, so a transit packet is often
**smaller than the IP packet it carries** — which leaves room for the ZDP
header without pushing past the substrate MTU.

**10. End.** Visas expire on a timestamp every recipient can see, so no
"expired" message is ever sent; Zodes stop forwarding and tear down the
flow's streams, and further traffic looks new. (In `ph` today a Zode checks
expiry only when a new flow matches a visa, with no timer: ZODE_LINKS.md,
"Visa expiry at match time (#271)". Checking it on every use of a visa,
so bound streams stop at expiry too, is
[zl-zipline#274](https://github.com/org-zpr/zl-zipline/issues/274), part of
the Zodes epic.) Revocation is separate — an administrative
action or a policy change — and is pushed to the affected nodes, which stop
immediately.

See [ZDP.md](ZDP.md) for the wire protocol and
[VISA_SERVICE.md](VISA_SERVICE.md) for the control plane.

---

## Data plane and control plane

Both run as internal packets between nodes, and both declare their type and
configuration in the header.

- **Transit packets** are the data plane: a visa identifier, payload length, an
  optional data attribute tag, the payload, and an integrity check computed
  over everything except the visa identifier. The visa identifier is
  **link-local** — it changes as the packet moves from link to link, though the
  visa does not.
- **Management packets** are the control plane: assigning link-local visa
  identifiers, distributing and retracting visas.

## Addressing

ZPR addresses are IPv6 from `fd5a:5052::/32`, assigned by the visa service when
an endpoint joins. In the current implementation:

| Range | Use |
|---|---|
| `fd5a:5052::1` | The visa service |
| `fd5a:5052:90de:1::/64` | Nodes (pool-allocated) |
| `fd5a:5052:adda:1::/64` | Adapters (pool-allocated by the visa service, which keeps its adapter roles; since zl-zipline#234 no `ph` docks an adapter) |
| `fd5a:5052:8888::/64` | Static: trusted-service-granted (de facto range), and ZIPLINE's Zodes (see below) |
| `10.192.0.0/22`, `10.128.0.0/22` | IPv4 equivalents of the node and adapter pools |

An endpoint may hold a static address from the network's own configuration — a
`zpr_address` on a node — or be granted one by a trusted service vending
`device.zpr_addr` (zipline#99); an authored `zpr.addr` pin in policy text is a
compile error (zipline#109). **Static addresses live in
a different address space from the pools the visa service manages**: at join
time the visa service rejects a static address that is inside a managed pool
(so the pool can never double-allocate it), outside `fd5a:5052::/32`, equal to
its own address, or already held by a live actor — the second claimant is
rejected, never silently renumbered. `fd5a:5052:8888::/64` is the de facto
static range: nothing reserves it, but every granted static address in
the tree's fixtures and demos is in it, and every static node address is
outside the node pool. The
substrate addresses that carry virtual links are ordinary IP addresses and
are unrelated to these.

**ZIPLINE gives each Zode a static address in `fd5a:5052:8888::/64`.** When a
registration is confirmed, the ZIPLINE server allocates `fd5a:5052:8888::<n>`,
where `n` is one more than the network's count of addresses handed out so far
(starting at 1, written in the address's low 32 bits). The count only goes up,
and an address is unique within its network, so a deleted Zode's address is
never handed to another machine: DNS answers and open connections elsewhere may
still name it. `zipline register` writes the address into the Zode's
`zipline.toml`, and `ph zode --zpr-addr` claims it (zipline runs `ph node` until
[org-zpr/zipline#256](https://github.com/org-zpr/zipline/issues/256) switches it). The visa service honours
the claim only once the network's policy pins the address to the Zode's CN as
the node's `zpr_address`, and only if it passes the static-address checks
above. ZIPLINE's generated policy writes that pin for every registered Zode
([org-zpr/zipline#253](https://github.com/org-zpr/zipline/issues/253)). Within a
ZIPLINE network the counter is the only allocator in the range: ZIPLINE runs
no trusted service that grants `device.zpr_addr`. A network that added such a
grant inside `fd5a:5052:8888::/64` could collide with a Zode, and the visa
service would reject whichever of the two joined second.
([org-zpr/zipline#251](https://github.com/org-zpr/zipline/issues/251), gate
g113 q5)

RFC-7 describes generating topology, node numbering, and forwarding rules
automatically from connection policy and hardware constraints — deriving the
routing architecture from the policy rather than configuring it by hand. The
current implementation is the direct version of this: links are declared in the
`.zplc` configuration, and the visa service's topology manager maintains the
live graph and selects routes.

---

## What runs where

| Concept | Binary | Repository |
|---|---|---|
| Zode (node and its own actor) | `ph zode` | `zl-zpr-core/adapter/ph` |
| Zode admin client | `ph-cli` | `zl-zpr-core/adapter/cli` |
| Visa service | `vs` | `zl-zpr-visaservice/vs` |
| Policy evaluation | `libeval` (library) | `zl-zpr-visaservice/libeval` |
| Policy compiler | `zplc`, `zpdump` | `zl-zpr-compiler` |
| Admin client / dashboard | `vs-admin`, `zpr-dashboard` | `zl-zpr-visaservice` |
| Policy testing without a network | `zpt` | `zl-zpr-visaservice/zpt` |
| Shared types and wire formats | `zpr` crate | `zl-zpr-common`, with `zl-zpr-policy` and `zl-zpr-vsapi` as submodules |

**`ph zode` is the only mode.** `ph` — the packet handler — has one
subcommand, and every Zode uses a TUN interface for the IP side. (The
`adapter/` directory name is historical.)

A minimal working ZPRnet is: one relay Zode hosting the visa service on its own
TUN, a Valkey/Redis for visa service state, a compiled policy, and at least one
more Zode that dials the relay, for something to talk to.
`zl-zpr-core/docs/SETUP.md` walks through building one, key by key; `zl-zpr-demo` packages containerized, multi-node, and IoT demos.
See [BUILD.md](BUILD.md) and [REPOSITORIES.md](REPOSITORIES.md).

---

## Design decisions

Rationale carried over from completed master plans, retired once shipped. Full
plan text is in git history:

- `git show 44e37d0:docs/plans/2026-09-28-windows.md` — umbrella
  [zipline#126](https://github.com/org-zpr/zl-zipline/issues/126)
- `git show cc02abd:docs/plans/2026-10-01-win-node.md` — umbrella
  [zipline#149](https://github.com/org-zpr/zl-zipline/issues/149)

The Windows and macOS entries below were decided for `ph adapter` and
`ph node`, before zl-zipline#234 made `ph zode` the only mode. Where a
decision is about the platform (Wintun, the `WaitSet`, the named pipe, `netsh`,
DAD, firewall rules, utun addressing) it holds for `ph zode` unchanged; where
it was about the adapter or node role, the entry says what replaced it.

### One mode: `ph zode`

`ph zode` is the only `ph` subcommand
([zl-zipline#234](https://github.com/org-zpr/zl-zipline/issues/234), plan p51,
Zodes master plan §5.11).

- **What was removed.** `ph node`, `ph adapter` and `enum PhMode`; the adapter
  side of a dock (`AdapterToNode`: the tether, actor activation, the RSA
  bootstrap auth of a docked adapter, auto-connect and link auto-restart, the
  `add_route` route-owner check and exit code 7); the node side of a dock
  (`NodeToAdapter`: node-side authorization and renewal of docked actors, ZDP
  packet types 138-145, the visa service's own adapter dock and its "special
  peers"); the `[adapter]` config section and `[node]
  auth_renewal_lead_seconds`; `ph-cli addr` and the `<id>` argument of
  `ph-cli connect` and `auth-agent`. A Zode's node side and its actor side
  still talk over the internal link pair inside the process; that is the
  RFC's Adapter/Node split, kept as a logical one.
- **A Zode accepts only Zode links.** Any first handshake message that is not
  a Zode handshake is refused and counted as `UnknownPeer` (`mgmt/dispatch.rs`).
- **No alias, no backwards compatibility.** `ph node` and `ph adapter` are
  unknown subcommands (clap exits 2), and `[adapter]` and
  `auth_renewal_lead_seconds` are refused as unknown keys, not ignored: a
  stale command line or config fails at once instead of half-working.
- **Exactly one of relay or initial link.** A Zode is a relay (`--relay true`
  or `[node] relay = true`), which never dials, or dials its relay over
  `--initial-link CN@IP:PORT` (`[node.initial_link]`). Both, or neither, is
  refused at startup (`config.rs`): a Zode with neither could reach no visa
  service, and the old one-node layout, where the visa service docked as an
  adapter on a non-relay node, is gone.
- **The CLI contract** (what zipline's service runner, org-zpr/zipline#256,
  consumes). Every flag `ph node` took keeps its name and meaning:
  `--control-path`, `--control-group`, `-a/--self-addr` (port 0 means the
  default link port, 5000), `--ca-file`,
  `--certificate-file`, `-k/--private-key-file`, `-K/--noise-private-key`,
  `-i/--tun-if`, `-z/--zpr-addr` (repeatable: the Zode's own IPv6 address
  first; a relay adds `fd5a:5052::1`, and no other Zode lists it),
  `-l/--logging`, `--io-engine`, `--km-impl`, `-c/--config-file`,
  `--auth-private-key`, `--advertised-substrate-addr`, `--forwarding`,
  `--initial-link`, `--relay`, and the `enable-security-testing` flags. So a
  caller switches by replacing the word `node` with `zode`. `ph-cli counters`,
  `link show [id]` and `tun` keep their output formats, except that
  `link show` no longer prints `Actor state:` or `Last auth failure:` (no
  actor is authorized over a link). **Upgrade `ph` and `ph-cli` together:**
  `adapter/admin-api/cli.capnp` changed the `CmdLineInter` interface ID and
  renumbered its methods, so an older `ph-cli` cannot call a new `ph`, not
  even for `counters` or `tun`.
- **The Zode's own login renews with a fixed 300 s lead**
  (`zode_login::AUTH_RENEWAL_LEAD`). The removed
  `auth_renewal_lead_seconds` had configured both the node-side renewal of
  docked actors and this; no knob replaced it.
- **The Windows visa-service /128 route is gone** (gate g165 q4 left the
  decision to #234). Since zl-zipline#272 every Windows TUN address carries
  its /32, which puts `vs.zpr` on-link as on Linux and macOS, so the extra
  route only covered a hand-configured address with a longer prefix. It went
  with the route-owner check that guarded it. `ph` leaves an address already
  on the TUN as it is, so before upgrading, give such a hand-configured
  address a prefix that covers `vs.zpr` (or add an on-link route to it).
- **AAA is kept, unused** (Zodes master plan §5.4): see
  [ZODE_LINKS.md](ZODE_LINKS.md), *Design decisions*.

### Windows support (decided for `ph adapter`)

`ph adapter` ran on Windows 10/11 (x64, MSVC), and `ph zode` does now. Build and toolchain decisions
(crypto, pcap, capnp) are in [BUILD.md](BUILD.md) "Design decisions"; the
user-facing walkthrough is `zl-zpr-core/docs/SETUP.md` "Windows".

**Adapter first, elevated console, no service.** The first release was
`ph adapter` on an end-user PC run from an Administrator console; the node
role followed under its own umbrella (next subsection). Service wrapper,
installer and Event Log are deferred.
([zipline#126](https://github.com/org-zpr/zl-zipline/issues/126))

**Windows is a third arm of `sys/`, plus one new abstraction.** No unix
behaviour changes. The one abstraction is the wait set (next entry); everything
else is `sys/windows/` or a `#[cfg(windows)]` block next to the existing macOS
ones — the same pattern the macOS port established.
([zipline#126](https://github.com/org-zpr/zl-zipline/issues/126))

**The fastpath waits on a `WaitSet` of `Waitable`s, not on fds.** Unix: a
`Waitable` is a `BorrowedFd` and `WaitSet::wait` is `poll(2)`, byte-identical
to the old behaviour; Windows: a HANDLE and `WaitForMultipleObjects` (TUN =
Wintun's read-wait event, UDP socket = an Event bound with
`WSAEventSelect(FD_READ)`, `Notify` = a manual-reset Event). Landed first as a
unix-only refactor with the netns tier green, so the Windows arm plugged into
a proven seam. Rejected: per-source reader threads plus channels (extra copy
and hop per packet, second datapath shape); a tokio async datapath on Windows
only (forks the datapath, loses `batch_io`).
([zipline#128](https://github.com/org-zpr/zl-zipline/issues/128))

**Wintun via the `wintun` crate; addresses and routes via `netsh`.** The
signed `wintun.dll` ships next to `ph.exe` and is loaded at runtime;
`PI_SIZE = 0`. Address and route changes shell out to
`netsh interface ipv6 ...`, the same pattern as `ip` and `ifconfig`.
(`route_owner_conflict`, which parsed `netsh` route output, went with the
route-owner check in zl-zipline#234.) The known risk is
localized `netsh` output (fixtures are English); the named upgrade path is the
IP Helper API ([zipline#153](https://github.com/org-zpr/zl-zipline/issues/153)).
Wintun adapter lifecycle: create fresh, delete on graceful exit, and reap any
stale Wintun adapter with our name at startup.
([zipline#130](https://github.com/org-zpr/zl-zipline/issues/130))

**Windows batch_io is a `windows_unbatched` engine with pktinfo.**
Non-blocking, one datagram per call: `WSARecvMsg`/`WSASendMsg` with
`IP_PKTINFO`/`IPV6_PKTINFO` on the socket side, Wintun ring send/receive on
the TUN side. Each received datagram carries its destination address, and a
send can name its source, as on the unix engines, so a wildcard `self_addr`
works. (Originally no pktinfo, with multi-homing deferred;
[zipline#150](https://github.com/org-zpr/zl-zipline/issues/150) added it in
zl-zpr-core#119.)
`BorrowedFd` in the `BatchIoImpl` trait became the `Waitable` handle type,
and raw `libc::c_int` send flags a `SendFlags` newtype.
([zipline#131](https://github.com/org-zpr/zl-zipline/issues/131))

**Control RPC over a tokio named pipe with an explicit DACL.**
`\\.\pipe\zpr-control-<sid>` (or the shared `\\.\pipe\zpr-control`),
created with a protected DACL (`D:P`) granting access to
`BUILTIN\Administrators`, the owning user's SID and, optionally, one
configured local group — nothing else; the default named-pipe DACL is not
acceptable for a control channel. Cap'n Proto RPC runs over any
`AsyncRead + AsyncWrite`, so only how the stream is obtained changed. The
owner is the process token's user SID. Paths use `%ProgramData%\zpr`
instead of `/var/run/zpr`.
([zipline#129](https://github.com/org-zpr/zl-zipline/issues/129),
[zipline#130](https://github.com/org-zpr/zl-zipline/issues/130))

**Non-elevated `ph-cli` through a configured local group.** The owner-SID
ACE alone serves only a `ph` and `ph-cli` run by the same user; it rules
out the expected deployment, `ph` as a Windows service (LocalSystem,
session 0) with users running `ph-cli` unelevated — a non-admin has no
matching ACE, and an admin's unelevated token holds `Administrators`
deny-only. So `ph` takes a group name, `control_group` in `[global]` or
`--control-group` (no default; the flag wins), and adds one ACE for it:
`D:P(A;;GA;;;BA)(A;;GA;;;<owner>)(A;;GA;;;<group>)`. Design points:

- The lookup is local-only: `LookupAccountNameW` on `<COMPUTERNAME>\<name>`
  then `BUILTIN\<name>`, never an unqualified name, so a same-named domain
  group cannot stand in; the result must be `SidTypeAlias`/`SidTypeGroup`.
  (The issue sketched `.\<name>`; that prefix is a logon-UI convention that
  `LookupAccountName` does not document, and it would miss `BUILTIN`.)
- The SID in the SDDL comes only from `ConvertSidToStringSidW` on the
  binary SID the lookup returned — never from configured text — and debug
  builds assert its `S-1-[0-9-]` shape before interpolation.
- Configured but missing: today's two-ACE DACL plus one startup warning,
  worded identically on every platform.
- **Packager contract.** The packager owns the group's name and lifecycle on
  all three platforms: it creates the group at install, deletes it at
  uninstall, and its service units (`zipline-svc`, `zipline.service`, the
  launchd plist) pass the name to `ph` (Zipline uses `zipline`;
  [org-zpr/zipline#29](https://github.com/org-zpr/zipline/issues/29)).
  Admission is an administrator adding a user to the group; nothing opens
  by default. Membership is control of `ph`, and a newly added user must
  log on again before their token carries it.
- **Same setting on unix, and a behaviour change there.** The setting
  replaced the hard-coded `zpr` fallback group of the unix socket: an
  ownerless (systemd/launchd) `ph` chgrps the socket to the configured
  group (0660) and, with none configured, no longer looks for `zpr` — a
  host that relied on it must set `control_group = "zpr"`. A resolved
  sudo/pkexec owner still wins on unix.

Rejected: detecting the interactive session user (no such user for a
session-0 service); a `--control-owner` flag (the group covers it); opening
the pipe to `Authenticated Users` or `INTERACTIVE` (hands control of `ph` to
every local user); per-command client identity via impersonation (add it
only if `ph` ever needs to know which member sent a command).
([zipline#154](https://github.com/org-zpr/zl-zipline/issues/154))

**Capture is Unsupported on Windows.** The capture pipe is not created;
`set-capture-file` returns `Unsupported`. The unix fd-passing design exists to
keep `ph` from writing to user-chosen paths as root, and Windows has no cheap
equivalent — passing a path was rejected as a security regression. The
recommended follow-up is `DuplicateHandle`
([zipline#148](https://github.com/org-zpr/zl-zipline/issues/148)). The
internal capture queue also stopped being an AF_UNIX socketpair on every OS —
a pure simplification that removed the last `UnixDatagram` from the datapath.
([zipline#129](https://github.com/org-zpr/zl-zipline/issues/129))

**Signals map onto the Windows console events.** `ctrl_c` and `ctrl_close` →
graceful shutdown (second Ctrl-C hard-exits, as SIGINT does), `ctrl_break` →
print counters (the SIGUSR1 role), `ctrl_shutdown` → graceful.
([zipline#129](https://github.com/org-zpr/zl-zipline/issues/129))

**`zpr-ext` was fixed at the source, not worked around.** Its unix-only
modules (`std::os::fd`, `tokio::net`, `new_unspec`) are gated `cfg(unix)` and
`mtu` gained a Windows arm, tagged `zpr-ext-v0.6.0` with no API change on
unix. ([zipline#127](https://github.com/org-zpr/zl-zipline/issues/127))

**Verification is a Windows CI job plus a hand-run smoke test.**
`windows-latest` builds `ph`/`ph-cli` and runs unit tests (inert while Actions
stays disabled on the forks); end-to-end traffic is verified by hand, today
per `zl-zpr-core/integration-test/windows-zode-test.md` (a Windows Zode
against a relay Zode and the visa service on Linux). A finding worth keeping: `cargo check --target x86_64-pc-windows-msvc`
compiles no test code, so the Windows *tests* had never been built until the
CI work ran them — four test-only fixes resulted. An unsigned `ph.exe` runs
from an elevated console without a SmartScreen block (`wintun.dll` is signed
by WireGuard LLC).
([zipline#132](https://github.com/org-zpr/zl-zipline/issues/132),
[zipline#133](https://github.com/org-zpr/zl-zipline/issues/133))

### A Zode on Windows (decided for `ph node`)

Decisions carried over from the Windows node plan (umbrella
[zipline#149](https://github.com/org-zpr/zl-zipline/issues/149); full text
`git show cc02abd:docs/plans/2026-10-01-win-node.md`). The user-facing
walkthrough is `zl-zpr-core/docs/SETUP.md` "Windows".

**A dev/demo node: elevated console, single worker.** The visa service stays
on Linux. The plan made a Windows node single-homed, with a
concrete `self_addr` and the wildcard bind refused.
[zipline#150](https://github.com/org-zpr/zl-zipline/issues/150) (done,
zl-zpr-core#119) lifted that: the engine uses `WSARecvMsg`/`WSASendMsg` with
pktinfo, and a wildcard `self_addr` is allowed. A relay with a wildcard
`self_addr` still needs `advertised_substrate_addr`. A Zode with an
initial link needs neither: it reports its initial link's local address
([ZODE_LINKS.md](ZODE_LINKS.md), "Network change (#230)"). No service wrapper in `ph` itself
([zipline#147](https://github.com/org-zpr/zl-zipline/issues/147); ZIPLINE's
MSI runs `ph` under its own service, still as `ph node` until [org-zpr/zipline#256](https://github.com/org-zpr/zipline/issues/256); a
`ph zode`-only `ph.exe` dropped into an older MSI install does not start).
Functional parity with Linux is the bar; throughput through one unbatched
socket was measured and recorded, not fixed (findings below).
([zipline#149](https://github.com/org-zpr/zl-zipline/issues/149))

**Nothing may assume `ph` is the only instance on the host.** The link
listener works with a loopback `self_addr`, `tun_if` stays per-instance, and no
change may add a host-wide singleton. (The node-plus-adapter phase this was
written for is superseded: a Zode is both in one process.) The one known
pre-existing collision — the control path is derived from the owner uid alone
(`admin_api::control_socket_path`), so two Zodes for one user collide on
every platform unless `--control-path` is given — is deferred as
[zipline#169](https://github.com/org-zpr/zl-zipline/issues/169).
([zipline#149](https://github.com/org-zpr/zl-zipline/issues/149))

**The node applies its own ZPR address through `TunCtl`, on every platform.**
Before the `local_zpr_addrs_missing_from_tun` check, the node adds each
missing local ZPR address, whose /32 makes the visa-service address
reachable through the TUN. (Windows also installed an explicit /128 route
until zl-zipline#234, see the next entry.) Idempotent, so hand-configured setups and the
integration scripts' out-of-band `ip addr add` are untouched; a Windows-only
arm was rejected to keep macOS from redoing it.
([zipline#159](https://github.com/org-zpr/zl-zipline/issues/159))

**Windows applies the prefix length of a TUN address.** Until
[zl-zipline#272](https://github.com/org-zpr/zl-zipline/issues/272) the
Windows `add_address` dropped `prefix_len` and ran `netsh interface ipv6 add
address <if> <addr>`, and netsh applied its default /64. The /128
visa-service route above was added on the false premise that netsh takes no
prefix, and nothing else outside the address's own /64 was on-link. On real
machines this meant a Windows Zode could not reach the DNS Zode
`fd5a:5052:90de::2`, so no name resolved (org-zpr/zipline#255).
- **The fix.** `add_address` and `add_authentication_address` now pass
  `<addr>/<len>` (`ZPRNET_PREFIX_LEN`, 32), and Windows installs the on-link
  `fd5a:5052::/32` route itself, as Linux and macOS do. One fix in
  `add_address_impl` covers every caller: node/Zode self-addressing, the
  granted actor address and the AAA. Unit tests and Windows CI cover the
  argv; the first real-host check is the #271 retest, with Windows as the
  client by DNS name.
- **One argv builder.** The netsh argv comes from
  `sys/addr_hint.rs::netsh_add_address_args`, which is compiled on every OS
  so its tests run on Linux. The manual-fix hint, which had the same bug, is
  built from the same function, so it prints exactly what `ph` runs.
- **Rejected:** keeping the /64 and adding an on-link /32 route in the node
  path. That would patch one caller and leave every address, and the hint,
  wrong.
- **The /128 visa-service route** was kept by #272 (gate g165 q4) and
  deleted by zl-zipline#234 (see "One mode: `ph zode`").

**The Windows substrate socket tolerates connection reset.** At bind,
`SIO_UDP_CONNRESET` is disabled via `WSAIoctl`; belt and braces,
`windows_unbatched::recv_from_batch` treats `ConnectionReset` as "no
datagram". A Zode talks to many peers, any of which can exit, so the
`WSAECONNRESET` after an ICMP port unreachable would otherwise panic the
fastpath routinely. (This change also made a wildcard bind a logged startup
error; zipline#150 later allowed the wildcard.)
([zipline#160](https://github.com/org-zpr/zl-zipline/issues/160))

**Verification is a hand-run test document plus unit tests; no new CI tier.**
`zl-zpr-core/integration-test/windows-zode-test.md`: the Windows VM is a
Zode (`zb`) that dials a relay Zode on the Linux host, which also runs valkey
and the visa service. (The node-era documents, `windows-node-test.md` and
`windows-adapter-test.md`, were replaced by it in zl-zipline#234.) Automation remains
[zipline#152](https://github.com/org-zpr/zl-zipline/issues/152).
([zipline#162](https://github.com/org-zpr/zl-zipline/issues/162))

Lasting findings from the first end-to-end run (2026-10-01, the test
document's "Findings" has the full table):

**A freshly `netsh`-added address is Tentative, and a bind on it fails.**
Duplicate-address detection made the VSS listener bind fail with 10049
(`AddrNotAvailable`) on every start. The Windows `add_address` now waits,
bounded at 5 s, for the address to leave the tentative state, reading DAD
state via IP Helper `GetUnicastIpAddressEntry` — not `netsh`, whose text
output is localized; the wait loop is `sys/dad.rs`, unit-tested on Linux.
([zipline#162](https://github.com/org-zpr/zl-zipline/issues/162))

**Two inbound firewall rules.** UDP 5000 (link listener, LAN interface) is
required on a Zode that accepts inbound links (a relay, or a Zode a direct
link is dialed to) — without it every inbound link hits a handshake timeout.
A Zode that only dials its initial link does not need it. TCP 8183 (VSS
listener) is scoped to the Wintun interface, which Windows places on the
Public profile where unsolicited inbound is dropped; the rule cannot be
added before the node has created the adapter, and persists once created.
([zipline#162](https://github.com/org-zpr/zl-zipline/issues/162))
A ZIPLINE machine uses a different pair, which the MSI installs:
- "Zipline (ZPR network)", inbound to `fd5a:5052::/32`. It is scoped by
  destination prefix rather than by interface, so it can exist before the
  adapter does;
- "Zipline (direct links)", `ph.exe` on UDP 5000 on every profile, which a
  Windows Zode needs to accept a direct link
  ([zipline#268](https://github.com/org-zpr/zipline/issues/268)).

**Measured single-worker throughput.** Through one unbatched substrate
socket with two client adapters docked (measured on `ph node`): flood-ping loss 0 % (500 packets,
`-i 0.02`), HTTP download 6,751,114 bytes/s (64 MiB in 9.94 s). Recorded,
not tuned.
([zipline#162](https://github.com/org-zpr/zl-zipline/issues/162))

### A Zode on macOS (decided for `ph node`)

Decisions carried over from the macOS node plan (umbrella
[zipline#158](https://github.com/org-zpr/zl-zipline/issues/158); full text
`git show 7323267:docs/plans/2026-10-01-mac-node.md`). The user-facing
walkthrough is `zl-zpr-core/docs/SETUP.md` "macOS".

**A dev/demo node: `sudo`, single worker, visa service on Linux.** Apple
Silicon and Intel, no OS floor beyond what the earlier macOS adapter port
needed. Wildcard and multi-homed binds are allowed — the unix
`posix_unbatched` engine has pktinfo (the Windows node, at first, did not;
zipline#150) — so the Linux integration topology applies unchanged. Functional parity is the bar;
single-worker throughput was measured and recorded, not fixed (findings
below). No launchd service.
([zipline#158](https://github.com/org-zpr/zl-zipline/issues/158))

**Nothing may assume `ph` is the only instance on the host.** Same
constraint as on Windows: a loopback link listener must work, `tun_if`
stays per-instance, no new host-wide singleton. The uid-derived control-path
collision is the same pre-existing cross-platform issue, deferred as
[zipline#169](https://github.com/org-zpr/zl-zipline/issues/169); the macOS
plan only inherited the constraint.
([zipline#158](https://github.com/org-zpr/zl-zipline/issues/158))

**The node's self-address comes from the portable `TunCtl` work (the
Windows plan's C1, merged as
[zl-zpr-core#61](https://github.com/org-zpr/zl-zpr-core/pull/61)); the
macOS plan did not duplicate it.** The first end-to-end run confirmed the
helper needs no macOS-specific tweak: `ifconfig <utun> inet6 <addr>/32
alias` suffices on its own (see the route-shape finding below).
([zipline#159](https://github.com/org-zpr/zl-zipline/issues/159))

**macOS `new_mq` no longer requires an address; the address picks nothing
but the family.** The "address is required on macos" check and its `TODO:
Temporary` are gone: `new_mq` builds the utun as IPv6 when the address is
`None`, otherwise from the address's family, and never applies the address
(it never did — the builder's `with_address` was dead code). On every
platform the TUN is created unaddressed and addressed afterwards through
`TunCtl`. The manual-fix hint `ph` prints when a local ZPR address is
missing from the TUN is now a per-platform string from `sys/` (`ip -6 addr
add` / `ifconfig … inet6 … alias` / `netsh interface ipv6 add address`,
with the prefix length since zl-zipline#272).
Rejected: applying the address inside `new_mq` via the dead builder path —
two ways to address a TUN, and the startup-check path would differ from
Linux and Windows.
([zipline#161](https://github.com/org-zpr/zl-zipline/issues/161))

**Verification is a hand-run test document plus unit tests; no new CI
tier.** `zl-zpr-core/integration-test/macos-zode-test.md`: the Mac is a
Zode (`zb`) that dials a relay Zode on the Linux host, which also runs valkey
and the visa service (it replaced `macos-node-test.md` in zl-zipline#234). An automated
macOS tier would need a macOS runner with root and a routable host — same
shape as [zipline#152](https://github.com/org-zpr/zl-zipline/issues/152),
no issue filed.
([zipline#164](https://github.com/org-zpr/zl-zipline/issues/164))

Lasting findings from the first end-to-end run (2026-10-02, macOS 26.5.2 /
Apple M2, on `ph node`; the node-era test document's "Findings" had the
full table):

**The `/32 alias` alone installs the on-link route.** After `ifconfig utun9
inet6 fd5a:5052::2/32 alias` the kernel installs `fd5a:5052::/32 … Uc utun9`
on-link (plus the self-address `UHL` via `lo0`) — no explicit route is
needed to reach the visa service, and the /32 is harmless for a Zode since
everything else is forwarded by the node itself. No change to the
zipline#159 helper.
([zipline#164](https://github.com/org-zpr/zl-zipline/issues/164))

**Wildcard `self_addr` with pktinfo was verified through a single Mac
address only.** All docks completed and stayed up with `--self-addr
0.0.0.0:5000`, but the remote-node test environment (deleted with the
adapter-docked tests in zl-zipline#234) pointed every adapter at one
`NODE_LAN_IP` and advertised a single substrate address, so the
two-interface case is unverified
([zipline#172](https://github.com/org-zpr/zl-zipline/issues/172)).
([zipline#164](https://github.com/org-zpr/zl-zipline/issues/164))

**The macOS application firewall was not exercised: it was disabled on the
test Mac.** `socketfilterfw --getglobalstate` reported `State = 0` (left off
by choice), so the run observed neither a prompt nor a block on the inbound
link (UDP 5000) and VSS (TCP 8183) listeners, and no `socketfilterfw` rules
were needed, added, or validated. On a firewall-enabled Mac the knob is
`/usr/libexec/ApplicationFirewall/socketfilterfw`, with rules keyed on the
executable's path — allow the copy `ph` actually runs from, not the
build-tree binary; the add-nothing-first procedure and expected command set
are in `zl-zpr-core/integration-test/macos-zode-test.md` ("Prerequisites"),
unvalidated until someone runs it with the firewall on.
([zipline#164](https://github.com/org-zpr/zl-zipline/issues/164))

**Links that never completed the noise handshake stayed in `Keying`
indefinitely** under `ph node`. Mis-NATed docks from before the NAT rules were
up kept re-timing-out the handshake for the whole run and were removed only at
shutdown ([zipline#173](https://github.com/org-zpr/zl-zipline/issues/173),
closed). The adapter dock is gone, and a Zode link's keying is bounded by
`ZODE_KEYING_TIMEOUT = 2 s` ([ZDP.md](ZDP.md)).
([zipline#164](https://github.com/org-zpr/zl-zipline/issues/164))

**Graceful shutdown is clean.** Ctrl-C revokes visas, resets links, exits on
its own, and the utun — being process-scoped — takes its address and route
with it; a second start with the same command line comes up without a stale
route or interface. Measured single-worker numbers: flood-ping loss 0 %
(500 packets, `-i 0.02`), HTTP download 8,432,600 bytes/s (64 MiB), over
Wi-Fi.
([zipline#164](https://github.com/org-zpr/zl-zipline/issues/164))

**A network change is handled by the rejoin.** A Zode on a Mac (or any OS)
with a wildcard `self_addr`, an initial link, no `advertised_substrate_addr`
and no substrate address in policy survives a change of its address (an
advertised address wins, and the visa service refuses a report that differs
from a policy address). The keep-alive closes the initial link, the redial
goes out from the new address, and the rejoin's `connect` or `open` reports that address
([ZODE_LINKS.md](ZODE_LINKS.md), "Network change (#230)"). Sends from the old
address fail with `EADDRNOTAVAIL` on macOS (measured on 26.5.2), and are
dropped and counted, not fatal. Verified by hand on 2026-11-14:
`macos-node-test.md` section 6 (now section 2 of `macos-zode-test.md`), where such a Zode starts, joins through a
relay on Linux and reports its real LAN address. The network switch itself
was checked in netns on Linux, not on the Mac.
([zl-zipline#230](https://github.com/org-zpr/zl-zipline/issues/230);
moving the Mac's address by hand is
[zl-zipline#263](https://github.com/org-zpr/zl-zipline/issues/263))

### The relay Zode hosts the visa service on one TUN

The relay's `ph zode` carries both its own ZPR address and `vs.zpr` on one TUN,
and the visa service runs in the same network namespace, as a local actor of
the relay. There is no visa-service adapter and no second namespace. The
earlier layout kept the visa service in its own namespace, docked as an
adapter, partly so that loopback between the relay and `vs.zpr` could not
happen (ZIPLINE's M4 note). That reason is gone: loopback between the relay and
`vs.zpr` is now the intended in-host path for the relay's own sessions with
the visa service, and every client on the host binds its source address. The
details, the startup order and the decisions of gate g117 are in
[ZODE_LINKS.md](ZODE_LINKS.md), "The relay hosts the visa service (#229)" and
its *Design decisions*; the trust consequence is in
[SECURITY_MODEL.md](SECURITY_MODEL.md).
([zl-zipline#229](https://github.com/org-zpr/zl-zipline/issues/229))

On ZIPLINE each network is now two containers ([org-zpr/zipline#252](https://github.com/org-zpr/zipline/issues/252)):
`zipline-relay-<id>` (the relay Zode, valkey and the visa service) and
`zipline-dns-<id>` (CoreDNS behind the `dns.zpr` Zode, linked to the relay).
The third, separate visa-service container and its `vs.zpr` adapter are gone.
Every ZIPLINE machine is a Zode linked to its network's relay; there are no
ZIPLINE adapters. Networks and machines from before Zodes were cut over at the
flag day ([org-zpr/zipline#254](https://github.com/org-zpr/zipline/issues/254)):
the web server re-provisioned each such network in place, and its machines
registered again as Zodes.

### Deferred

Out-of-scope items from the Windows and macOS adapter and node plans, and
from zl-zipline#234, each with an open issue unless noted:

- Windows service and installer —
  [zipline#147](https://github.com/org-zpr/zl-zipline/issues/147)
- Packet capture to a file via `DuplicateHandle` —
  [zipline#148](https://github.com/org-zpr/zl-zipline/issues/148)
- Npcap SDK for `ph-cli` filter compilation —
  [zipline#151](https://github.com/org-zpr/zl-zipline/issues/151)
- A Windows integration test tier —
  [zipline#152](https://github.com/org-zpr/zl-zipline/issues/152)
- IP Helper API instead of `netsh` —
  [zipline#153](https://github.com/org-zpr/zl-zipline/issues/153)
- Per-instance control path (two Zodes for one user on one host) —
  [zipline#169](https://github.com/org-zpr/zl-zipline/issues/169)
- Two-interface pktinfo verification on a real host (the remote-node test
  env it named is gone) —
  [zipline#172](https://github.com/org-zpr/zl-zipline/issues/172)
- `ph zode` command-line options do not override the config file for most
  `[global]` keys, although `--help` says they do: `set_from_common` (the
  CLI) runs before `set_from_globals` (the file), and `--zpr-addr` is
  prepended to the file's list instead of replacing it —
  [zl-zipline#278](https://github.com/org-zpr/zl-zipline/issues/278)
  (`zl-zpr-core/docs/SETUP.md` describes the real behaviour; ZIPLINE passes
  everything on the command line)
- launchd service / installer for macOS — no issue filed; file if asked
- An automated macOS integration test tier — no issue filed; same shape as
  [zipline#152](https://github.com/org-zpr/zl-zipline/issues/152)

---

## Implementation status

`zl-zpr-core` is a **pre-release reference implementation**; its README says the
full suite of end-to-end security features is not yet implemented. Read the
RFCs as design intent, not as a description of current behavior.

Working today: Zodes (`ph zode`, the only mode since zl-zipline#234) over
Noise-secured Zode links, end-to-end (A2A) integrity, bootstrap authentication, visa issuance and distribution with route
selection, revocation and reconciliation on policy change, the ZPL compiler,
and file-backed attribute sources.

Designed but not yet implemented:

- **Conditions, circumstances, and limits in ZPL** — the caps on bandwidth,
  connections, and transferred data that carry much of the denial-of-service
  and exfiltration argument in the threat model. See [ZPL.md](ZPL.md).
- **Assertions.** RFC-12 describes assertions of intent, checked at compile
  time against the combined permissions and rechecked as attribute values
  change. What exists today is the `never` statement.
- **Route-aware evaluation.** ZPL `over` clauses are recorded and routability
  is checked by the topology manager, but stage 2 of the evaluator is a
  scaffold.
- **End-to-end (A2A) confidentiality.** Integrity only, today.
- **Multiple concurrent configurations.** The changeover state machine
  (`TESTING`, `DEACTIVATING`) is design; the visa service loads one policy at a
  time and can *test* a proposed policy against live visas.
- **Byzantine or replicated internal services.** One visa service instance,
  enforced by a database lock. Multicast and combining flows likewise remain
  design.
- **Automatic topology generation** from connection policy, per RFC-7.

[SECURITY_MODEL.md](SECURITY_MODEL.md) has the full status of the security
properties specifically, including anti-replay and k-of-n administration.

---

## Where to go next

| Document | Covers |
|---|---|
| [TERMINOLOGY.md](TERMINOLOGY.md) | The glossary. Start here if a word above was unfamiliar. |
| [ZPL.md](ZPL.md) | The policy language and the compiler toolchain. |
| [VISA_SERVICE.md](VISA_SERVICE.md) | The control plane: issuance, revocation, attributes, admin API. |
| [ZDP.md](ZDP.md) | The data protocol: packet formats and links. |
| [ZODE_LINKS.md](ZODE_LINKS.md) | Zodes: who dials, link lifecycle, joining through the relay, forwarding. |
| [SECURITY_MODEL.md](SECURITY_MODEL.md) | Threat model, trust boundaries, what ZPR does not defend against. |
| [REPOSITORIES.md](REPOSITORIES.md) | Which repository implements what. |
| [BUILD.md](BUILD.md) | Building and testing everything. |

For the data path, the PlantUML diagrams in `zl-zpr-core/diagrams/`
(`fastpath-overview`, `substrate_ingress`, `substrate_egress`) are the best
next read.
