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
  ingress and egress adapters get the integrity key. **Only the visa service
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
| **Node** | Forwards packets and enforces policy on every packet passing through. Nodes connect to each other over links. |
| **Dock** | The interface on a node that endpoints connect to, speaking IP. Converts between IP packets and transit packets. |
| **Adapter** | Lets unmodified IP applications reach a ZPRnet. Presents a normal IP interface locally and connects to a dock over a secure **docking session**. |
| **Link** | A connection between two nodes; physical, or virtual over an IP substrate. Carries attributes that ZPL `over` clauses can constrain. |
| **Visa service** | Issues, distributes, and revokes visas; holds policy; admits endpoints. See [VISA_SERVICE.md](VISA_SERVICE.md). |
| **Admin service** | Generates and distributes configurations, including topology and forwarding rules. |
| **Trusted services** | External sources of authentication and attributes — LDAP, Active Directory, cloud services. Named in policy; the only sources ZPR will consult. |

### What an adapter is and is not

An adapter proves the identity of its device — and of any user or service
associated with a flow — to the ZPRnet. That is the whole of its trusted role.

It **does not enforce policy and does not know what the policy is.** It has no
knowledge of network configuration, traffic, connection status, or the policy
governing anyone else. It cannot even see the policy for its own users, beyond
observing which communications succeed. It is trusted only by the specific
users and services that use it.

---

## Life of a flow

Concretely, from writing policy to delivering a packet.

**1. Author and compile.** An operator writes a `.zpl` policy and a `.zplc`
configuration (TOML: nodes, links, protocols, services, trusted services,
bootstrap keys). `zplc` compiles both together into a signed binary policy
(`.bin2`). The signing key must match the one the visa service is configured
with. See [ZPL.md](ZPL.md).

**2. Bootstrap.** A ZPRnet needs a visa service, a docking node, and a trusted
authentication service before ordinary authentication can work. Those come up
on static configuration: the policy's `[bootstrap]` block maps certificate CNs
to RSA public keys, and the holder proves possession by signing a challenge.
The visa service reserves the name `vs.zpr`.

**3. Links come up.** An adapter and a node establish a link with a Noise
handshake, exchanging certificates signed by the deployment's CA. A node
recognizes the visa service's adapter only when it presents a CA-verified
certificate claiming `vs.zpr` — without that, the visa service connects as an
ordinary adapter and nothing routes to it.

**4. An endpoint joins.** The visa service authenticates the endpoint via
trusted services, which return tokens carrying policy-specified attributes.
Some are marked *identity attributes*, and the visa service uses them to query
other trusted services and complete the profile. It then asks policy: may this
endpoint communicate at all, may it host services, does it get a static
address, is it explicitly denied? On success the endpoint receives a ZPR
address and joins.

**5. First packet.** An application sends to the adapter, which finds no entry
for the 5-tuple in its lookup table and issues a **bind request** to its dock,
including the packet.

**6. Visa request.** If the dock has no matching visa it forwards a visa
request to the visa service, packet included. The visa service gathers both
endpoints and their attributes — refreshing expired ones, denying if an
endpoint is unknown or its authentication has expired — and hands the policy,
the endpoints, and a description of the attempt to the evaluator.

**7. Decision.** The evaluator checks *all* deny statements first, then allow
statements, returning every match in policy order. The visa service takes the
first allow match the topology manager can also route. No routable match is a
denial.

**8. Distribution.** The visa goes to the requesting dock and to each node
along the chosen path — each receiving only its own fragment. The dock installs
it, allocates a tether ID, and answers the adapter's bind request with
instructions for compressing packets on this flow.

**9. Steady state.** Subsequent packets hit the adapter's lookup table
directly. The adapter computes the adapter-to-adapter integrity check over the
whole IP packet, compresses the packet — the fields the visa pins, including
addresses and ports, are removed — and sends it with a ZDP transit header. Each
node verifies against the visa and forwards. The egress adapter reconstructs
the original IP packet from the visa's field values, verifies the integrity
check, and delivers it.

Compression removes what is constant over a flow, so a transit packet is often
**smaller than the IP packet it carries** — which leaves room for the ZDP
header without pushing past the substrate MTU.

