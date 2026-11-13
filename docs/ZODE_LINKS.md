# Zode links: lifecycle (v1)

**Status:** Implemented for zl-zipline#250 (rules 1–15 except 11a) and for
#251, forwarding over Zode links (F1–F8, except F2's `prev_hop` check, which is
#253), on `feature/zodes`. Rules 11a and 16
and the relay join (J1–J11) come with #252: designed and approved, not yet
implemented. See "Implementation status" for where the code lives and what it
does differently.

This document says how two Zodes bring up, keep, lose and recover a link between
them. It replaces the link handling built in core#104 (closed), which grew
without a design: crossed dials, redials, duplicate and stale links. Read it with
`ZDP.md` (packet format, Hello, keying), `ROUTING.md` (topology, link-state routing)
and `VISA_SERVICE.md` (VS-API session, reconnect).

## Why

A Zode is the one `ph` process on a machine. Endpoints reach the network through a
**relay** Zode and, where both ends can reach each other, over **direct** links
that the visa service hands out in `setTopology`. Link handling has to be simple
enough to reason about. That means a small set of rules decides **who dials**,
**what is accepted** and **what happens on loss**, so that no case needs special
handling.

## Goals

- Exactly one link per Zode pair, with no crossed dials and no duplicates, by
  construction.
- Every link loss is handled the same way: clear that link's state, report it,
  and let the visa service decide what happens next.
- An endpoint that changes networks gets back on the network by itself.
- Direct links carry traffic only once both ends have confirmed them.

## Non-goals (v1)

- Reaching a Zode that can't be dialed (NAT traversal, ZBOLT, rendezvous). That is
  v2. **In v1 a direct (non-relay) link exists only between two Zodes that can each
  reach the other.** Everything else goes through the relay.
- Background retries of direct links. A failed direct link stays down until the
  visa service sends new topology.
- Preserving link or stream state across a link loss.
- Backwards compatibility. Old or legacy values and formats are rejected, not
  tolerated (operator rule for all Zodes work).

## Anti-goals

- Any rule that needs a race, a tie-break after keying, or a "which duplicate
  wins" decision. If the design needs one, the design is wrong.
- The relay redialing an endpoint. From the relay's side a vanished endpoint is
  the same as one that was force-quit.

## Kinds of link

| Kind | Where it comes from | Owner (dials) | Retries |
|---|---|---|---|
| **Initial link** | The Zode's own config: relay substrate address and the relay's expected CN | The side that has it configured, which is always the endpoint | **Yes**, with backoff, until it comes back. It is the Zode's lifeline. |
| **Topology link** | `setTopology` from the visa service | The end with the **lower ZPR address**, comparing the two addresses as 128-bit unsigned integers (IPv4 is compared as its IPv4-mapped IPv6 form) | **No.** One attempt per `setTopology`. |

If a link is both (the relay also appears in the endpoint's topology), the
**initial-link rule wins**: the endpoint owns it and retries it.

`Link.zprAddr` and `Link.peerCn` are **required** in `setTopology`. A link missing
either is rejected and logged, and the rest of the topology is applied. The schema
comments saying "may be unset (older senders)" go away.

## Rules

### Who dials

1. Only the owner dials, and only the owner ever redials.
2. The non-owner never dials that peer. It only answers.

### What is accepted (inbound)

3. An inbound link is accepted only if the peer's verified certificate CN appears
   in this Zode's **current** topology **and** the peer is that link's owner.
   Anything else is refused after keying, when the CN is known, and logged with
   the reason. That covers both an unknown CN and a known peer that isn't the
   owner.
4. **A Zode configured as a relay never dials** an endpoint, whatever the address
   order. It accepts inbound links from any peer whose CN is in its topology. The
   relay is the one exception to "the dialer must be the owner". No schema marker is
   needed: being a relay is the Zode's own configuration. As in #252, the joiner's CN
   is in the relay's topology before the joiner connects. *(Coordinator call,
   from the builder check.)*
5. An inbound link from a peer that already has an established link **replaces**
   it: the old link is closed and its state cleared, then the new one is set up.
   The usual reason is that the owner restarted and dialed again.

### Bringing a topology link up (owner)

