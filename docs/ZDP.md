# ZDP — the ZPR Data Protocol

ZDP is the wire protocol of a ZPRnet. Everything that crosses a substrate
network between two ZPR entities is a ZDP packet: application traffic
(*transit packets*), flow setup, link and docking-session management, and key
exchange. ZDP is what makes a ZPRnet an overlay — an actor's IP packet is
stripped of its addresses, wrapped in a ZDP header carrying only a numeric
Stream ID, and switched hop by hop on that ID.

Read this before touching `adapter/ph` packet handling, adding or renumbering
a message type, or changing anything about how packets are secured on the wire.

## Authoritative sources

| Source | What it governs |
|---|---|
| **Internal RFC 17** — "ZDP Protocol Definition" | The current protocol definition: procedures, packet formats, message types. |
| **Internal RFC 6.4** — "ZPR Data Protocols" | The predecessor. Still the best source for **substrate** requirements and encapsulations (§3), the addressing model (§4), and the rationale behind field sizes. Its packet formats are superseded. |
| **`zl-zpr-core/adapter/ph/src/zdp.rs`** | The wire format **as actually implemented** — header structs and the message-type enum. |
| **`zl-zpr-core/packet_walk.md`** | The intended life of a packet, written as an implementation guide. |

The ZDP RFCs live only as `.docx`/`.pdf`; unlike internal RFCs 4, 12, 15, 16,
and 19 there is no Markdown source. Extract text with `pdftotext -layout`.

Code comments cite "RFC 6.5" (17 places) — a revision that is **not** in the
internal RFC set, which stops at 6.4. Section numbers in those comments track
6.4 closely enough to follow; treat them as approximate.

Related: internal RFC 4 (terminology), internal RFC 12 (ZPR overview),
internal RFC 16 (identity).
For policy see [ZPL.md](ZPL.md); for terms see [TERMINOLOGY.md](TERMINOLOGY.md).

---

## A note on names

The same entity has three names across three eras, and all three are in the
tree right now:

| Internal RFC 6.4 | Internal RFC 17 | `zl-zpr-core` code |
|---|---|---|
| Agent | Endpoint | **Actor** |
| Agent Packet | Endpoint Packet | actor packet |
| D2D (dock-to-dock) security | Adapter-to-Adapter security | **A2A** |
| Visa Heralding | Visa Installation / Stream ID Request | `StreamIdRequest` |

The code is the newest and says *actor*, but leftovers survive (e.g.
`ZdpBindActorAddressRequestHeader.endpoint_packet_length`). ZPL had its own
version of this churn: `device` → `endpoint` → `device` again. Expect to
translate.

---

## The protocol family

Internal RFC 6.4 §1 names five protocols. ZDP is the transport under the others:

| Protocol | Between | Status in `zl-zpr-core` |
|---|---|---|
| **ZDP** — data protocol | any two ZPR entities | implemented |
| **ZIP** — ZPR Interface Protocol | adapter ↔ dock | implemented as ZDP management messages |
| **ZPR Link Management** | forwarder ↔ forwarder | messages implemented; node-to-node links are not |
| **ZPR Keying** | adapter ↔ dock, forwarder ↔ forwarder | implemented with **Noise**, not IKEv2 |
| **ZARP** — address resolution | multipoint substrates | not implemented (type reserved) |

ZIP and Link Management are not separate wire protocols — they are ZDP
management message types, disambiguated by the Type field.

---

## Substrate

ZDP requires very little of the substrate (6.4 §3.1): an unreliable,
unordered, best-effort packet service. No security, no flow control, no
fragmentation, no delivery notification. Substrate addresses are required only
where an interface can reach more than one peer.

The RFC defines encapsulations for raw links, PPP, bare Ethernet, IP, and
IP/UDP. **`zl-zpr-core` implements IP/UDP only** — `ph` binds
`std::net::UdpSocket` per configured interface (`main.rs`), IPv4 and IPv6 both.
`self_addr = "129.6.7.1:5000"` in a node config is the substrate address.