**10. End.** Visas expire on a timestamp every recipient can see, so no
"expired" message is ever sent; nodes stop forwarding and tear down tethers,
and further traffic looks new. Revocation is separate — an administrative
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
| `fd5a:5052:adda:1::/64` | Adapters (pool-allocated) |
| `fd5a:5052:8888::/64` | Static, trusted-service-granted (de facto range; see below) |
| `10.192.0.0/22`, `10.128.0.0/22` | IPv4 equivalents for nodes and adapters |

An endpoint may hold a static address from the network's own configuration — a
`zpr_address` on a node — or be granted one by a trusted service vending
`device.zpr_addr` (zipline#99); an authored `zpr.addr` pin in policy text is a
compile error (zipline#109). **Static addresses live in
a different address space from the pools the visa service manages**: at join
time the visa service rejects a static address that is inside a managed pool
(so the pool can never double-allocate it), outside `fd5a:5052::/32`, equal to
its own address, or already held by a live actor — the second claimant is
rejected, never silently renumbered. `fd5a:5052:8888::/64` is the de facto
static range: nothing reserves it, but every granted static adapter address in
the tree's fixtures and demos is in it, and every static node address is
outside the node pool. The
substrate addresses that carry virtual links are ordinary IP addresses and
are unrelated to these.

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
| Node | `ph node` | `zl-zpr-core/adapter/ph` |
| Adapter | `ph adapter` | `zl-zpr-core/adapter/ph` |
| Visa service | `vs` | `zl-zpr-visaservice/vs` |
| Policy evaluation | `libeval` (library) | `zl-zpr-visaservice/libeval` |
| Policy compiler | `zplc`, `zpdump` | `zl-zpr-compiler` |
| Admin client / dashboard | `vs-admin`, `zpr-dashboard` | `zl-zpr-visaservice` |
| Policy testing without a network | `zpt` | `zl-zpr-visaservice/zpt` |
| Shared types and wire formats | `zpr` crate | `zl-zpr-common`, with `zl-zpr-policy` and `zl-zpr-vsapi` as submodules |

**The node and the adapter are the same binary.** `ph` — the packet handler —
runs as either depending on its subcommand. Both use a TUN interface for the IP
side.

A minimal working ZPRnet is: one node, one visa service (with its own adapter),
a Valkey/Redis for visa service state, a compiled policy, and at least one more
adapter for something to talk to. `zl-zpr-core/README.md` walks through building
one, key by key; `zl-zpr-demo` packages containerized, multi-node, and IoT demos.
See [BUILD.md](BUILD.md) and [REPOSITORIES.md](REPOSITORIES.md).

---

## Design decisions

Rationale carried over from completed master plans, retired once shipped. Full
plan text is in git history:

- `git show 44e37d0:docs/plans/2026-09-28-windows.md` — umbrella
  [zipline#126](https://github.com/org-zpr/zl-zipline/issues/126)
- `git show cc02abd:docs/plans/2026-10-01-win-node.md` — umbrella
  [zipline#149](https://github.com/org-zpr/zl-zipline/issues/149)

### Windows support for `ph adapter`

`ph adapter` runs on Windows 10/11 (x64, MSVC). Build and toolchain decisions
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
`netsh interface ipv6 ...`, the same pattern as `ip` and `ifconfig`;
`route_owner_conflict` parses `netsh` output in a `Platform::Windows` arm of
the portable route parser so its tests run on Linux. The known risk is
localized `netsh` output (fixtures are English); the named upgrade path is the
IP Helper API ([zipline#153](https://github.com/org-zpr/zl-zipline/issues/153)).
Adapter lifecycle: create fresh, delete on graceful exit, and reap any stale
adapter with our name at startup.
([zipline#130](https://github.com/org-zpr/zl-zipline/issues/130))

**Windows batch_io is a `windows_unbatched` engine with no pktinfo.**
Non-blocking `recv_from`/`send_to` on the socket side, Wintun ring
send/receive on the TUN side. No `WSARecvMsg`/`IP_PKTINFO`: an end-user
adapter is single-homed for our purposes and `self_addr` is configured or
discovered once at start — the ceiling is stated in code, and multi-homing is
deferred ([zipline#150](https://github.com/org-zpr/zl-zipline/issues/150)).
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
  by default. Membership is adapter control, and a newly added user must
  log on again before their token carries it.
- **Same setting on unix, and a behaviour change there.** The setting
  replaced the hard-coded `zpr` fallback group of the unix socket: an
  ownerless (systemd/launchd) `ph` chgrps the socket to the configured
  group (0660) and, with none configured, no longer looks for `zpr` — a
  host that relied on it must set `control_group = "zpr"`. A resolved
  sudo/pkexec owner still wins on unix.

Rejected: detecting the interactive session user (no such user for a
session-0 service); a `--control-owner` flag (the group covers it); opening
the pipe to `Authenticated Users` or `INTERACTIVE` (hands adapter control to
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
stays disabled on the forks); end-to-end traffic is verified by hand per
`zl-zpr-core/integration-test/windows-adapter-test.md` against a Linux node and visa
service. A finding worth keeping: `cargo check --target x86_64-pc-windows-msvc`
compiles no test code, so the Windows *tests* had never been built until the
CI work ran them — four test-only fixes resulted. An unsigned `ph.exe` runs
from an elevated console without a SmartScreen block (`wintun.dll` is signed
by WireGuard LLC).
([zipline#132](https://github.com/org-zpr/zl-zipline/issues/132),
[zipline#133](https://github.com/org-zpr/zl-zipline/issues/133))

### `ph node` on Windows

Decisions carried over from the Windows node plan (umbrella
[zipline#149](https://github.com/org-zpr/zl-zipline/issues/149); full text
`git show cc02abd:docs/plans/2026-10-01-win-node.md`). The user-facing
walkthrough is `zl-zpr-core/docs/SETUP.md` "Windows".

**A dev/demo node: single-homed, concrete `self_addr`, elevated console,
single worker.** Adapters and the visa service stay on Linux. A Windows node
must be given a concrete `self_addr` (or `advertised_substrate_addr`); the
wildcard-bind rejection stands by design and
[zipline#150](https://github.com/org-zpr/zl-zipline/issues/150) keeps
multi-homing. No service wrapper
([zipline#147](https://github.com/org-zpr/zl-zipline/issues/147)).
Functional parity with Linux is the bar; throughput through one unbatched
socket was measured and recorded, not fixed (findings below).
([zipline#149](https://github.com/org-zpr/zl-zipline/issues/149))

**Nothing may assume `ph` is the only instance on the host.** A later phase
runs a node and an adapter on one Windows machine (peer-to-peer), so the dock
listen works with a loopback `self_addr`, `tun_if` stays per-instance, and no
change may add a host-wide singleton. The one known pre-existing collision —
the control path is derived from the owner uid alone (`config.rs:356`,
`admin_api::control_socket_path`), so a node and an adapter for one user
collide on every platform unless `--control-path` is given — is deferred as
[zipline#169](https://github.com/org-zpr/zl-zipline/issues/169).
([zipline#149](https://github.com/org-zpr/zl-zipline/issues/149))

**The node applies its own ZPR address through `TunCtl`, on every platform.**
Before the `local_zpr_addrs_missing_from_tun` check, the node adds each
missing local ZPR address and makes the visa-service address reachable
through the TUN (on Windows an explicit /128 route, since `netsh add address`
ignores the prefix). Idempotent, so hand-configured setups and the
integration scripts' out-of-band `ip addr add` are untouched; a Windows-only
arm was rejected to keep macOS from redoing it.
([zipline#159](https://github.com/org-zpr/zl-zipline/issues/159))

**The Windows substrate socket tolerates connection reset.** At bind,
`SIO_UDP_CONNRESET` is disabled via `WSAIoctl`; belt and braces,
`windows_unbatched::recv_from_batch` treats `ConnectionReset` as "no
datagram". A node talks to many adapters, any of which can exit, so the
`WSAECONNRESET` after an ICMP port unreachable would otherwise panic the
fastpath routinely. A wildcard bind now fails as a logged startup error
naming the fix instead of a panic.
([zipline#160](https://github.com/org-zpr/zl-zipline/issues/160))

**Verification is a hand-run test document plus unit tests; no new CI tier.**
`zl-zpr-core/integration-test/windows-node-test.md` mirrors
`windows-adapter-test.md` with the roles inverted: the Windows VM is the
node; the Linux host runs valkey, the visa service and its adapter, and two
client adapters in netns. Automation remains
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

**Two inbound firewall rules.** UDP 5000 (dock listener, LAN interface) is
required — without it every dock hits a handshake timeout. TCP 8183 (VSS
listener) is scoped to the Wintun interface, which Windows places on the
Public profile where unsolicited inbound is dropped; the rule cannot be
added before the node has created the adapter, and persists once created.
([zipline#162](https://github.com/org-zpr/zl-zipline/issues/162))

**Measured single-worker throughput.** Through one unbatched substrate
socket with two client adapters docked: flood-ping loss 0 % (500 packets,
`-i 0.02`), HTTP download 6,751,114 bytes/s (64 MiB in 9.94 s). Recorded,
not tuned.
([zipline#162](https://github.com/org-zpr/zl-zipline/issues/162))

### `ph node` on macOS

Decisions carried over from the macOS node plan (umbrella
[zipline#158](https://github.com/org-zpr/zl-zipline/issues/158); full text
`git show 7323267:docs/plans/2026-10-01-mac-node.md`). The user-facing
walkthrough is `zl-zpr-core/docs/SETUP.md` "macOS".

**A dev/demo node: `sudo`, single worker, adapters and visa service on
Linux.** Apple Silicon and Intel, no OS floor beyond what the macOS adapter
port already needed. Unlike the Windows node, wildcard and multi-homed binds
are allowed — the unix `posix_unbatched` engine has pktinfo — so the Linux
integration topology applies unchanged. Functional parity is the bar;
single-worker throughput was measured and recorded, not fixed (findings
below). No launchd service.
([zipline#158](https://github.com/org-zpr/zl-zipline/issues/158))

**Nothing may assume `ph` is the only instance on the host.** Same
constraint as the Windows node: loopback dock listen must work, `tun_if`
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
`TunCtl`. The manual-fix hint the node prints when a local ZPR address is
missing from the TUN is now a per-platform string from `sys/` (`ip -6 addr
add` / `ifconfig … inet6 … alias` / `netsh interface ipv6 add address`).
Rejected: applying the address inside `new_mq` via the dead builder path —
two ways to address a TUN, and the startup-check path would differ from
Linux and Windows.
([zipline#161](https://github.com/org-zpr/zl-zipline/issues/161))

**Verification is a hand-run test document plus unit tests; no new CI
tier.** `zl-zpr-core/integration-test/macos-node-test.md` mirrors
`windows-node-test.md` with the Mac as the node: the Linux host runs valkey,
the visa service and its adapter, and client adapters in netns. An automated
macOS tier would need a macOS runner with root and a routable host — same
shape as [zipline#152](https://github.com/org-zpr/zl-zipline/issues/152),
no issue filed.
([zipline#164](https://github.com/org-zpr/zl-zipline/issues/164))

Lasting findings from the first end-to-end run (2026-10-02, macOS 26.5.2 /
Apple M2; the test document's "Findings" has the full table):

**The `/32 alias` alone installs the on-link route.** After `ifconfig utun9
inet6 fd5a:5052::2/32 alias` the kernel installs `fd5a:5052::/32 … Uc utun9`
on-link (plus the self-address `UHL` via `lo0`) — no explicit `add_route` is
needed to reach the visa service, and the /32 is harmless for a node since
everything else is forwarded by the node itself. No change to the
zipline#159 helper.
([zipline#164](https://github.com/org-zpr/zl-zipline/issues/164))

**Wildcard `self_addr` with pktinfo was verified through a single Mac
address only.** All docks complete and stay up with `--self-addr
0.0.0.0:5000`, but the remote-node test environment points every adapter at
one `NODE_LAN_IP` and advertises a single substrate address, so the
two-interface case is unverified; test-env multi-address support is
[zipline#172](https://github.com/org-zpr/zl-zipline/issues/172).
([zipline#164](https://github.com/org-zpr/zl-zipline/issues/164))

**The macOS application firewall was not exercised: it was disabled on the
test Mac.** `socketfilterfw --getglobalstate` reported `State = 0` (left off
by choice), so the run observed neither a prompt nor a block on the inbound
dock (UDP 5000) and VSS (TCP 8183) listeners, and no `socketfilterfw` rules
were needed, added, or validated. On a firewall-enabled Mac the knob is
`/usr/libexec/ApplicationFirewall/socketfilterfw`, with rules keyed on the
executable's path — allow the copy the node actually runs, not the
build-tree binary; the add-nothing-first procedure and expected command set
are in `zl-zpr-core/integration-test/macos-node-test.md` ("Prerequisites,
Mac"), unvalidated until someone runs it with the firewall on.
([zipline#164](https://github.com/org-zpr/zl-zipline/issues/164))

**Links that never complete the noise handshake stay in `Keying`
indefinitely.** Mis-NATed docks from before the NAT rules were up kept
re-timing-out the handshake for the whole run and were removed only at
shutdown. Not macOS-specific — filed as
[zipline#173](https://github.com/org-zpr/zl-zipline/issues/173).
([zipline#164](https://github.com/org-zpr/zl-zipline/issues/164))

**Graceful shutdown is clean.** Ctrl-C revokes visas, resets links, exits on
its own, and the utun — being process-scoped — takes its address and route
with it; a second start with the same command line comes up without a stale
route or interface. Measured single-worker numbers: flood-ping loss 0 %
(500 packets, `-i 0.02`), HTTP download 8,432,600 bytes/s (64 MiB), over
Wi-Fi.
([zipline#164](https://github.com/org-zpr/zl-zipline/issues/164))

### Deferred

Out-of-scope items from the Windows and macOS adapter and node plans, each
with an open issue unless noted:

- Windows service and installer —
  [zipline#147](https://github.com/org-zpr/zl-zipline/issues/147)
- Packet capture to a file via `DuplicateHandle` —
  [zipline#148](https://github.com/org-zpr/zl-zipline/issues/148)
- pktinfo / multi-homed hosts and address change while running —
  [zipline#150](https://github.com/org-zpr/zl-zipline/issues/150)
- Npcap SDK for `ph-cli` filter compilation —
  [zipline#151](https://github.com/org-zpr/zl-zipline/issues/151)
- A Windows integration test tier —
  [zipline#152](https://github.com/org-zpr/zl-zipline/issues/152)
- IP Helper API instead of `netsh` —
  [zipline#153](https://github.com/org-zpr/zl-zipline/issues/153)
- Per-instance control path (node and adapter for one user on one host) —
  [zipline#169](https://github.com/org-zpr/zl-zipline/issues/169)
- Second node substrate address in the remote-node test env (two-interface
  pktinfo verification) —
  [zipline#172](https://github.com/org-zpr/zl-zipline/issues/172)
- Never-completing links stuck in `Keying` —
  [zipline#173](https://github.com/org-zpr/zl-zipline/issues/173)
- Documenting and hand-testing the macOS adapter role —
  [zipline#174](https://github.com/org-zpr/zl-zipline/issues/174)
- launchd service / installer for macOS — no issue filed; file if asked
- An automated macOS integration test tier — no issue filed; same shape as
  [zipline#152](https://github.com/org-zpr/zl-zipline/issues/152)

---

## Implementation status

`zl-zpr-core` is a **pre-release reference implementation**; its README says the
full suite of end-to-end security features is not yet implemented. Read the
RFCs as design intent, not as a description of current behavior.

Working today: nodes and adapters over Noise-secured links, adapter-to-adapter
integrity, bootstrap authentication, visa issuance and distribution with route
selection, revocation and reconciliation on policy change, the ZPL compiler,
and file-backed attribute sources. On `feature/zodes`, Zodes link to each other
from topology and from configured initial links, join through a relay, and
forward across it ([zl-zipline#222](https://github.com/org-zpr/zl-zipline/issues/222);
see [ROUTING.md](ROUTING.md)).

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
- **Adapter-to-adapter confidentiality.** Integrity only, today.
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
| [ZDP.md](ZDP.md) | The data protocol: packet formats, links, docking sessions. |
| [SECURITY_MODEL.md](SECURITY_MODEL.md) | Threat model, trust boundaries, what ZPR does not defend against. |
| [REPOSITORIES.md](REPOSITORIES.md) | Which repository implements what. |
| [BUILD.md](BUILD.md) | Building and testing everything. |

`zl-zpr-core/packet_walk.md` follows a packet through the whole system and is the
best next read for anyone working on the data path.