6. When `setTopology` names a link this Zode owns, the owner dials and keys. If the
   link isn't up within **5 seconds** (default, see "Left to the builder"), it
   reports `failed` and stops. **Within that window the owner may redial about once
   a second.** This covers the case where the owner's copy of topology lands first
   and the peer refuses it because the peer doesn't know the link yet (rule 3).
   The non-owner stays strict: an unknown CN is refused, never held. *(Coordinator
   call, from the builder check.)*
7. Once up and the CN verified, it reports `up`.

### Bringing a topology link up (non-owner)

8. The non-owner reports `up` once the owner's link arrives and passes rule 3. It
   never reports `failed` for a link it doesn't own.

### Loss

9. When any link is lost or closed, **both sides clear all state tied to that
   link**: streams bound over it, visas learned over it (including VS-API and
   bootstrap visas carried over an initial link), and pending requests.
   Re-requesting visas afterwards is harmless. Nothing about the old link survives.
10. For a topology link, each side reports `down`. The owner does **not** redial.
11. For the initial link, the endpoint redials with backoff until it comes back.
    The relay does nothing except clear the state, per rule 9.
11a. **Every return to the relay is a join.** Each new initial link goes through the
    same join hand-off as the first one (#252): after Hello, the relay passes the
    endpoint fresh VS-minted bootstrap visas. The visa service re-mints them once it
    sees the endpoint's VS-API session gone. Only then does rule 16 apply.

### Topology changes

12. A link removed from `setTopology` is closed, and its state cleared per rule 9.
    It is no longer reported.
13. **Every `setTopology` retries** each `failed` or `down` link it still contains:
    the owner makes one more attempt under rule 6. Links that are up are untouched,
    and the Zode keeps no other retry logic.
13b. **The visa service proposes a direct link only when it can work.** It needs a known substrate address for the peer the owner must dial, from policy or reported by that peer. No address means no direct link, and traffic goes through the relay. The visa service never asks a Zode to make a connection it can't possibly make. *(Operator, 2026-11-12; #255 drops visaservice#65's `[::]:0` fallback.)*
13a. **The visa service owns the retry policy.** It remembers links reported
    `failed` and leaves them out of the topology it proposes until something about
    that pair changes: a peer's substrate address changes (#227
    `updateSubstrateAddr`), a peer reconnects, or in v2 a ZBOLT/rendezvous step says
    the pair can try again. A `failed` report never itself causes a topology send,
    so the two can't loop. Dropping the link from topology closes it under rule 12.
    This is also the v2 hook: once both sides have holes open, the visa service just
    proposes the link again.

### Routing on link state

14. The visa service routes over a topology link only when **both** ends report it
    `up`. A link with no report counts as **down**. This replaces #224's
    transitional "never reported = up" rule; #242 is folded into this work.
15. `down` or `failed` revokes visas over that link, as #224 does today. New visa
    requests then take another path, usually through the relay.

### VS-API session

16. The VS-API session runs over the initial link. When it is lost, the Zode
    reconnects as `VSConnT::reconnect` once its initial link is back. **The visa
    service decides** whether that is enough: it may answer `invalidState` or
    `invalidOperation`, and then the Zode clears its state and connects as `reset`,
    re-authenticating the machine and user. After either, the visa service re-sends
    topology and anything else the Zode needs.

## Walkthroughs

**Laptop switches networks.** The initial link drops. The laptop clears that
link's state (rule 9) and keeps redialing the relay with backoff (rule 11).
Meanwhile the relay sees a dead link, clears its state and waits. When the new
network comes up, the laptop's initial link comes back, and it goes through the
join hand-off again (rule 11a). The laptop then reconnects its VS-API session
(rule 16), and the visa service either resumes it or demands a
fresh login. Topology arrives again. Direct links that were up before are
attempted once by their owners (rule 13).

**Two reachable Zodes get a direct link.** Both receive `setTopology` naming each
other. The lower ZPR address dials (rules 1 and 6), the higher one accepts it (rule
3), and both report `up` (rules 7 and 8). The visa service starts routing over it
(rule 14).

**The higher-address Zode dials anyway**, from a bug or a stale config. The lower
one refuses it after keying because the dialer isn't the owner (rule 3). No
duplicate is possible.

**A direct link can't come up** (a firewall, say). After 5 seconds the owner
reports `failed` (rule 6). Traffic goes through the relay. The visa service stops
proposing that link (rule 13a) until something about the pair changes. When it does
propose the link again, the owner tries once more (rule 13).