A node keeps ZPR-address → (substrate IP, UDP port) mappings in its peer
table, keyed for receive by source substrate address
(`peer_table.lookup_peer`). A packet from an unknown substrate address becomes
a candidate new link and goes to the management plane.

---

## Addressing and identifiers

ZDP separates the addresses that appear on the wire in headers (almost none)
from the addresses used to name things in the control plane.

**Control-plane addresses** (6.4 §4.1). Never appear in ZDP headers, so their
syntax is unconstrained — strings, DNs, or IP addresses all work:

| | |
|---|---|
| **AA** — Actor Address | The actor's own IP address. The only one visible outside the ZPRnet, and the only one in an actor packet. |
| **SA** — Substrate Address | An interface's address on the substrate. Visible outside. |
| **NA** — Node Address | Names a node; a prefix from which DA and FA are cut. |
| **DA / FA** — Dock / Forwarder Address | Derived from the NA. |
| **LA / TA** — Link / Tether Address | Derived from FA / DA. A link has two LAs, one per end. |

Routing is to a **Tether Address**; because NA ⊃ DA ⊃ TA as prefixes, a route
to a dock covers every tether on it.

**Data-plane identifiers** (6.4 §4.2, `zl-zpr-common/src/packet_info.rs`):

| Name | Type | Notes |
|---|---|---|
| **Stream ID** | `u32` | The generic name. Called a **Visa ID** on a link, a **Tether ID** on a docking session. Sole basis of forwarding. |
| Visa ID (control plane) | `u64` | The visa service's name for a visa. Distinct from the on-wire Stream ID. `MIN_VISA_ID = 1000`. |
| **Link ID** | `u32` | Local handle for a link or docking session. `0` = unknown, `1` = local actor, `2` = the dock. |
| **ZPI** | `u8` | ZPR Parameter Index — selects SA and configuration. |
| **A2A SAID** | `u8` | Selects the adapter-to-adapter SA within a flow. |

Stream ID **0** is reserved for node-to-node control-plane traffic. Each
direction of each link has its own ID space, so the same ID may name four
different flows around a two-link path — deliberately, so that neither end has
to coordinate allocation and so IDs can be raw table indices.

Stream IDs are allocated from 1 after a restart, and a terminated tether's ID
is held for 10 seconds before reuse (6.4 §5.9) so in-flight packets drain.

---

## Packet format

ZDP has no fixed packet format. Field widths come from a **ZPR
Configuration**, selected — along with the security association — by the ZPI
byte, which is always the first byte and never encrypted. The RFC therefore
specifies a **Baseline Configuration** that every implementation must hard-code
and that is used when nothing else is active.

The layouts below are what `zl-zpr-core` actually builds. Headers are pushed
inner-to-outer with `alloc_zeroed_header`, so read the source in reverse.

### Transit packet (Type 0)

```text
+-------+-------+---------------+-----------------+----------+-------------+---------+---------+
|  ZPI  | Type  | Excess Length |    Stream ID    | A2A SAID | compressed  | A2A MAC | link    |
|  1 B  | 1 B   |     1 B       |       4 B       |   1 B    | actor pkt   |   8 B   | MAC 8 B |
+-------+-------+---------------+-----------------+----------+-------------+---------+---------+
        |<------------- HMAC'd (transit) or encrypted (mgmt) --------------------->|
```

### Management packet

```text
+-------+-------+---------------+-----------------+-------------+---------+-----------------+
|  ZPI  | Type  | Excess Length | Sequence Number | Stream ID   | Txn ID  | message body    |
|  1 B  | 1 B   |     1 B       |      8 B        | 4 B (per-   | 2 B     |                 |
|       |       |               |                 | flow only)  | (most)  |                 |
+-------+-------+---------------+-----------------+-------------+---------+-----------------+
```