**The owner restarts.** It dials again. The non-owner's old link is replaced by
the new one (rule 5), and both report `up`.

## Reporting summary

| Event | Owner reports | Non-owner reports |
|---|---|---|
| Link up, CN verified | `up` | `up` |
| Didn't come up within the timeout | `failed` | nothing (counts as down) |
| Established link lost or closed | `down` | `down` |
| Removed from topology | nothing (no longer in topology) | nothing |

## Scope of #250 (decided)

core `feature/zodes` has none of core#104's link code, so #250 builds the Zode link
layer from scratch:

- **zl-zpr-core:** initial-link config (relay substrate address plus expected CN,
  generalizing `node_addr`), Zode-to-Zode keying and Hello, the owner and CN rules
  (1–8), the per-link state machine, loss handling (9–12), retry on topology
  (13) and link-state reporting.
- **Schema:** `Link.zprAddr` and `Link.peerCn` become required. That touches
  zl-zpr-vsapi, then a zl-zpr-common tag, then the visa service and core moving
  their pins together (BUILD.md rule 6).
- **zl-zpr-visaservice:** rule 13a (stop proposing failed links until the pair
  changes), and rule 14 (unreported = down), which is #242, done alongside.
- **Not in #250:** the relay's bootstrap-visa hand-off (rule 11a) is #252.
  Until #252 lands, #250 is tested with Zodes whose initial link is the visa
  service's own node, the way `node_addr` works today.

## Reporting contract (snapshot semantics)

`reportLinkStates` is a snapshot. A topology link that isn't listed counts as not up.

- The owner lists a link as `failed` **only while that link's attempt has ended
  without the link coming up** (the 5 s window ran out). Once a retry starts after a
  new `setTopology`, the entry is **no longer listed**. If the retry fails too, it is
  listed as `failed` again.
- Every change to the set of listed entries sends a new snapshot. That includes
  `failed` → not listed → `failed`.
- The visa service marks a pair as failed (rule 13a) only on a **new** failure,
  meaning an entry that has moved into `failed` since that reporter last listed it.
  A snapshot that repeats an old `failed`, because it was sent for some other change
  before the new topology arrived, does not mark the pair again. That prevents a
  re-proposed link from being dropped before it is ever tried. *(Agreed between core
  and the visa service while implementing #250, from a Copilot review finding on visaservice#66.)*

## Forwarding over Zode links (#251, design approved 2026-11-12, implemented)

The link layer above decides **which links exist**. This section covers how traffic
**crosses** them.

**The visa service drives all forwarding.** It plans each path and seeds every
Zode on it with that hop's visa: the forward-only visa and its next hop, plus
`prev_hop` since #245. A Zode never picks a route and never agrees teardown with
a neighbour. It does what its visas say.

### Rules

F1. **Egress to a Zode.** When a visa's next hop is another Zode, the egress is the
    **up** Zode link whose peer has that ZPR address. The address is the topology
    `Link.zprAddr` of the entry matched by the peer's verified CN. It never comes
    from the Hello or anything else the peer claims about itself (gap 5).
    `find_egress_link` matches these links just as it matches local and adapter
    addresses. A next hop with no up link is `DestNotFound`, and the bind is
    refused. **An endpoint's initial link to its relay is always also an entry in
    the endpoint's `setTopology`**, carrying the relay's `zprAddr` and `peerCn`, so
    F1 can always find the relay. That is the path F6's recovery depends on. *(Coordinator call, builder check.)*
F2. **Binding.** Streams bind hop by hop as today: a `Stream ID Request` for the
    `visa_id` walks down the path, and each Zode fills its forwarding (PFT) entry
    when the answer comes back. Keep the loop guard (egress == ingress is
    refused) and refuse when transaction IDs run out, instead of panicking. #253
    adds the `prev_hop` check: a request is accepted only from the visa's
    previous hop.
F3. **Both sides of a binding are recorded.** Each binding remembers its ingress
    link and its egress link. Then rule 9 (a lost link clears everything tied to
    it) clears entries on **both** sides, so no forwarding entry points at a dead
    or reused link ID. Losing either side **revokes the visa locally**, not just
    its binding. So at the Zode where a flow starts, the next packet finds no visa
    and F6 applies. *(Coordinator call, builder check.)*
F4. **Teardown has exactly three causes.**
    - The visa **expires**.
    - The visa service **revokes** it. Only the visa service tears a binding down
      administratively, and it tells every Zode on the path directly with a VSS
      revoke. A link going down causes this too (#224).
    - A **link drops**: local cleanup under rule 9, a `down` report, then the
      revocation above.
F5. **No Zode-to-Zode teardown messages.** On Zode links, `Stream ID Withdrawal`
    (ZDP type 5) and `Unbind Egress Stream Indication` (type 13) are neither sent
    nor accepted. Zodes don't use them with each other: the existing NodeToNode
    handlers and the `node::unbind_stream` send path are **removed**, not fixed.
    The message types stay for docking sessions.
F6. **Recovery is lazy.** After a revoke, the next packet at the ingress Zode
    finds no visa and sends a `visaRequest`. The visa service plans a route that
    avoids the dead link, usually through the relay. No other recovery signal
    exists. The race where the request beats the `down` report can't happen for
    the Zode that saw the link drop, because its report and its request are
    ordered on one VS-API session. A `down` or `failed` transition is reported
    **at once**, skipping the reporter's coalescing window, and any `visaRequest`
    made after a link loss goes out only after that report has been sent. Other
    Zodes lose their visa only after the visa service already knows the link is
    down. *(Coordinator call, builder check: the 100 ms coalescing would
    otherwise let the request overtake the report.)*
F7. **An unknown Stream ID on a Zode link is dropped silently** and counted.
    This deliberately departs from the RFC's `Destination Unreachable`, for the
    reason the RFC's own editorial note gives: answering would let a compromised
    upstream probe which stream IDs are live. A stale stream ID only shows up
    after a link loss or restart, and F4 and F6 already recover from that.
F8. **One-address endpoint.** A Zode's own traffic, sent from the address it also
    authenticates with, leaves through a Zode link under F1. Expected to need
    tests only.

### Acceptance (#251)
- Unit tests for F1 (egress by topology address, up links only, Hello addresses
  ignored), F2's guards, and F3 on both sides of a link loss. Also a test that
  a withdrawal or unbind arriving on a Zode link is refused and logged.
- Netns, modelled on the sj branch's `two-node-test.sh`: two Zodes on a direct
  link ping across it under their own identities, the link is dropped, and the
  flow re-routes through the relay after the revoke and re-request.

### Order
#251's code and unit tests can land before #252. Its netns acceptance needs
Zodes with their own VS-API sessions, which only #252's relay join provides. So
that scenario moves to #254, after #252, the same way #250 deferred its two-Zode
scenarios. *(Coordinator call, builder check.)*

### Prior art
`org-zpr/zpr-core` branch `sj/link-state-inter-node` routed between nodes the
same way: it filled the peer's actor addresses so `find_egress_link` matched, and
it relied on visa-service revocation for teardown. It took the peer address from
the Hello, which F1 forbids. #250 replaces its link-layer rework.

## Joining through the relay (#252, design approved 2026-11-12, not implemented)

**Bootstrap visas are requested on demand, not stocked ahead of time.** The relay
already has a VS-API session, so it asks for a joining Zode's bootstrap visas
when that Zode's initial link comes up. This is the same idea as a node acting
for an adapter that docks with it. The visa service no longer keeps bootstrap visas
for offline Zodes in topology.

### Steps

J1. **The link comes up.** The joiner dials its configured relay (an initial
    link). The relay verifies the joiner's certificate and accepts the link only
    if that CN is in its topology (rule 4). The joiner starts with an empty VS-API
    visa table.
J2. **The relay requests the bootstrap visas.** Once the link is up, the relay
    sends a `visaRequest` over its own VS-API session for joiner ↔ visa
    service:VSAPI, in both directions. The request names the joiner by its
    **link-verified CN**. The joiner's ZPR address is the relay's topology
    `zprAddr` for that CN, never anything the joiner claims.
J3. **A narrow rule in the visa service.** A relay may request on behalf of a peer
    with no VS-API session only for a CN in that relay's topology, and only to
    the visa service's VSAPI port. The visa service mints these visas from the
    policy-declared peering, the same authorization `visa_bootstrap.rs` uses today.
    Only the timing changes: they are minted on request instead of pushed ahead of
    time. They are **short-lived**: they only have to last through
    authentication.
J4. **The hand-off.** The relay sends the two visas in the `Bootstrap Visas`
    message right after Hello. The joiner keeps the strict checks: initial link
    only, no VS-API session yet, exactly two visas, the right five-tuple, Active,
    and never inside Hello.
J5. **Authentication.** The joiner authenticates as a node over the bootstrap
    visas, or reconnects (`reconnect` vs `reset`: the visa service decides, R16).
    - **J5a.** When it succeeds, the visa service issues the Zode its **own VS-API
      visas** (Zode ↔ visa service:VSAPI, both directions), bound to the
      authenticated node, with a normal lifetime. The Zode switches to them at
      once.
    - **J5b.** On `registerVss`, the visa service issues the **VSS visa** (visa
      service → Zode's VSS port), as today.
    - **J5c.** The bootstrap visas are left to expire. They only carried the
      authentication exchange.
J6. **Every return is a join** (rule 11a). A new initial link means a new request
    and a new hand-off. Nothing waits in the visa service for an offline Zode, so
    bootstrap visas expiring while a Zode is away is never a problem.
J7. **If the relay can't reach the visa service**, it can't request visas. It
    closes the link instead of handing off, and the joiner retries with its
    initial-link backoff.
J8. **`Link.visas` is removed from the schema.** Topology no longer carries
    bootstrap visas: vsapi, then a common tag, then the pins move together (BUILD.md
    rule 6).

### Settled after the builder check *(coordinator calls)*

- **The joiner's egress before it has topology.** Bootstrap visas are bound to
  the initial link they arrive on. The joiner records the relay's ZPR address
  from the forward bootstrap visa's next hop, as that link's peer address.
  This is the one exception to F1's "address from topology", and it holds only
  until the Zode's first `setTopology`, which then includes the relay (F1).
- **Who may request on someone's behalf (J3).** In v1 that is only **the node the
  visa service docks on**: one relay, one hop from the visa service. This
  matches `visa_bootstrap.rs` and g44 q3. A Zode reached over a direct link never
  qualifies, so the non-transit rule holds. Multiple relays, and relays that
  don't host the visa service, are deferred.
- **A rejoin before the visa service has noticed the old session is gone.** The
  relay's on-behalf request is the proof: one link per peer (rule 5) means the
  relay holds no other live link for that CN. So the visa service tears down
  the old session state and mints the bootstrap visas, instead of refusing.
- **`reconnect` after a link wipe.** A rejoin may still be `reconnect`: the
  Zode's identity and local actors survive. But the visa service treats every
  visa it pushed to that Zode as **not installed** and pushes them again,
  including the VSS visa. Rule 9 cleared them on the Zode.
- **The `Bootstrap Visas` message** is a new ZDP control message (core#104 used
  type 146). It gets defined in ZDP.md, with the message body: exactly two
  visas.

### Keeping the core visas alive (operator approved 2026-11-12)

The three core visas from J5a and J5b are what the Zode needs to talk to the visa
service at all. If they lapse, the Zode can't send a `visaRequest` (that needs
VSAPI), and the visa service can't push anything (that needs VSS). So F6's
"re-request on the next packet" cannot renew them.

J9. **The visa service renews them.** It re-mints a Zode's three core visas
    before they expire, at about two-thirds of their lifetime, and pushes them
    over VSS while the current VSS visa is still valid. The Zode installs the new
    ones and lets the old ones expire.
J10. **Fallback: a broken session means rejoin.** If a Zode's VS-API session
    fails for any reason, including its core visas lapsing, it closes its
    initial link and dials it again. That is J1–J6: fresh bootstrap visas, then
    reconnect or re-authentication. There is always a way back, and only one
    recovery path to test.

### Moving a live connection onto newer visas (operator: supersede and rebind)

**Core tenet: visas are immutable** (operator). Once granted, a visa is never changed: not its expiry, not its binding to an actor, nothing. Renewing or upgrading a visa always means issuing a **new** visa, and the old one is revoked or left to expire. A visa ID from the visa service always names the same visa: a Zode never compares two visas with the same ID, and when asked to install an ID it already holds it drops the incoming visa and answers success, so install is idempotent for the visa service (operator, gate g88 q4).

J11. The visas from J5a (after authentication) and J9 (renewal) have the **same
     five-tuple** as visas the Zode already uses for a live VS-API or VSS
     connection. A newer visa for an identical five-tuple **supersedes** the
     older one. Each Zode on the path moves the existing stream binding onto the
     new visa ID **atomically**: a `Stream ID Request` for the new visa ID goes
     down the path, and each hop swaps its forwarding entry once the answer
     comes back, so packets are never left without a binding. Only then does the
     older visa stop being used, and it expires on its own. If the rebind fails
     at any hop, the old binding stays until it expires. If that kills the
     session, J10's rejoin recovers.
     This is general: it applies to any same-five-tuple visa replacement, not
     only the core visas.

### Open for the planning gate
- How the on-behalf-of identity is carried: a new optional `visaRequest` field
  (for example `onBehalfOfCn`) or a separate VS-API call.
- The bootstrap visa lifetime (short, for example a few minutes) and the J9
  renewal threshold.

## Left to the builder

- The link-up timeout: 5 s is the default (my call). It is a constant with a comment.
- The initial-link backoff schedule: start around 1 s, double, cap around 30 s,
  with jitter (my call).
- Where the per-link state machine lives in `ph`, and its exact state names. The
  states have to cover at least: dialing, keying, CN check, up, closing, closed.
  Every transition gets a unit test, and the rules above are the acceptance criteria.
- How rules 9 and 12 find "state tied to a link". Recommended: hang everything
  link-scoped off one per-link object, so dropping it clears everything.

## Implementation status

Built on `feature/zodes` in three core PRs (zl-zpr-core#105 keying and
acceptance, #106 lifecycle, and the reporting PR), with the visa-service half in
zl-zpr-visaservice#66 and the schema in zl-zpr-vsapi#5 and zl-zpr-common#14
(tag `zl-v0.32.0-zodes.3`).

### Where the code lives

| Concern | Location |
|---|---|
| Who owns a link (`link_owner`), acceptance after keying (`check_keyed_peer`, rules 3 and 4), replacing an established link (rule 5) | `zl-zpr-core/adapter/ph/src/zode_links.rs` |
| The per-peer link record and its state machine (rules 6–8, 10, 11, 13): kind, owner, dial address, current connection, state, timers | `zl-zpr-core/adapter/ph/src/zode_link_record.rs` |
| Driving the records: connection events, `setTopology` (rules 12, 13), dials, closes and timers; the link-state snapshot builder (`ZodeLinkTable::snapshot`) and reporter (`run_link_state_reporter`) | `zl-zpr-core/adapter/ph/src/zode_links.rs` |
| The per-connection state machine (`LinkType::NodeToNode`, `ZodeEnd::Dialer`/`Answerer`) | `zl-zpr-core/adapter/ph/src/link_state.rs` |
| Recognising an inbound Zode handshake | `zl-zpr-core/adapter/ph/src/mgmt/dispatch.rs`, `km_noise.rs` (`ZODE_HANDSHAKE_MARKER`) |
| `reportLinkStates` RPC; per-link `setTopology` decoding | `zl-zpr-core/libnode2/src/vsconn.rs`, `libnode2/src/vss.rs` (`decode_topology_links`) |
| Rule 14 (`end_agrees`), rule 13a (failed pairs, `record_failures`, `clear_failed_pairs_for`) | `zl-zpr-visaservice/vs/src/link_state.rs`, `vs/src/topology_mgr.rs`, `vs/src/vsapi_worker.rs` |
| netns test: a relay, a Zode with an initial link to it, and a Zode outside the relay's topology | `zl-zpr-core/integration-test/zode-link-test.sh`, `integration-test/lib/zode_funcs.sh` |

Everything scoped to one connection hangs off that connection's peer-table
entry, so dropping the entry clears it (rule 9). A record holds only the
connection's id. #251's stream bindings are the exception: they live beside
their visa in the visa table, and `drop_peer` revokes every visa bound over the
lost connection (F3). #252 hangs its bootstrap visas off the peer entry.

### Constants

All in `zl-zpr-core/adapter/ph/src/`, each with a comment:

- `LINK_UP_WINDOW` = 5 s, the owner's link-up window (rule 6), `zode_link_record.rs`.
- `OWNER_REDIAL_INTERVAL` = 1 s, the owner's redial inside the window (rule 6).
- `INITIAL_LINK_BACKOFF_MIN` = 1 s, doubling to `INITIAL_LINK_BACKOFF_MAX` = 30 s,
  each delay jittered to between half and all of it (rule 11).
- `ZODE_KEYING_TIMEOUT` = 2 s and `ZODE_HELLO_TIMEOUT` = 2 s, `config.rs`. Together
  they fit inside the window, which a compile-time assertion checks, so an
  attempt ends on its own before the window does. When the window ends, the
  owner also closes any attempt still in flight.
- `REPORT_COALESCE` = 100 ms: report requests that close together go out as one
  snapshot, `zode_links.rs`.

### Configuration

- **Initial link.** `[node.initial_link]` with `addr = "IP:port"` (the relay's
  substrate address) and `cn = "<relay CN>"`, or `ph node --initial-link
  CN@IP:PORT`, which overrides the file. At most one. Validation rejects an empty
  CN, an unspecified IP and port 0. Node only.
- **Relay.** `relay = true` in `[node]`, or `ph node --relay true`. Default off.
  A relay has no initial link: setting both is a config error.
- A Zode's own address, the one ownership compares, is `zpr_addr[0]`.

### Wire

- **Handshake marker.** A dialing Zode puts `ZODE1` in the plaintext payload of
  the first Noise XX message. The node it reaches reads the marker before any
  link exists, so it creates a Zode link instead of an adapter dock. Noise mixes
  the payload into the handshake hash, so a marker changed in transit breaks the
  handshake. See [ZDP.md](ZDP.md), "Zode links".
- **Hello.** The dialer sends the HelloRequest. The answerer replies with none
  of the adapter-dock TLVs (`AAA`, `OIDC_IDP`, `VS_BOOTSTRAP_LINK`), and a Zode
  HelloResponse that carries any of them closes the link.
- **`setTopology`.** A `Link` without `zprAddr` or with an empty `peerCn` is
  logged and skipped, and the rest are applied. The call still answers `ok`.
- **`reportLinkStates`.** Snapshots follow "Reporting summary" and "Reporting
  contract". A snapshot goes out after every change to the listed set, after
  every VS-API registration (`registerVss`), and never while there is no VS-API
  session: VSConn refuses the call locally and nothing goes on the wire. The
  next registration asks for a fresh one.

### Deviations from the rules as written

- **Rule 5 (replace).** The new inbound link keys alongside the established one.
  The old link is closed only once the new link's CN has passed rule 3, so a
  stray or refused dial cannot take down a working link. A redial from the
  same substrate address takes over that address only then (gate g78 q5).
- **Rule 13b** holds today for addresses from policy: the visa service
  proposes a link only to a peer whose substrate address policy gives and
  skips the rest. Using an address the peer reported itself is #227 and #255.

### Forwarding over Zode links (#251)

Built on `feature/zodes` in two core PRs: zl-zpr-core#109 (F1, F2, F5, and the
F7 and F8 tests) and the F3/F6 PR (merge set B with this document). Covered by
unit tests and core CI; the netns scenario (two Zodes ping across a direct
link, the link drops, the flow re-routes through the relay) is **#254**,
because it needs #252's relay join.

| Rule | Location (`zl-zpr-core/adapter/ph/src/`) |
|---|---|
| F1 egress to a Zode: an up link record whose verified CN's topology `zprAddr` is the next hop; adapter matching (`actor_auth`, AAA) on non-Zode links only | `assembly.rs::find_egress_link` |
| F1 warning when a `setTopology` lacks the initial link's CN | `zode_links.rs::apply_topology` (`initial_link_missing_from_topology`) |
| F2 loop guard (egress == ingress is refused), and a refusal when transaction IDs run out. F2's `prev_hop` check is **not** implemented: it is #253 | `mgmt/node.rs::requested_visa_granted` |
| F3 bindings beside the visa (`StreamBinding { ingress, egress }`), recorded by `requested_tether_granted`; `revoke_for_link` revokes on either side, except the node's built-in visa-service visas (IDs below `MIN_VISA_ID`). Rule 9's cleanup of VS-API and bootstrap visas carried over an initial link comes with #252 | `visa_table.rs` (`VisaTable::bind`, `revoke_for_link`), `mgmt/node.rs` |
| Rule 9 for pending binds: binds waiting on the lost link are refused, next-hop binds issued for it are abandoned | `mgmt/node.rs::clear_pending_binds_for_lost_link`, called from `assembly.rs::drop_peer` |
| F5 types 5 and 13 refused on Zode links; none sent on one | `mgmt/handlers.rs` (`handle_stream_id_withdrawal`, `handle_unbind_indication`), `assembly.rs::withdraw_streams` |
| F6 `down`/`failed` skip the coalescing window; the loss barrier | `zode_links.rs` (`register_loss`, `wait_for_loss_reports`, `run_link_state_reporter`), `mgmt/node.rs` (`visa_request_task`, `visa_push_request_task`) |
| F7 unknown Stream ID dropped and counted (`UnknownStreamId`) | `fastpath.rs::forward` |
| F8 a Zode's own traffic out a Zode link | `mgmt/node.rs` (test `test_own_traffic_binds_out_a_zode_link`) |

**The tenet in code.** `visa_table::Visa` has only private fields, set once
when the visa is inserted; nothing hands out `&mut Visa`. Stream bindings live
in a separate table in `VisaTable`, under the same lock. `bind` is the only
writer that adds one; revocation, expiry and link loss all remove a visa's
bindings, with their PFT entries, through one function. `insert_visa` of a held
ID returns success before it parses anything, so a duplicate leaves no trace.
#252's J11 adds one `rebind(old, new)` under the same lock: it moves a binding
onto the superseding visa's ID and swaps the `visa_id` of the ingress PFT entry,
replacing the visa without editing either one.

Decided while building it:

- **The F6 barrier releases when the report's RPC returns**, Ok or error. With
  no VS-API session the report fails locally at once, so a visa request never
  hangs on it. Each send is bounded by the RPC timeout, but snapshots go out one
  at a time, in order, so the loss report can also wait behind earlier sends
  still queued (at most `REPORT_QUEUE_MAX`, each bounded the same way). A
  waiting request rechecks the current loss epoch whenever the reported epoch
  moves, so a second loss registered while it waits holds it until that report
  has returned too (core#110 review). Each snapshot carries
  the loss epoch read just before it was taken; the reporter publishes it once
  the send returns.
- **A loss is registered before any revocation.** `drop_peer` registers it
  first, then revokes and withdraws, so a visa request that a withdrawal
  triggers already waits. Only a connection serving an up link record counts as
  a loss: a dial attempt that never came up, or a connection a newer one
  replaced (rule 5), revokes its visas but sends no extra report.
- **A late answer to an abandoned next-hop bind is ignored.** When the bind it
  served is gone (its link was lost, it was refused, or the requestor reused
  its transaction ID), the next-hop transaction stays open and its answer is
  dropped. Refusing it as an unknown transaction would close the next hop's
  link, which did nothing wrong. The consistency panics on that path are gone.
- **A visa request's result is dropped** unless the bind request it serves is
  still there and still names that request's worker task, so it never answers
  a peer that took the link ID since, or a newer request that reused the
  transaction ID (core#110 review).
- Gate g88: **q2** a type 5 or 13 on a Zode link is refused through the strict
  handler-error path, which closes the link. **q3** a type 13 from a node's own
  adapter or local actor is refused too. **q4** a held visa ID is an idempotent
  no-op with no content comparison (the tenet above).

### What waits for which issue

- **#252** relay join: J1–J11, rule 11a (the bootstrap hand-off on every join)
  and rule 16 (the VS-API session over the initial link). Designed and
  approved above; none of it is implemented. Until then only the Zode that
  hosts the visa service has a VS-API session. Other Zodes link to it but cannot
  receive `setTopology` or report, so the netns test reports from the relay's
  side only, and no flow can be set up across a Zode link outside unit tests.
- **#253** gap 7: the `prev_hop` check on `Stream ID Request` (F2). Today a
  Zode accepts a request from any node link.
- **#254** netns scripts, including #251's re-route scenario.
- **#227** `updateSubstrateAddr`: once implemented, it calls
  `TopologyMgr::clear_failed_pairs_for` as a second rule-13a trigger. Today only
  a VS-API (re)connect (`authenticate`, `open`) clears a node's failed pairs.
- **v2**: dialing a Zode that can't be reached (ZBOLT, rendezvous), background
  retries of direct links.

## Related

- #251 node-to-node forwarding, #252 relay join, #253 gap 7, #254 netns
  scripts. These build on this lifecycle.
- #242 (removing "never reported = up") is folded into rule 14.
- v2: dialing a Zode that can't be reached, ZBOLT and rendezvous, retries.