Per-flow versus non-per-flow is encoded in the **high bit of the Type field**:
clear (`0x00`–`0x7f`) means a Stream ID follows the sequence number, set
(`0x80`–`0xff`) means it does not. `ZdpPacketType::is_per_flow()`. The cBPF
`SO_REUSEPORT` steering program in `packet_steering.rs` relies on exactly this,
plus the convention that a ZPI with the high bit set (`ZPI_ENCRYPTED_HEADER_FLAG
= 0x80`) means the payload is encrypted and the Stream ID cannot be read.

Divergences from the RFC's baseline packet, all live in the code today:

- **No Flags field.** The RFC's `Flags` byte and its Partial Encryption (`P`)
  bit do not exist. The implementation makes the same distinction with two
  ZPIs per SA instead (see below).
- **Sequence number is 8 bytes, not 2, and only on management packets.** The
  RFC puts a 2-byte sequence number in every header for replay protection.
  `ZdpMgmtHeader` is a full `u64` and transit packets carry none.
- **Field order differs**: sequence number precedes the Stream ID.
- **A 2-byte transaction ID** matches requests to responses for most
  management messages. The RFC uses the sequence number for that.
- **No Pad field**; the MICV/MAC is appended to the end of the packet rather
  than sitting between header and payload.
- **`Excess Length` is never set or read** — always 0. It exists for
  substrates with a minimum frame size (Ethernet's 46 bytes); irrelevant while
  the only substrate is UDP.

### Message types

As implemented (`zdp.rs`). **The numbering is not internal RFC 6.4's** — 6.4's
table is superseded by internal RFC 17 and the code follows the newer scheme.
Never infer a type value from 6.4.

| Value | Type | Per-flow |
|---|---|---|
| 0 | Transit Packet | yes |
| 1 | Destination Unreachable | yes |
| 2 | Set Path MTU | yes |
| 3 / 4 | Stream ID Request / Response | yes |
| 5 | Stream ID Withdrawal | yes |
| 6 / 7 | Bind Actor Address Request / Response | yes |
| 8 / 9 | Bind Egress Stream Request / Response † | yes |
| 13 | Unbind Egress Stream Indication | yes |
| 128 | ZPR ARP (reserved, unimplemented) | no |
| 129 | Key Management | no |
| 130 | Discard | no |
| 131 | Echo | no |
| 132 | Report | no |
| 133 | Terminate Link or Docking Session | no |
| 134 / 135 | Hello Request / Response | no |
| 136 / 137 | Configuration Request / Response | no |
| 138 / 139 | Acquire / Grant ZPR Address † | no |
| 140 | Revoke ZPR Address | no |
| 141 | Init Authentication Request † | no |
| 142 / 143 | Start Authentication / Abandon Authentication Attempt † | no |
| 144 / 145 | Renew Authentication Request / Response ‡ | no |
| 252 / 253 | Canceled / Cancel † | no |
| 254 | Acknowledgement | no |
| 255 | Reserved — silently discard | — |

† marked `TODO: add to RFC` or *experimental* in `zdp.rs`: implemented ahead of
the spec. 142/143 are upstream zpr-core #1410's experimental actor-authorization
assignments (`diagrams/actor_authorization.md` in zl-zpr-core).

‡ zipline-only (zipline#66): node-to-adapter credential renewal. They were 142/143
until zl-zpr-core#96 merged #1410, which claimed those numbers; zipline's moved,
so zipline nodes and adapters from before and after that merge do not interoperate.

Note the shape of the divergence: the RFC has separate Echo Request/Response
and Terminate Request/Response/Indication types; the implementation has one
type each and distinguishes direction in the body. Types 252–254 are the
reliability layer, which the RFC does not have at all.

---

## Security

Two independent layers, deliberately (6.4 §6.2.1). Together they protect what
the network itself reads without making every forwarder do actor-payload crypto.

**ZDP Header SA (ZHSA)** — per link or docking session, pairwise, established
by the keying protocol during bring-up. Covers the ZDP header, and for
management packets the payload too. Provides integrity, origin authenticity,
confidentiality, and replay protection. Does **not** cover the encapsulated
actor packet.

**A2A SA** (the RFC's D2D) — end to end for a flow. Integrity only, no
confidentiality and no replay protection. Keys come from the visa service with
the visa, never propagated hop by hop, so a compromised forwarder cannot forge
actor payloads. The RFC specifies dock-to-dock and notes in a margin comment
that pushing it out to the adapters would offload the docks; the
implementation did exactly that, hence "A2A".

As implemented:

- The A2A MICV is BLAKE3, keyed by `blake3::derive_key(ZDP_A2A_MICV_KEY_CONTEXT,
  shared_secret)` over an X25519 exchange, truncated to 8 bytes. With no key
  established it degrades to an unkeyed `blake3::hash` — integrity check only,
  no authentication. Only A2A SAID `0` is handled; anything else is `todo!()`.
- Computed by the **ingress adapter over the uncompressed packet** before
  compression, and checked by the **egress adapter after expansion** — so the
  A2A MICV is invariant across whatever the docks compress.
- The link MAC is BLAKE3 keyed with the SA's send key, 8 bytes, appended last.
- Each SA gets **two ZPIs** (`ZPIPair { encr, hmac }`). Transit packets go out
  under the `hmac` ZPI: MAC appended, body in the clear. Management packets go
  out under the `encr` ZPI: whole body encrypted. This replaces the RFC's
  Partial Encryption flag.

**ZPI 0** is the NULL SA (6.4 §5.25.2): no encryption, a 2-byte unkeyed
Internet checksum for error detection, Baseline Configuration. It exists
because the keying protocol has to run before any SA exists, and it is
self-securing. `zl-zpr-core` enforces the restriction on ingress — under ZPI 0,
**only Key Management messages are accepted**; anything else is dropped and
counted.

**Keying.** The RFC specifies IKEv2, with a long editorial aside about
redefining it to run over non-IP substrates. `zl-zpr-core` implements **Noise**
(`km_noise.rs`, `KM_ID_NOISE = 2`) with X.509 certificates carrying Noise
public keys, signed by a CA (`km_cert_exchange.rs`, `pki.rs`). On a link
between two Zodes both ends send their certificate and both verify the peer's:
the handshake aborts on a missing certificate, a certificate not signed by the
local CA or outside its validity window, or a certificate whose key is not the
peer's Noise static key, and a
Zode without a CA certificate does not start the link. After keying, the
certificate's CN must be a `peerCn` in the node's current topology, or the link
is closed before Hello (see [SECURITY_MODEL.md](SECURITY_MODEL.md), *Design
decisions*; [zipline#220](https://github.com/org-zpr/zl-zipline/issues/220)).
`KM_ID_IKEV2 = 1` exists as a constant and a predicate; there is no IKEv2
implementation.
RFC 8019 anti-DDoS measures for the responder are not implemented.

---

## Reliability (ZDPR)

Management messages need request/response semantics. Internal RFC 6.4 §6.4 specifies
stop-and-wait ARQ: window of exactly 1, 64-bit non-wrapping sequence numbers
per link, 1-second timer, 3 retries. The RFC argues the simplicity is worth the
throughput because these are low-frequency operations.

`zl-zpr-core` implements a sliding-window version in `zdpr.rs` (`Sender` /
`Receiver`, one pair per direction per link) with cancellation:

| | Internal RFC 6.4 | `zl-zpr-core` |
|---|---|---|
| Window | 1 | `DEFAULT_ZDPR_RECEIVE_WINDOW_SIZE = 32` |
| Retry timer | 1 s | `DEFAULT_ZDPR_RETRY_TIMER = 600 ms` |
| Retry limit | 3 | `DEFAULT_ZDPR_RETRY_LIMIT = 3` |
| Matching | sequence number | sequence number for ack; 2-byte transaction ID for request/response |
| Cancellation | none | `Cancel` / `Canceled` types |

Sequence numbers are per link and never wrap (`SeqNum = u64`), which is what
makes replay impossible without a replay window. The sender tracks
out-of-window, duplicate, too-old and too-new acks as statistics, and
`SenderStat::is_protocol_error()` marks the ones that indicate a misbehaving
peer — the caller may reset the link on those.

---

## Procedures

### Link and docking-session bring-up

Docking sessions are **always initiated by the adapter** (6.4 §5.6): the dock
platform serves many adapters, adapters often have no fixed substrate address,
and probing would be wasteful and noisy. For node-to-node links, whichever node
has the numerically smaller node number initiates.

The sequence is: keying (Noise) → `Hello` Request/Response → link Active with
`Echo` keep-alives → actor authorization. A Hello Response may accept, reject, or
redirect to another dock. Each side names the connection policy under which it
initiated or accepted.

Since zl-zpr-core#96 (which merged upstream #1410) **the link and the actor have
separate state machines.** The link is Active once Hello completes; it then runs
actor authorization as its own exchange, and a failed authorization no longer
tears the link down.

- **Link** (`link_state.rs`). States: `Inactive`, `Keying`, `Helloing`, `Active`,
  `Closing`, `Resetting`, `Disconnecting`, `Error`. Link types: `Internal`,
  `AdapterToNode`, `NodeToAdapter`, and `NodeToNode` — the last marked
  *currently unsupported*.
- **Actor** (`actor_auth.rs`, `ActorAuthorization`). One transaction carries
  `Start Authentication` → `Init Authentication` (the node's challenge) →
  `Acquire ZPR Address` (the adapter's evidence) → `Grant ZPR Address`, or
  `Abandon Authentication Attempt` on a local failure. Grant and Renew Response
  carry an `AuthenticationStatus`: `Success` 0, `Rejected` 1, `Retryable` 2, and
  the zipline-only `PolicyDenied` 3. With auto-connect off every failure ends
  the attempt; with it on, transient failures retry with backoff and explicit
  denials latch. Either way the operator retries with `ph-cli connect` (or by
  restarting the adapter).

The Hello exchange carries TLVs 9 `ACTOR_AUTH_PROTOCOL` (both directions,
version 1, mandatory), 10 `VS_BOOTSTRAP_LINK` (response, verified VS adapter
only), and the zipline-only 11 `OIDC_IDP` (response, one per advertised OIDC
identity provider; it was 9 before zl-zpr-core#96). The upstream ASA TLV (4) is
not used: zipline removed it with the BAS client (zipline#15).

Keep-alive is `Echo` every `DEFAULT_KEEP_ALIVE_PERIOD = 3 s` with a matching
3-second timeout; a link is torn down after `KEEP_ALIVE_MAX_MISSES = 3`
consecutive misses (zipline#113). `LINK_HELLO_TIMEOUT = 3 s`, restart holddown
5 s, and a node drops a link whose keying does not finish within
`NODE_KEYING_TIMEOUT = 60 s` (zipline#173).

### Flow setup

The adapter keeps an **Actor Lookup Table** (ALT/ELT) keyed by 5-tuple and a
**Dock Lookup Table** (DLT) keyed by Stream ID; the node keeps a **Peer
Forwarding Table** (PFT) per link, keyed by Stream ID. A lookup yields a
**PEP** (Policy Enforcement Procedure): where to send the packet, what Stream ID
to write, how to compress, plus any traffic constraints.

First packet of a new flow (`fastpath.rs::actor_output_post_classify`):

1. Adapter classifies the packet, decrements TTL, looks up the 5-tuple. Miss →
   a `Bind Actor Address Request` carrying the packet body (capped at
   `BIND_REQUEST_MAX_PAYLOAD_LENGTH = 256`) goes to the dock, and the ALT entry
   goes `Pending`.
2. Further packets on a `Pending` entry are **dropped**, counted as
   `DroppedAwaitingBind`. (The RFC says buffer the most recent one.)
3. The dock either matches an existing visa or asks the visa service, which
   evaluates policy against classified attributes and returns a visa plus A2A
   keying material.
4. The dock allocates a Tether ID and answers with a `Bind Actor Address
   Response` carrying the traffic classifier, compression mode, and the peer's
   A2A public key. The adapter finalizes the ALT entry and re-injects the held
   packet.
5. Downstream, a `Stream ID Request` naming the `visa_id` walks toward the
   egress; `Bind Egress Stream Request/Response` sets up the egress adapter's
   DLT entry and returns the Stream ID to use.

This is the successor to **Visa Heralding** (6.4 §5.11), where the visa itself
travelled hop by hop in `Visa Herald Request` messages with a hop count, each
forwarder validating it and offering back a Visa ID. Read §5.11 for the
intended semantics of re-heralding around a failed link and of the
`Visa Deaccept` cascade that tears a half-built path back down — the
implementation currently has one node, so none of it is exercised.

An intermediary node holds a `forwardOnly` visa for the flow: an ID and a next
hop, no 5-tuple. It answers a `Stream ID Request` from the previous node by
sending its own `Stream ID Request` for the same `visa_id` to the next hop, then
maps the upstream Stream ID to the downstream one in the upstream link's PFT. It
does this only with its `forwarding` setting on, and only between two node
links: an adapter binds by traffic classifier, which a `forwardOnly` visa lacks.
See [ROUTING.md](ROUTING.md) ([zipline#219](https://github.com/org-zpr/zl-zipline/issues/219)).

A `Stream ID Request` is accepted only from the visa's **previous hop**:
`Visa.prevHop`, compared against the ZPR address the node learned for the
arriving link's peer from topology, never from Hello. A visa without
`prevHop` (every ingress visa, and any visa from a visa service that predates
the field) never accepts one, so it cannot be bound for forwarding (gap 7;
[zipline#243](https://github.com/org-zpr/zl-zipline/issues/243)).

### Forwarding

Forwarding is on **Stream ID alone** (6.4 §5.14): the ingress link plus the
Stream ID in the header determine the outbound link and the Stream ID to write.
Nothing else is consulted; the substrate's own IP routing is invisible to ZDP.
Stream ID 0 goes to the control plane. An unknown Stream ID is dropped and
counted — the RFC says send `Destination Unreachable` upstream, with an
editorial note that doing so lets a compromised upstream probe which IDs are
live. `fastpath.rs::forward` drops silently and has a `TODO: policy
enforcement` where the PEP's constraints should be applied.

### Compression

Two stages, and only the first is mandatory (6.4 §5.26).

The **ingress adapter always removes the IP addresses**, which the bind
exchange has already given to both ends. This is not primarily a bandwidth
optimization: for IPv6 it reclaims exactly the room the ZDP headers need, so
the packet does not grow and trip an MTU somewhere. (For IPv4 it does not
reclaim enough.) It also raises payload entropy.

Optionally the **ingress dock** removes further fields — only invariant ones,
as directed by the visa — and the egress dock restores them. So a packet on a
tether has exactly its addresses removed; a packet on a link may have more.

`compress.rs` implements the adapter stage for IPv4 and IPv6 plus the ports:
`compression_mode` bits `SOURCE_PORT_PRESENT` (0x40) and
`DESTINATION_PORT_PRESENT` (0x20) say which ports remain on the wire. IPv4
keeps header length, DSCP, fragment id/offset/flags and TTL; total length and
header checksum are recomputed on expansion. There is a known deviation in how
fragment flags are packed, marked `NOTE/TODO: spec deviation`. Fragments
themselves are **not handled** — the classifier detects them and the packet is
dropped.

---

## Implementation status

Specified, not implemented:

- **Node-to-node links.** `LinkType::NodeToNode` is *currently unsupported*;
  integration tests are `one-node-test.sh` and `one-node-v6-test.sh`. Multi-hop
  forwarding, visa heralding, next-hop selection (6.4 §5.11.2), and route
  distribution (§5.12) are therefore unexercised. The intermediary node's
  `forwardOnly` stream binding (Flow setup, above) is implemented but tested
  in-process only ([zipline#219](https://github.com/org-zpr/zl-zipline/issues/219)).
- **Substrates other than IP/UDP** — raw links, PPP, bare Ethernet.
- **ZARP** (6.4 §3.3.1) — type 128 reserved, no implementation.
- **IKEv2** — constant only; Noise is what runs.
- **IP fragmentation and path-MTU management** (§5.19, §5.29, §7.2). The RFC's
  own editorial notes call this area unfinished.
- **ZPR Configurations.** Only the Baseline Configuration exists; field widths
  are Rust struct definitions, not runtime-configurable.
- **A2A SAIDs other than 0**, and MAC sizes other than 8 bytes.
- **`Stream ID Request` previous-hop rule.** `Visa.prevHop` is in the schema
  only: the visa service fills it in
  [zipline#245](https://github.com/org-zpr/zl-zipline/issues/245) and core
  enforces it in [zipline#222](https://github.com/org-zpr/zl-zipline/issues/222).
  Today a node accepts a `Stream ID Request` from any node link.
  Reverse binding for symmetric visas is undefined
  ([zipline#246](https://github.com/org-zpr/zl-zipline/issues/246)).
- **Visa lifetime expiry, retraction, de-acceptance, and refusal** (§5.15) as a
  cascade. `Stream ID Withdrawal` and `Unbind Egress Stream` exist.
- **PEP traffic constraints** — rate limits, QoS, DSCP marking, queue
  assignment. The PEP carries next-hop only.

Implemented ahead of the RFC: the reliability layer (`Cancel`/`Canceled`/
`Acknowledgement`), `Bind Egress Stream`, `Acquire`/`Grant ZPR Address`,
`Init Authentication`, `Start`/`Abandon Authentication`, the zipline-only
`Renew Authentication` pair, and transaction IDs.

Before assuming a message or field works, read `zdp.rs` and grep
`mgmt/handlers.rs` for a handler. A type in the enum does not imply a
handler, and a handler does not imply the RFC describes it.

---

## Where the code lives

All paths under `zl-zpr-core/adapter/ph/src/` unless noted.

| Concern | Location |
|---|---|
| Wire format: header structs, type enum, constants | `zdp.rs` |
| Shared identifier types, ZPI, KM IDs, compression modes | `zl-zpr-common/src/packet_info.rs` |
| Datapath: ingress, egress, forwarding, encrypt/decrypt | `fastpath.rs`, `fastpath_io.rs`, `batch_io.rs` |
| Packet buffer: metadata / headroom / body / tailroom | `packet.rs` |
| 5-tuple classification | `classifier.rs` |
| Header compression and expansion | `compress.rs` |
| Reliable-transport state machine | `zdpr.rs` |
| Management plane: dispatch, handlers, senders | `mgmt/`, `mgmt_dispatch_worker.rs`, `mgmt_processor_worker.rs` |
| Transaction IDs | `mgmt/txn_mgr.rs` |
| Link/docking-session FSM | `link_state.rs`, `peer_table.rs` |
| Keying | `km.rs`, `km_noise.rs`, `km_cert_exchange.rs`, `km_multiplexor.rs`, `pki.rs` |
| Visa and forwarding tables | `visa_table.rs`, `forwarding_tables.rs`, `adapter_tables.rs` |
| Visa service connection | `visa_mgmt.rs`, `libnode2/src/vsconn.rs` |
| Receive-queue steering (cBPF over ZDP headers) | `packet_steering.rs` |
| Timers, window sizes, buffer sizes | `config.rs` |
| Counters (per-drop-reason, per-worker) | `counters.rs` |

A change to the wire format usually touches `zdp.rs`, the datapath in
`fastpath.rs`, the management plane in `mgmt/`, and — if an identifier type or
constant is shared — `zl-zpr-common`. The visa service side of visa issuance lives
in `zl-zpr-visaservice`; see [REPOSITORIES.md](REPOSITORIES.md).
