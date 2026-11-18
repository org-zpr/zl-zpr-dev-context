# Zode links: lifecycle (v1)

**Status:** Implemented for zl-zipline#250 (rules 1–15 except 11a) and for
#251, forwarding over Zode links (F1–F8; F2's `prev_hop` check is #253's,
zl-zpr-core#115), on `feature/zodes`. Rules 11a and 16 and the relay join (J1–J11) are
implemented for #252: the hand-off codec and checks (zl-zpr-core#111) and J11
(zl-zpr-core#112) are merged; the relay and joiner (zl-zpr-core#113), the visa
service's half (zl-zpr-visaservice#67) and the compiler's pin
(zl-zpr-compiler#26) are #252's merge set C, which lands together with this
document. See "Implementation status" for where the code lives and what it
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
- Unbounded background retries of direct links. A direct link reported `down`
  gets **one** re-proposal from the visa service (rules 10, 13 and 13a,
  zl-zipline#268). If that attempt fails, the link stays down until something
  about the pair changes. The Zode itself never retries a topology link.
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

`Link.peer`, the substrate address the owner dials, is **optional** since #255.
Unset means the receiving Zode never dials this link: it is the relay, or the link
is the one it reaches through its own initial link. A Zode that owns a link with
no `peer` logs it and skips it, and applies the rest (rule 13b).

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
   link isn't up within **10 seconds** (default, see "Left to the builder"), it
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
10. For a topology link, each side reports `down`. The owner does **not** redial:
    the Zode keeps no retry logic. Instead, the visa service sends `setTopology`
    to both ends again `LINK_REPROPOSE_DELAY` (5 s) after the first `down` report,
    and the owner retries under rule 13 (zl-zipline#268; see rule 13a for the
    bound).
11. For the initial link, the endpoint redials with backoff until it comes back.
    The relay does nothing except clear the state, per rule 9.
11a. **Every return to the relay is a join.** Each new initial link goes through the
    same join hand-off as the first one (#252): after Hello, the relay asks the
    visa service for the endpoint's bootstrap visas and passes them on (J2–J4).
    The visa service mints new ones on every request, and each request ends
    whatever the endpoint's previous join left behind (J6). Only then does rule
    16 apply.

### Topology changes

12. A link removed from `setTopology` is closed, and its state cleared per rule 9.
    It is no longer reported.
13. **Every `setTopology` retries** each `failed` or `down` link it still contains:
    the owner makes one more attempt under rule 6. Links that are up, or still in
    their link-up window, are untouched, and the Zode keeps no other retry logic.
    Such a `setTopology` comes from a policy install, a peer's VS-API session
    start, or, since zl-zipline#268, the visa service's re-proposal of a link
    reported `down` (rule 13a).
13b. **The visa service proposes a direct link only when it can work.** It needs a known substrate address for the peer the owner must dial, from policy or reported by that peer. No address means no direct link, and traffic goes through the relay. The visa service never asks a Zode to make a connection it can't possibly make. *(Operator, 2026-11-12; #255 drops visaservice#65's `[::]:0` fallback.)* In detail (gate g115 q2-q4):
    - A node's **known address** is its policy substrate address (DNS-resolved).
      Failing that, it is the address the node reported in its last `connect` or
      `open`, stored on its node actor. Nothing else counts, and nothing is
      guessed. Only the node's next `connect` or `open` replaces a reported
      address; a rejoin keeps the actor, so the old address stays known until
      then (see "Known limit" under "Links from known addresses (#255)").
    - A **relay link**, one end of which is the relay, is always proposed: the
      endpoint dials its configured initial link, and the relay must hold the
      joiner's CN before the joiner connects (rule 4, J2). `peer` is the other
      end's known address, or **unset** when there is none. The relay never dials,
      so it never needs one.
    - A **direct link**, between two endpoints, is proposed only when **both** ends
      have a known address. Each end's `peer` is the other end's address. The visa
      service does not work out which end owns the link: that stays the Zodes' rule.
    - When a node's VS-API session starts, its address may just have become known,
      so the visa service sends `setTopology` to each of its policy peers that has
      a live VSS connection. Nothing is sent when a session ends. Once a node
      with no policy address has disconnected and its actor is gone, the next
      topology built for any reason leaves out its direct links; within a
      rejoin, its old address may still be proposed until the new `connect` or
      `open`. A node with a policy address keeps its direct links whether or not
      it has a session: its address is known from policy.
    - As built, "can work" means a **known, dialable** address: a specific IP
      and a non-zero port, from policy or from a report. The visa service cannot
      check that the address is reachable. An unreachable or stale address (the
      retained address above) gives a failed dial, and the pair falls to rule
      13a.
    - Whether a Zode can misuse the address it reports is
      [#264](https://github.com/org-zpr/zl-zipline/issues/264). A wrong address
      only steers where a peer dials: the link still needs the reporter's CA-signed
      certificate with the expected CN (rule 3).
13a. **The visa service owns the retry policy.** It remembers links reported
    `failed` and leaves them out of the topology it proposes until something about
    that pair changes: a peer reconnects, or in v2 a ZBOLT/rendezvous step says
    the pair can try again. A peer's substrate address changes only that way:
    it is fixed for the life of a VS-API session, and a new one arrives with a
    rejoin's `connect` or `open`. (`updateSubstrateAddr` was removed from the
    schema in #252, after #223 was closed.) A `failed` report never itself causes a topology send,
    so the two can't loop. Dropping the link from topology closes it under rule 12.
    A policy install is not a pair change: it keeps the marks.
    This is also the v2 hook: once both sides have holes open, the visa service just
    proposes the link again.

    **A `down` report gets one re-proposal** (zl-zipline#268, visaservice#71).
    When either end of a direct link newly reports it `down` (its previous
    snapshot did not list the peer as `down`), the visa service waits
    `LINK_REPROPOSE_DELAY` (5 s) and then sends `setTopology` to **both** ends.
    The other end's `down`, and any repeated `down`, within the delay coalesce
    into that one send. When the delay ends, the visa service checks again and
    sends nothing if the pair has been marked failed, the link is up again, policy
    no longer declares it, or either end has no live VSS connection (that end is
    rejoining, and its session start proposes its links anyway). Relay links are
    left out: the endpoint redials its initial link itself (rule 11).

    **The bound.** Each re-proposal needs a new `down`, and a link reports `down`
    only after it was `up`. A failed attempt ends in `failed`, which marks the
    pair, and then nothing is proposed again. So one real `up` buys at most one
    retry, and the Zode and the visa service cannot loop.

### Routing on link state

14. The visa service routes over a topology link only when **both** ends report it
    `up`. A link with no report counts as **down**. This replaces #224's
    transitional "never reported = up" rule; #242 is folded into this work.
15. `down` or `failed` revokes visas over that link, as #224 does today. New visa
    requests then take another path, usually through the relay.

### VS-API session

16. The VS-API session runs over the initial link. When it is lost, the Zode
    reconnects as `VSConnT::reconnect` once its initial link is back. **The visa
    service decides** whether that is enough: it may answer `outOfSync` or
    `invalidOperation`, and then the Zode clears its state and connects as `reset`,
    re-authenticating the machine and user. After either, the visa service re-sends
    topology and anything else the Zode needs. *Built:* "clears its state" means
    the Zode resets every link except the one carrying the VS-API session it is
    resetting, then revokes every visa except those for its own VS-API flows. On
    a relay, each joiner on the other links redials and gets a new hand-off.

## Walkthroughs

**Laptop switches networks.** The laptop's old address goes away, so its
initial link stops working. There is no network-change listener: the
keep-alive notices, as for any dead link. After 3 missed keep-alives, about
12 s, the laptop closes the initial link and clears its state (rule 9). Until
then, sends from the old address fail, and each one is dropped and counted;
none stops `ph`. The laptop keeps redialing the relay with backoff (rule 11).
A laptop Zode has a wildcard `self_addr` and no `advertised_substrate_addr`,
so each dial takes the local address the OS picks for the relay, which is the
new network's. (An advertised address, or a concrete `self_addr`, is reported
as it is and does not follow the change.) Meanwhile the relay sees a dead
link, clears its state and waits. The new initial link goes through the join
hand-off again (rule 11a). The laptop then reconnects its VS-API session (rule
16), and the visa service either resumes it or demands a fresh login. Either
way, the `connect` or `open` reports the initial link's new local address as
the laptop's substrate address. Policy gives a laptop no substrate address, so
the visa service keeps the reported one (rule 13b), and the relay's next
topology carries it. Topology arrives again. Direct links that
were up before are attempted once by their owners (rule 13).

**Two reachable Zodes get a direct link.** Both receive `setTopology` naming each
other. The lower ZPR address dials (rules 1 and 6), the higher one accepts it (rule
3), and both report `up` (rules 7 and 8). The visa service starts routing over it
(rule 14).

**The higher-address Zode dials anyway**, from a bug or a stale config. The lower
one refuses it after keying because the dialer isn't the owner (rule 3). No
duplicate is possible.

**A direct link can't come up** (a firewall, say). After 10 seconds the owner
reports `failed` (rule 6). Traffic goes through the relay. The visa service stops
proposing that link (rule 13a) until something about the pair changes. When it does
propose the link again, the owner tries once more (rule 13). If the link was up and
then went `down` behind a lasting block, the visa service proposes it once more
after `LINK_REPROPOSE_DELAY`; that attempt fails, and the pair stays on the relay
from then on, as above. A policy install does not bring it back; a restart or
rejoin of either end does. `zode-reroute-test.sh` steps 3 and 4 run this case.

**A direct link blips.** The substrate between two Zodes drops for a few seconds.
Keep-alive gives up after about 9-12 s, and both ends report the link `down` (rule
10). The visa service revokes the visas over it (rule 15), and their flows
re-request through the relay. `LINK_REPROPOSE_DELAY` (5 s) after the first `down`
report, the visa service sends `setTopology` to both ends (rule 13a). The owner
redials about once a second for its 10 s window (rule 6). If the outage has cleared,
the link comes back, both report `up`, and new flows take it again (rule 14). No
policy install or restart is needed. This works for an outage that clears within
about `LINK_REPROPOSE_DELAY` + `LINK_UP_WINDOW` (15 s) of the first `down`. A longer
one ends in `failed`, as in the walkthrough above. `zode-reroute-test.sh` step 5
runs this case.

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
  without the link coming up** (the 10 s window ran out). Once a retry starts after a
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
    The one exception is a joiner until a `setTopology` names its relay: its
    initial link's peer address is the relay address its bootstrap hand-off
    recorded
    (see "The joiner's egress before it has topology" below). F2's
    previous-hop check uses this same lookup, exception included.
    `find_egress_link` matches these links just as it matches local and adapter
    addresses. A next hop with no up link is `DestNotFound`, and the bind is
    refused. **An endpoint's initial link to its relay is always also an entry in
    the endpoint's `setTopology`**, carrying the relay's `zprAddr` and `peerCn`, so
    F1 can always find the relay. That is the path F6's recovery depends on. *(Coordinator call, builder check.)*
F2. **Binding.** Streams bind hop by hop as today: a `Stream ID Request` for the
    `visa_id` walks down the path, and each Zode fills its forwarding (PFT) entry
    when the answer comes back. Keep the loop guard (egress == ingress is
    refused) and refuse when transaction IDs run out, instead of panicking. A
    request that arrives on a Zode link is accepted only from the visa's
    **previous hop** (#253): the visa's `prev_hop` must be set, and F1's
    egress lookup for that address must give exactly the link the request
    came in on. So the address is F1's: the topology `zprAddr` of the entry
    matched by the link's verified CN, or, on a joiner until a `setTopology`
    names its relay, the relay address its hand-off recorded (J4). The link's
    record must be up and point at this connection. An unset `prev_hop`, or a
    link with no address, is refused, with no fallback. A refusal is an error
    `Stream ID Response`; the link stays open. It is logged with its reason
    and counted (`StreamIdRequestRejected`, which counts every refused
    `Stream ID Request`, whatever the reason). An adapter or the local actor
    is a stream's ingress and has no previous hop, so the check does not
    apply to it.
F3. **Both sides of a binding are recorded.** Each binding remembers its ingress
    link and its egress link. Then rule 9 (a lost link clears everything tied to
    it) clears entries on **both** sides, so no forwarding entry points at a dead
    or reused link ID. Losing either side **revokes the visa locally**, not just
    its binding. So at the Zode where a flow starts, the next packet finds no visa
    and F6 applies. *(Coordinator call, builder check.)*
F4. **Teardown has exactly three causes.**
    - The visa **expires**. *Built (#271):* a Zode enforces expiry when it
      matches a new flow to a visa, not on a timer; see "Visa expiry at
      match time (#271)" under "Implementation status".
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
    upstream probe which stream IDs are live. A stale stream ID shows up after
    a link loss or restart, or on a healthy link when expiry or revocation
    removed the downstream entry before the upstream stopped sending; F4 and
    F6, and the upstream's own expiry, already recover from all of these.
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

## Joining through the relay (#252, design approved 2026-11-12, implemented)

**Bootstrap visas are requested on demand, not stocked ahead of time.** The relay
already has a VS-API session, so it asks for a joining Zode's bootstrap visas
when that Zode's initial link comes up. This is the same idea as a node acting
for an adapter that docks with it. The visa service no longer keeps bootstrap visas
for offline Zodes in topology.

The steps below are the approved design. Where the code does something more
specific, or something different, a *Built:* note says so. "Implementation
status" says where the code lives.

### Steps

J1. **The link comes up.** The joiner dials its configured relay (an initial
    link). The relay verifies the joiner's certificate and accepts the link only
    if that CN is in its topology (rule 4). The joiner starts with an empty VS-API
    visa table. *Built:* the relay, which has no initial link, keeps its
    self-granted VS-API pair (IDs 1 and 2) (`VisaTable::initial_for_node`).
    Since #229 the relay's session to its own visa service runs over `lo`, so
    this pair matches no packet; it is kept on purpose (gate g117 q5, see "The
    relay hosts the visa service (#229)").
J2. **The relay requests the bootstrap visas.** Once the link is up, the relay
    asks over its own VS-API session for joiner ↔ visa service:VSAPI, in both
    directions. The request names the joiner by its **link-verified CN**. The
    joiner's ZPR address is the relay's topology `zprAddr` for that CN, never
    anything the joiner claims. *Built:* the request is a separate VS-API call,
    `requestBootstrapVisas(peerCn)`, not a `visaRequest` (gate g89 q2). The
    answer holds two lists of two visas: `peer` (the joiner's copies) and
    `relay` (the relay's own). The relay waits at most
    `BOOTSTRAP_REQUEST_TIMEOUT` (5 s).
J3. **A narrow rule in the visa service.** A relay may request on behalf of a peer
    with no VS-API session only for a CN in that relay's topology, and only to
    the visa service's VSAPI port. The visa service mints these visas from the
    policy-declared peering, the same authorization `visa_bootstrap.rs` used
    for the stocked visas. Only the timing changes: they are minted on request
    instead of pushed ahead of time. They are **short-lived**: they only have to
    last through authentication. *Built:* the lifetime is 5 minutes
    (`BOOTSTRAP_VISA_LIFETIME`, gate g89 q3). The visas carry no session keys.
    Their path is stitched from the peering, `[joiner, relay]`, not routed. They
    are recorded as installed on both Zodes at once, so ingress-last delivery
    (J5a) never holds them back. The refusals and their order are in
    VISA_SERVICE.md, "VS-API calls added for Zodes".
J4. **The hand-off.** The relay sends the two visas in the `Bootstrap Visas`
    message right after Hello. The joiner keeps the strict checks: initial link
    only, no VS-API session yet, exactly two visas, the right five-tuple, Active,
    and never inside Hello. *Built:* ZDP type 146, defined in
    [ZDP.md](ZDP.md) ("Bootstrap Visas"). The relay installs its own copies,
    bound to the link (rule 9), before it sends the joiner's, so the joiner's
    first packet already matches at the relay. The joiner installs both visas
    bound to the link, and a refused hand-off closes the link. *Built (#253,
    gate g106 q4):* the joiner also refuses a pair whose reverse visa's
    `prev_hop` is unset or is not the forward visa's next hop, the relay
    address the hand-off records (`HandoffError::PrevHopMismatch`). The relay's
    `Stream ID Request` for that visa would otherwise be refused at bind time
    (F2), a silent J10 rejoin loop.
J5. **Authentication.** The joiner authenticates as a node over the bootstrap
    visas, or reconnects (`reconnect` vs `reset`: the visa service decides, R16).
    *Built:* the joiner connects only once a hand-off is installed, and after a
    rejoin only once a new one is. It reports its link states right after
    `connect`, before `registerVss`, and retries `registerVss` on `internal` up
    to 5 times, 1 s apart: its link to the relay is routable only once both ends
    have reported it (rule 14).
    - **J5a.** When it succeeds, the visa service issues the Zode its **own VS-API
      visas** (Zode ↔ visa service:VSAPI, both directions), bound to the
      authenticated node, with a normal lifetime. The Zode switches to them at
      once. *Built:* they are minted through the ordinary visa-request pipeline
      on `authenticate` and on `open`, and they reach the Zodes by VSS push, not
      in the call's answer. Each Zode on the path except the ingress gets its
      copy first, and the ingress copy goes last, once the others have acked
      (gate g89 q4). "Switches at once" is J11: the new visa supersedes the
      bootstrap visa, and the ingress rebinds the live stream onto it. A failed
      issue is retried every second, for the same session only, until it
      succeeds, the session is replaced, or `BOOTSTRAP_VISA_LIFETIME` has
      passed. The relay that hosts the visa service gets J5a visas too (gate g89
      q7). The visas carry no session keys (see "Design decisions").
    - **J5b.** On `registerVss`, the visa service issues the **VSS visa** (visa
      service → Zode's VSS port), as today. *Built:* a new VSS visa on every
      `registerVss`, even when an older one is still installed. The answer to
      `registerVss` carries only the Zode's copy of it.
    - **J5c.** The bootstrap visas are left to expire. They only carried the
      authentication exchange. *Built:* issuing a core visa never revokes
      the visa it supersedes (see J9). On the Zodes the bootstrap visas also go
      when the link they arrived on goes (rule 9).
J6. **Every return is a join** (rule 11a). A new initial link means a new request
    and a new hand-off. Nothing waits in the visa service for an offline Zode, so
    bootstrap visas expiring while a Zode is away is never a problem. *Built:*
    each request first ends the joiner's previous join in the visa service: it
    revokes every visa between the joiner and the visa service, and if the
    joiner still has a VS-API session, it ends that session (see "Settled after
    the builder check").
J7. **If the relay can't reach the visa service**, it can't request visas. It
    closes the link instead of handing off, and the joiner retries with its
    initial-link backoff. *Built:* so does any refusal, a timeout, a failed
    install of the relay's own copies, and an answer that cannot be sent in one
    message. There is no retry. An answer for a link that dropped while the
    request was in flight is dropped.
J8. **`Link.visas` is removed from the schema.** Topology no longer carries
    bootstrap visas: vsapi, then a common tag, then the pins move together (BUILD.md
    rule 6). *Built:* vsapi#6 and common#15, tag `zl-v0.32.0-zodes.4`. The
    capnp ordinals after each removed member were renumbered, with no
    placeholders and no backwards compatibility (gate g89 q6).

### Settled after the builder check *(coordinator calls)*

- **The joiner's egress before it has topology.** Bootstrap visas are bound to
  the initial link they arrive on. The joiner records the relay's ZPR address
  from the forward bootstrap visa's next hop, as that link's peer address.
  This is the one exception to F1's "address from topology", and it holds only
  until the Zode's first `setTopology`, which then includes the relay (F1).
  *Built:* once a `setTopology` names the relay, the exception ends for the
  life of the node, across redials. If the two addresses differ, the topology
  wins, with a warning.
- **Who may request on someone's behalf (J3).** In v1 that is only **the node the
  visa service docks on**: one relay, one hop from the visa service. This
  matches `visa_bootstrap.rs` and g44 q3. A Zode reached over a direct link never
  qualifies, so the non-transit rule holds. Multiple relays, and relays that
  don't host the visa service, are deferred.
- **A rejoin before the visa service has noticed the old session is gone.** The
  relay's on-behalf request is the proof: one link per peer (rule 5) means the
  relay holds no other live link for that CN. So the visa service tears down
  the old session state and mints the bootstrap visas, instead of refusing.
  *Built (gate g89 q5):* the old session's handles fail from then on, its VSS
  worker is stopped, and its link-state reports are forgotten, so its links are
  down until it reports again (rule 14). The node actor and its adapters stay,
  so a `reconnect` can still succeed (R16).
- **`reconnect` after a link wipe.** A rejoin may still be `reconnect`: the
  Zode's identity and local actors survive. But the visa service treats every
  visa it pushed to that Zode as **not installed** and pushes them again,
  including the VSS visa. Rule 9 cleared them on the Zode. *Built differently
  for the core visas:* the previous join's VS-API visas and VSS visas are
  revoked, not pushed again. The rejoin brings new ones (J5a, and J5b on the
  next `registerVss`). Every other visa is marked not installed and pushed
  again. The comment on `requestBootstrapVisas` in `vs.capnp` still says the
  VSS visa is pushed again.
- **The `Bootstrap Visas` message** is a new ZDP control message, type 146
  (the value core#104 used), defined in [ZDP.md](ZDP.md). Its body holds
  exactly two visas.

### Keeping the core visas alive (operator approved 2026-11-12)

The three core visas from J5a and J5b are what the Zode needs to talk to the visa
service at all. If they lapse, the Zode can't send a `visaRequest` (that needs
VSAPI), and the visa service can't push anything (that needs VSS). So F6's
"re-request on the next packet" cannot renew them.

J9. **The visa service renews them.** It re-mints a Zode's three core visas
    before they expire, at about two-thirds of their lifetime, and pushes them
    over VSS while the current VSS visa is still valid. The Zode installs the new
    ones and lets the old ones expire. *Built:* the check runs on every VSS
    worker heartbeat (800 ms). For each core five-tuple it takes the newest visa
    still grantable to the Zode, and once that visa has passed `RENEW_AT`
    (2/3 of its lifetime, gate g89 q3), mints a new one, delivered ingress last
    like J5a. Issuing or renewing a core visa never revokes the one it
    supersedes, which expires on its own. Core visas are otherwise revoked
    like any visa (a policy that denies them, a link going down under them),
    and all of a Zode's core visas are revoked when its join ends (a rejoin or
    a disconnect). A failed mint
    or push is retried on the next heartbeat.
J10. **Fallback: a broken session means rejoin.** If a Zode's VS-API session
    fails for any reason, including its core visas lapsing, it closes its
    initial link and dials it again. That is J1–J6: fresh bootstrap visas, then
    reconnect or re-authentication. There is always a way back, and only one
    recovery path to test. *Built:* the triggers are a failed `connect`, a
    failed `registerVss` (after its retries), the VS-API run loop exiting (which
    is how an `authRequired` handle shows up), and losing the initial link.
    The Zode closes the link once, and its record redials. The relay that hosts
    the visa service has no initial link: when it loses its session, it grants
    itself its VS-API pair (IDs 1 and 2) again and reconnects (gate g89 q7).

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
     *Built differently at non-ingress hops (gate g89 q10):* only the ingress
     swaps its forwarding entry. It sends the `Stream ID Request` for the new
     visa and, when the answer comes back, moves the stream onto the new next
     hop under the same Stream ID. Every other hop binds the new visa as an
     ordinary F2 bind, a new forwarding entry beside the old one, and keeps the
     old entry until the old visa is revoked or a link it crosses drops.
     Expiry alone removes no binding at any hop (#271): an expired visa goes
     at the ingress when a new flow matches it there, and a transit hop's
     entry stays until revocation or link loss (see "Visa expiry at match
     time (#271)"); the same holds for "the old binding stays until it
     expires" above. *Built:* the rebind is
     refused, and counted as `RebindFailed`, when the new visa's session keys
     differ from the old one's, since the requestor still holds the old keys.
     A failed rebind is not retried. When the next hop grants a rebind whose
     target a still newer visa superseded while it was in flight, the commit
     is refused, but that stale commit is **not counted as a failure**: the
     newer visa's own rebind, started when that visa arrived, is the one
     that moves the stream, and it can still fail on its own (different
     session keys, say), which counts then. The stale commit is logged at
     INFO (#271, gate q5). A refused commit with no newer
     successor, and a next hop that refuses the rebind itself, still warn
     ("stays on visa") and count `RebindFailed`. The supersede of the
     bootstrap visas by the J5a visas, and of the relay's own pair by its
     J5a visas, runs in netns in `zode-link-test.sh` (join, restart and forced-drop rejoin), which also
     checks that no rebind fails. `zode-rejoin-test.sh` (#271) sends two
     concurrent queries per round to a UDP service, whose reply visas
     (`zc:53 -> zb:*`, a wildcard port) share a five-tuple and can supersede
     one another at zc.

### Answered at the planning gate (g89)

The design left two items open: how the on-behalf-of identity is carried, and
the bootstrap lifetime with the J9 threshold. The planning gate for #252 (pd
gate g89) answered them, along with the choices the code forced:

- **q2, on-behalf identity:** a separate VS-API call,
  `requestBootstrapVisas(peerCn)`, rather than an optional `onBehalfOfCn` field
  on `visaRequest`. The narrow J3 rule sits in one handler, the visa service
  derives both five-tuples from the CN, and the relay gets its own copies in
  the same answer.
- **q3, lifetime and threshold:** bootstrap visas live 5 minutes; core visas
  are renewed at 2/3 of their lifetime.
- **q4, delivery:** J5a and J9 visas always go by VSS push, ingress copy last,
  because J11 has no retry.
- **q5, a rejoin with a live session:** end the session and revoke its core
  visas, mark the Zode's other visas for a new push, and keep the node actor,
  so `reconnect` still works.
- **q6, schema removals:** renumber the later ordinals as a flag day, with all
  pins moving together.
- **q7, the relay:** it gets J5a/J9 visas like every Zode, and grants itself
  its VS-API pair again whenever it loses its session.
- **q8, netns scope:** the join and rejoin go into `zode-link-test.sh`; the
  two-joiner and wrong-CA scripts are #254. *Done in #254:*
  `zode-join-test.sh` and `zode-wrong-ca-test.sh`.
- **q9, core CI:** core takes the visa service from a pre-release cut from the
  visaservice PR's head while the set is open (BUILD.md).
- **q10, J11 per hop:** only the ingress swaps its forwarding entry (see J11).

## The relay hosts the visa service (#229)

The relay Zode and the visa service share one host, one network namespace and
one TUN. There is no `ph adapter` for `vs.zpr`.

- **One TUN, two addresses.** The relay's `zpr_addr` lists its own address
  first and `vs.zpr` (`fd5a:5052::1`) after it. `zpr_addr[0]` stays the
  relay's own address everywhere: its VS-API session and the source of that
  session, its VSS listener, and link ownership. `ph node` applies every
  `zpr_addr` entry to the TUN at startup if it is missing
  (`Assembly::ensure_local_zpr_addrs_on_tun`), and it never removes them at
  exit.
- **The visa service is a local actor of the relay.** Every `zpr_addr` entry
  is a local-actor address, so flows between `vs.zpr` and a Zode enter and
  leave at the relay's local actor, the way the relay's own flows do (F8).
  Endpoints reach the visa service through the relay: a packet from zb to
  `vs.zpr` reaches the relay's local actor, and the relay writes it to the TUN.
- **The relay's sessions with the visa service stay in the host** (Zodes plan
  §7 check 1). `vs.zpr` is a local address, so the kernel routes the relay's
  VS-API session to `vs.zpr`, and the visa service's VSS session to the relay,
  over `lo`. Those packets never reach the TUN or `ph`. The visa service's VSS
  sessions to other Zodes leave from `vs.zpr` (VISA_SERVICE.md, "Outbound VSS
  connections bind `vs_addr`") and go out through the relay's TUN.
- **Every client on the relay host binds its source.** With two addresses on
  one interface, the kernel picks `vs.zpr` as the source of an unbound
  connection to `vs.zpr` (RFC 6724 rule 1), and the address added last for
  other destinations. So the relay's VS-API connection binds `zpr_addr[0]`
  (`VSConn`, every `ph node`, gate g117 q3), the visa service's VSS and proxied
  JWKS connections bind `vs_addr`, and a future ZPRnet client on the relay
  host must bind too. `ph` does not deprecate `vs.zpr` (`preferred_lft 0`).
- **Startup order.** The TUN carries both addresses, then `ph node` starts,
  then valkey and the visa service. The visa service's VS-API and admin
  listeners bind `vs_addr`. If `vs_addr` is not local, the listener task logs
  an error and ends, but the process keeps running, so `vs.zpr` must already be
  on the TUN. The relay's VS-API connection retries until the visa service
  listens (`run_with_reconnect`). In netns, `ph` applies both addresses and the
  harness waits for `vs.zpr` on `tun0` before it starts the visa service.
  ZIPLINE's relay container (`zipline-relay-<id>`, [org-zpr/zipline#252](https://github.com/org-zpr/zipline/issues/252))
  pre-creates a persistent TUN with both addresses, so restarting `ph` never
  takes `vs.zpr` away from a running visa service. Then it starts `ph node
  --relay true --forwarding true`, valkey (127.0.0.1 only) and, once valkey
  answers PING, the visa service. If the visa service is not listening on
  `vs.zpr` (VS-API 5002, admin 8182) within 30 s, the container exits and
  Docker restarts it: the interim answer to
  [zl-zipline#266](https://github.com/org-zpr/zl-zipline/issues/266).
- **A relay refuses an adapter-docked visa service.** On a relay, an adapter
  whose verified certificate carries the visa service's DN is closed at keying,
  and no special-peer name is assigned (gate g117 q4). Nodes that are not relays
  (the `one-node-*` layouts) still accept it until #234.
- **The relay's own VS-API visas.** The self-granted pair (IDs 1 and 2), and
  the J5a and J9 visas that supersede it, match no packet, because the session
  runs over `lo`. They stay (gate g117 q5): an insert supersedes, there is no
  live stream to rebind, and the J10 relay path that #252 stabilised stays
  untouched. Removing them waits for #234.
- **The visa service needs no change.** It authenticates itself at startup.
  With no adapter, the first node to authenticate becomes the node the
  `vs.zpr` actor docks on (VISA_SERVICE.md), and that is always the relay.

## Left to the builder

- The link-up timeout: 10 s is the default. It is a constant with a comment.
  It started at 5 s (my call); gate g123 raised it to 10 s, because the
  non-owner can learn the link about 4 s after the owner (see Deviations).
- The initial-link backoff schedule: start around 1 s, double, cap around 30 s,
  with jitter (my call).
- Where the per-link state machine lives in `ph`, and its exact state names. The
  states have to cover at least: dialing, keying, CN check, up, closing, closed.
  Every transition gets a unit test, and the rules above are the acceptance criteria.
- How rules 9 and 12 find "state tied to a link". Recommended: hang everything
  link-scoped off one per-link object, so dropping it clears everything.

## Design decisions

Decisions taken while #252 was built, at the operator gates of pd plan p33,
while #253 was built (pd plan p34), and while #255 was built (pd plan p37).
The planning gate's answers are in "Answered at the planning gate (g89)" above.

**A bootstrap visa is rechecked against its peering, not its route** (gate
g94 q1). A policy or attribute sweep rechecks every live visa. A bootstrap
visa was minted from the policy peering, not from policy evaluation, so the
sweep asks the same questions the mint asked: the visa's holder is still the
relay, and the relay's topology still names the joiner by the CN policy gives
the joiner's address. Route and link state play no part. They cannot: a
rejoin of a Zode with a live session forgets that Zode's link reports (J6), so
it has no route until it reports again. A routed recheck in that window would
answer `NoRoute` and revoke the very pair the rejoin needs. A policy that
drops the peering or the joiner's CN still revokes the pair. Known limit: the
visa does not record the CN it was minted for, so a policy that only renames
the joiner keeps the pair for the rest of its 5-minute life. The rejected
options were skipping bootstrap visas in the sweep (a removed peering would
not end them) and a follow-up issue (a badly timed sweep would break rejoins
until it was fixed).

**A node's core visas carry no session keys** (gate g100). J11 refuses to
rebind a live stream onto a visa with different session keys, because the
stream's requestor still holds the old keys. A bootstrap visa cannot carry
keys: the joiner has not authenticated, so it has registered no A2A key yet.
So the visas that supersede it, the J5a VS-API pair, the J5b VSS visa and
their J9 renewals, carry none either. Otherwise the J5a supersede could never
happen, and every joiner would rejoin each time its bootstrap pair expired.
The visa service applies this to every flow between itself and a node, in
one place (`session_key_for`). Visas between two nodes keep their keys. This
traffic is the VS-API and VSS connections, which have their own TLS. That
TLS authenticates neither end today; see SECURITY_MODEL.md, "Design
decisions", and [#259](https://github.com/org-zpr/zl-zipline/issues/259).
Rejected: letting core rebind across a key change for node-to-VS flows only
(a special case in J11's guard), and accepting a rejoin every 5 minutes.

**A policy pass never waits on a node's VSS** (gate g101). The policy pass
runs on the visa service's single event worker. It used to wait for every
node's `setTopology`, `setServices` and `requestAuthentication`. A policy
that removes a joiner's link to the relay cuts the joiner off, and each of
its calls then waited out the 10 s RPC timeout, so the next policy install,
even one that restored the link, waited behind them: 21–50 s, and sometimes
minutes. #252 made this reachable, because a joiner now has a VSS
connection. Now:

- Each node's pushes run in a detached task of their own, in order: topology,
  then services, then the re-authentication request. A slow or unreachable
  node delays only its own pushes.
- The pass sends nothing to a node that a link it took down has cut off: an
  end of such a link with no route from the node the visa service docks on.
  That node's VSS worker is stopped. The node finds its session broken and
  rejoins (J10).
- Stopping a VSS worker never waits on the worker's queue.

No lock is held across a VSS call. Two passes' pushes for one node may
interleave, which is harmless: a `setTopology` or `setServices` makes the
worker re-send the current state at its next heartbeat.

**A Zode binds a `Stream ID Request` only from the visa's previous hop**
(gap 7, #253, pd plan p34, gate g106). Decided:

- **One address, F1's** (g106 q2). The check runs F1's egress lookup in
  reverse: `find_zode_egress_link(prev_hop)` must return the link the
  request arrived on. After topology that is the topology `zprAddr` for the
  verified CN; on a joiner until a `setTopology` names its relay, the relay
  address from the hand-off. The record must be up and point at this connection, so the old
  connection of a CN replaced under rule 5 is refused. Rejected: a second
  link-to-address helper that ignores record state (two definitions to keep
  in step).
- **It sits with the other F2 guards**, in `prepare_next_hop_bind`, first
  after the visa and egress lookup, and before the loop guard. Every bind
  path that can see a Zode requestor goes through it. A J11 rebind starts
  only at an adapter or local-actor ingress, and its `Stream ID Request` is
  checked at the next hop against the new visa's `prev_hop`.
- **The visa-by-ID fetch runs before the check**: `prev_hop` is in the visa.
  The cost is bounded: the visa service returns only this Zode's own copy
  of a visa on its path, when the VSS push would deliver it now or already
  has ([#262](https://github.com/org-zpr/zl-zipline/issues/262)), and the
  requestor's transaction IDs bound its open requests.
- **A refusal keeps the link open**: it is an error `Stream ID Response`,
  not the handler-error path F5 uses.
- **One counter for every refusal** (g106 q3): `StreamIdRequestRejected`
  ("Stream ID Request Rejected") counts every refused `Stream ID Request`:
  the previous-hop check, the other F2 guards, no egress, a visa denial,
  and rule 9's refusal of a bind pending over a lost link. The log line
  carries the reason; the previous-hop line says "not the visa's previous
  hop". `zode-link-test.sh` asserts that no previous-hop refusal is logged
  on the relay or the joiner, and that the counter is zero on both until
  the first scenario that breaks a link.
- **J4 refuses a mismatched pair** (g106 q4), see J4.
- **Symmetric visas stay separate**
  ([#246](https://github.com/org-zpr/zl-zipline/issues/246)). A request
  from a visa's next hop is refused, which is #246's recorded state; #246
  will extend this one check when it defines reverse binding.
- The core PR and this document merge together (g106 q5).

**`Link.peer` is optional, and unset means "never dialed"** (#255, gate g115
q2). A relay must hold a joiner's CN before the joiner connects (rule 4, J2).
A joiner with no substrate address in policy, such as a laptop that changes
networks, has not reported one yet at that point, so the relay's `Link` for it
has nothing true to carry. The relay never dials that link, so the field is now
optional in `vs.capnp`. zl-zpr-common decodes an unset `peer` as `None`, and
refuses a set one with an unspecified IP or port 0, so there is no sentinel on
the wire. Rejected: a `[::]:0` placeholder on ends that do not dial (a
fallback, which the no-backwards-compatibility rule forbids), requiring every
endpoint to have a policy address (an endpoint that changes networks cannot
have one), and leaving the link out until a follow-up (no such endpoint could
join).

**A reported address comes from the node's session, and a session start
re-proposes the node's links** (gate g115 q3). An address changes only by a
rejoin (rule 13a), so a session start is the one moment a peer's link can
become possible. The visa service then sends `setTopology` to the node's policy
peers that have a live VSS connection. This replaces what visaservice#65 did
with `updateSubstrateAddr`. Nothing is sent when a session ends. The address is
stored on the node's actor, which a rejoin keeps, so after a rejoin ends a
session the old address stays known until the next `connect` or `open`
replaces it (see "Known limit" under "Links from known addresses (#255)").

**Relay links are always proposed; direct links need both addresses** (gate
g115 q4). The relay link is the joiner's way in, so it cannot wait for an
address. A direct link is proposed only when both ends are known, so the visa
service need not copy the Zodes' lower-address ownership rule to find out which
address the owner needs. A pair that cannot reach each other at those addresses
falls to rule 13a.

**The relay is the node the visa service docks on, not the policy's `vsdock`
flag.** zplc sets the `vsdock` join flag on every node today (the "Not all nodes
are VS docks" TODO in `policybuilder.rs`), so using it would make every link a
relay link. `visa_bootstrap::is_relay` is the test `requestBootstrapVisas`
already uses (visaservice#68).

Decisions of gate g117 (pd plan p41,
[#229](https://github.com/org-zpr/zl-zipline/issues/229)):

**The relay lists `vs.zpr` explicitly** (q2). A relay must list
`fd5a:5052::1` in `zpr_addr` after its own address. Startup refuses a relay
without it, a relay that lists it first, a duplicate address, and a node that
is not a relay but lists it. Each refusal names the fix. Rejected: deriving
`vs.zpr` from `relay = true`, which hides the TUN's second address behind a
flag. ZIPLINE writes the address into the config, so the addressing is
visible there.

**Every node binds its own address for VS-API** (q3). One code path, and the
session always comes from the address the node claims. If the address is not
on the TUN yet, the dial fails and `VSConn` retries; there is no fallback to an
unbound connect. Rejected: binding only on a relay, a special case that leaves
joiners with a kernel-chosen source.

**The adapter-docked visa service is retired on a relay** (q4). A relay closes
a dock from the visa service's adapter, and every Zode netns script runs the
co-located relay, with J7 tested by stopping the visa service (SIGSTOP and
SIGCONT). The adapter-docked `one-node-*` scripts stay until #234.

**The relay's self-granted VS-API pair stays** (q5). Rejected: dropping it on a
relay, which would be half done, because the visa service would still mint J5a
and J9 visas for the relay unless the visa service changed too.

**Bind, never deprecate.** Following #228, `ph` does no `preferred_lft`
handling on the relay's TUN; every client that matters binds its source.

Decisions of pd plan p50
([#271](https://github.com/org-zpr/zl-zipline/issues/271)):

**A Zode enforces visa expiry when it matches a flow, with no sweep**
(operator, 2026-11-18). `ph` never enforced expiry when binding a flow
(J4 checks only a bootstrap hand-off), while the visa service
forgets a visa when it expires, so no revocation can reach an expired copy
afterwards. The fix checks expiry where a Zode uses a visa for a new
binding:

- **Each hop checks once, at its lookup.** The ingress checks at its
  match; a transit hop when it handles the `Stream ID Request`, against
  the visa it holds or, if it must fetch it by ID, at the visa service's
  live lookup. No hop checks again when the bind commits
  (`VisaTable::bind` checks only that the visa is held), and the ingress
  binds a visa it has just been granted unchecked. So a bind whose checks
  all passed before the visa expired is not stopped by the expiry.
- **At a flow's ingress, an expired visa is ejected.** `match_traffic`, the
  one lookup every new flow from an adapter or the local actor goes through,
  removes an expired visa through the one removal path (`revoke`: bindings,
  PFT entries, J11 indexes) and looks again. Unless that lookup finds a
  live visa, the Zode asks the visa service for a new visa (F6). The adapter streams that were
  bound under the ejected visa are withdrawn, outside the visa-table lock,
  so the adapter's next packet binds afresh. This is the "tears down the
  tethers to its adapters" of VISA_SERVICE.md.
- **Further down the path, a `Stream ID Request` for an expired visa the
  hop holds is refused**, and the visa is neither ejected nor fetched again by ID. Its
  live transit streams come from an upstream Zode that no withdrawal reaches
  (F5), so freeing their stream IDs here could hand one to an unrelated
  stream while that Zode still sends on it. The visa service has forgotten
  the visa, so a fetch would find nothing. The ingress Zode recovers by
  ejecting its own copy at its next match.
- **No periodic sweep.** `VisaTable::handle_expirations` stays test-only. At
  each Zode, an expired visa with live streams keeps carrying them until a
  new flow matches it there, or that Zode loses a link they cross (F3). The
  visa service cannot revoke it: it has forgotten the visa. Whether
  to sweep, and whether a sweep notifies, is
  [#274](https://github.com/org-zpr/zl-zipline/issues/274).
- **A rebind superseded mid-flight is no failure** (gate q5): see J11.

Rejected: a periodic sweep in this fix (deferred to #274); ejecting an
expired visa at a transit hop's by-ID request (stream ID reuse, above);
fetching an expired visa again by ID (the visa service no longer has it);
and revoking a visa locally when a next hop refuses it with "visa N not
found" (that is [#257](https://github.com/org-zpr/zl-zipline/issues/257)'s
question, and the match-time check already ends the expired case).

## Implementation status

Built on `feature/zodes` in three core PRs (zl-zpr-core#105 keying and
acceptance, #106 lifecycle, and the reporting PR), with the visa-service half in
zl-zpr-visaservice#66 and the schema in zl-zpr-vsapi#5 and zl-zpr-common#14
(tag `zl-v0.32.0-zodes.3`). Rules 11a and 16 and the relay join came with #252;
see "Relay join (#252)" below.

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
| The re-proposal after a `down` report (rules 10, 13 and 13a; zl-zipline#268, visaservice#71): newly-down detection and the pending marks (`LinkStateTable::newly_down`, `mark_repropose`, `take_repropose`), `TopologyMgr::record_link_states`; scheduling and the delayed send (`schedule_link_repropose`, `repropose_link_after_down`) | `zl-zpr-visaservice/vs/src/link_state.rs`, `vs/src/topology_mgr.rs`, `vs/src/vsapi_worker.rs` |
| netns test: a relay, a Zode with an initial link to it, and a Zode outside the relay's topology; since #252 also the join, restart and rejoin (see "Relay join (#252)"); since #229 the relay hosts the visa service, with §7 check 1 asserted at the end of scenario 1 | `zl-zpr-core/integration-test/zode-link-test.sh`, `integration-test/lib/zode_funcs.sh` |
| The relay hosts the visa service (#229): the relay's `zpr_addr` rules (`check_node_zpr_addrs`); the VS-API connection binds `zpr_addr[0]` (`VSConn::new`, `connect_from`); a relay refuses the visa service's adapter at keying (`process_keying_done`) | `zl-zpr-core/adapter/ph/src/config.rs`, `libnode2/src/vsconn.rs`, `adapter/ph/src/link_state.rs` |
| netns tests (#254): two joiners at once; zb - relay - zc with the relay's forwarding on and off; a direct link that drops and re-routes through the relay; a Zode in the policy with a certificate from another CA | `zl-zpr-core/integration-test/zode-join-test.sh`, `zode-line-test.sh`, `zode-reroute-test.sh`, `zode-wrong-ca-test.sh` |

Everything scoped to one connection hangs off that connection's peer-table
entry, so dropping the entry clears it (rule 9). A record holds only the
connection's id. #251's stream bindings are the exception: they live beside
their visa in the visa table, and `drop_peer` revokes every visa bound over the
lost connection (F3). #252's bootstrap visas hang off the peer entry
(`PeerState::bootstrap_handoff`).

### Constants

All in `zl-zpr-core/adapter/ph/src/`, each with a comment:

- `LINK_UP_WINDOW` = 10 s, the owner's link-up window (rule 6), `zode_link_record.rs`.
- `OWNER_REDIAL_INTERVAL` = 1 s, the owner's redial inside the window (rule 6).
- `INITIAL_LINK_BACKOFF_MIN` = 1 s, doubling to `INITIAL_LINK_BACKOFF_MAX` = 30 s,
  each delay jittered to between half and all of it (rule 11).
- `ZODE_KEYING_TIMEOUT` = 2 s and `ZODE_HELLO_TIMEOUT` = 2 s, `config.rs`. Together
  they fit inside the window, which a compile-time assertion checks, so an
  attempt ends on its own before the window does. When the window ends, the
  owner also closes any attempt still in flight.
- `REPORT_COALESCE` = 100 ms: report requests that close together go out as one
  snapshot, `zode_links.rs`.

In `zl-zpr-visaservice/vs/src/config.rs`:

- `LINK_REPROPOSE_DELAY` = 5 s, from the first `down` report of a direct link
  to its re-proposal (rule 13a, zl-zipline#268, gate g149). A silent link is
  reported only after keep-alive gives up (about 9-12 s), but a closed
  connection is reported at once, so the wait keeps the retry from landing in
  the same instant as the fault. The two ends detect a silent link up to one
  3 s keep-alive period apart, so the second end's `down` coalesces into the
  pending re-proposal. With the owner's 10 s window, an outage that clears
  within about 15 s of the first `down` heals by itself. Much longer would
  only keep the pair on the relay for no benefit.

### Configuration

- **Initial link.** `[node.initial_link]` with `addr = "IP:port"` (the relay's
  substrate address) and `cn = "<relay CN>"`, or `ph node --initial-link
  CN@IP:PORT`, which overrides the file. At most one. Validation rejects an empty
  CN, an unspecified IP and port 0. Node only. On a ZIPLINE machine,
  `zipline register` writes the initial link: the server's poll answer carries
  the relay's CN and IPv4 substrate address, and `zipline service-run` passes
  them as `--initial-link <relay_cn>@<relay_addr>`, beside the Zode
  certificate (`--certificate-file`, `--private-key-file`, `--ca-file`) the
  same registration issued. It never sets `--self-addr` or
  `--advertised-substrate-addr` on an endpoint
  ([org-zpr/zipline#251](https://github.com/org-zpr/zipline/issues/251);
  SECURITY_MODEL.md, *Design decisions*).
- **Relay.** `relay = true` in `[node]`, or `ph node --relay true`. Default off.
  A relay has no initial link: setting both is a config error.
- **The relay's topology on ZIPLINE.** Rule 4 needs every joiner's CN in the
  relay's topology before the joiner connects. ZIPLINE's generated policy
  declares a link from the relay to every registered Zode, so the push that
  enrols a machine already holds its relay link. Deleting the machine removes
  that link, and the relay refuses its still-valid certificate from then on.
  The network's own `dns.zpr` Zode is in that topology too ([org-zpr/zipline#252](https://github.com/org-zpr/zipline/issues/252)):
  ZIPLINE's dns container runs CoreDNS behind a `ph node` named `dns.zpr`,
  with `--initial-link node.zpr.org@<relay dock address>`, a server-generated
  link certificate and RSA key, and its fixed address `fd5a:5052:90de::2`. It
  links to the relay only; no rule gives it a direct link.
  Direct links follow the network's allow rules (ROUTING.md, "Implementation
  status"; [org-zpr/zipline#253](https://github.com/org-zpr/zipline/issues/253)).
- A Zode's own address, the one ownership compares, is `zpr_addr[0]`. It must
  be IPv6, because the VS-API session to `vs.zpr` leaves from it (#229).
- **The relay hosts the visa service** (#229, gate g117 q2). A relay lists
  `fd5a:5052::1` (`vs.zpr`) in `zpr_addr`, and not first. No address may be
  listed twice, and a node that is not a relay never lists `vs.zpr`.
  `Config::check_valid` refuses each case and names the fix. The command
  line's `--zpr-addr` values come before the file's.

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
  `Link.peer` is optional (tag `zl-v0.32.0-zodes.5`): a link this Zode owns with
  no `peer` is logged and skipped too, and a set `peer` with an unspecified IP or
  port 0 fails to decode. A link the peer owns, the relay's links and the initial
  link's entry need no `peer`. The log line for a received topology prints each
  peer as `addr:port` or `unset`.
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
- **Rules 3 and 6 (refuse, then redial).** A link refused after keying gives
  up the peer's substrate address at once, then closes. Its close can wait
  up to `DEFAULT_TERMINATE_TIMEOUT` for a Terminate ack the dialer often
  cannot send, because it has already dropped its SA. Before #254 the
  address stayed with the dying link for that wait, and the owner's redial
  from the same address went to it and was lost, so the link ended
  `failed` (gate g120, found by `zode-reroute-test.sh`). Now the redial
  starts a link of its own.
- **Rule 6 (the window).** The window is 10 s, not 5 s (gate g123). If the
  owner already has a VSS session when the non-owner authenticates, the
  visa service sends the owner the link at once. The non-owner gets it
  only after the visa service's VSS start delay (3 s) and a NoRoute retry
  (1 s). `zode-reroute-test.sh` saw a lag of 4.2 s. With 5 s and a redial
  every second, the window ended `failed` although both ends were healthy.

### Forwarding over Zode links (#251)

Built on `feature/zodes` in two core PRs: zl-zpr-core#109 (F1, F2, F5, and the
F7 and F8 tests) and the F3/F6 PR (merge set B with this document). Covered by
unit tests and core CI, and in netns since #254 by `zode-reroute-test.sh`:
zb and zc bring up a direct link and ping across it with no forward-only
visa on the relay; the zb-zc substrate is blocked, both ends report the link
down, the visa service stops routing over it, and zb's next echo re-requests
and comes back through the relay on forward-only visas (rules 9, 10, 14, 15;
F3, F4, F6). Since [#231](https://github.com/org-zpr/zl-zipline/issues/231)
the same script is the netns Zodes demo. A bulk transfer leaves the relay's
received bytes (`br0`'s RX counter in its namespace) flat over the direct
link and raises them through the relay. Then the block is lifted, and new
flows go direct again. *As built:*
- **A `down` link is proposed again once.** The owner does not redial (rule
  10). Since [#268](https://github.com/org-zpr/zl-zipline/issues/268), the
  visa service sends the pair new topology `LINK_REPROPOSE_DELAY` after a
  `down` report. The owner then makes one attempt, and a `failed` report
  falls to rule 13a, so the retries are bounded. The test's long block
  outlasts that attempt, so it asserts one re-proposal and then `failed`;
  it then restarts zc, a pair change, to bring the link back, because a
  policy install keeps the failed mark. A last step blocks the link only
  until both ends report it `down`, and the link comes back by itself.
- **Flows that moved to the relay stay there** until their visas expire.
  Only new visas take the link (Zodes plan §5.8). A TCP client visa
  wildcards the source port, and every echo request between two addresses
  shares one ICMP visa, so a new connection or ping between the same pair
  is not a new flow. The test's new flow is zc -> zb.

`zode-line-test.sh` runs a flow between two Zodes through the
relay with its `forwarding` on and off (F1, F2, F8).

| Rule | Location (`zl-zpr-core/adapter/ph/src/`) |
|---|---|
| F1 egress to a Zode: an up link record whose verified CN's topology `zprAddr` is the next hop; adapter matching (`actor_auth`, AAA) on non-Zode links only | `assembly.rs::find_egress_link` |
| F1 warning when a `setTopology` lacks the initial link's CN | `zode_links.rs::apply_topology` (`initial_link_missing_from_topology`) |
| F2 guards, in order: no visa or no egress, the previous hop (#253: `check_prev_hop`, F1's `find_zode_egress_link` for the visa's `prev_hop` must give the ingress link; `Visa::prev_hop`), the loop guard (egress == ingress is refused), a forward-only visa from or to an adapter, and a refusal when transaction IDs run out. Every refused `Stream ID Request` is counted once, as `StreamIdRequestRejected`, in `bind_reject` | `mgmt/node.rs::prepare_next_hop_bind` |
| F3 bindings beside the visa (`StreamBinding { ingress, egress }`), recorded by `requested_tether_granted`; `revoke_for_link` revokes on either side, except the node's built-in visa-service visas (IDs below `MIN_VISA_ID`). Rule 9's cleanup of the bootstrap visas carried over an initial link is #252's `bootstrap_handoff::revoke_for_lost_link` | `visa_table.rs` (`VisaTable::bind`, `revoke_for_link`), `mgmt/node.rs` |
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
#252's J11 adds one `commit_supersede(old, new, ...)` under the same lock: it
moves a binding onto the superseding visa's ID and swaps the ingress PFT entry
onto the new next-hop stream under the same Stream ID, replacing the visa
without editing either one. The supersede state (`current`, `successors`)
lives beside the visas in `VisaTable`.

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

### Relay join (#252)

Built in five PRs. The schema is zl-zpr-vsapi#6 and zl-zpr-common#15 (tag
`zl-v0.32.0-zodes.4`). The hand-off codec and the joiner's checks are
zl-zpr-core#111, and J11 is zl-zpr-core#112; all four are merged. The relay and
the joiner (zl-zpr-core#113), the visa service's half
(zl-zpr-visaservice#67) and the compiler's pin (zl-zpr-compiler#26) are #252's
merge set C, which merges together with this document.

| Concern | Location |
|---|---|
| ZDP 146: the codec (`encode`, `decode`), the joiner's checks (`check_handoff`) and install bound to the link (`accept`, `install_bound_to_link`); rule 9 for bootstrap visas (`revoke_for_lost_link`); the joiner's egress before topology (`pre_topology_relay_cn`, `topology_names_relay`) | `zl-zpr-core/adapter/ph/src/bootstrap_handoff.rs`; the handler is `mgmt/handlers.rs::handle_bootstrap_visas`, the packing `tlv.rs` (`pack_visa`, `unpack_visa`) |
| The relay's request and hand-off, and J7 (`start_relay_hand_off`, `relay_hand_off`), started after Hello | `zl-zpr-core/adapter/ph/src/bootstrap_handoff.rs`, called from `zode_links.rs::link_up` |
| `requestBootstrapVisas` on the node | `zl-zpr-core/libnode2/src/vsconn.rs` (`VSConnHandle::request_bootstrap_visas`) |
| J1 (`VisaTable::initial_for_node`); the relay's own pair after a lost session (`regrant_vs_visas`); R16 reset (`retain_vs_api_visas`) | `zl-zpr-core/adapter/ph/src/visa_table.rs` |
| J5 (connect only after a hand-off), J10 (`rejoin`), the `registerVss` retries, R16 | `zl-zpr-core/adapter/ph/src/vs_worker.rs` |
| J11 supersede in the visa table (`insert_superseding`, `commit_supersede`, `has_same_session_keys`) | `zl-zpr-core/adapter/ph/src/visa_table.rs` |
| J11 rebind at the ingress (`rebind_stream`, counted `RebindFailed`), started by `visa_mgmt.rs::insert_visa`; the in-place PFT swap | `zl-zpr-core/adapter/ph/src/mgmt/node.rs`, `forwarding_tables.rs` |
| `requestBootstrapVisas`: the handler, the rule (`request_bootstrap_visas`), the rejoin teardown (`end_previous_join`), the sweep recheck (`bootstrap_visa_authorized`) | `zl-zpr-visaservice/vs/src/vsapi_worker.rs`, `vs/src/visa_bootstrap.rs` |
| The bootstrap mint (`mint_bootstrap_visa`), ingress-last delivery (`deliverable_visas_for_node`), the by-ID fetch (`visas_for_node_by_ids`), no keys on node-to-VS flows (`session_key_for`, `is_node_vs_flow`), a rejoin's re-push (`forget_installs_on_node`) | `zl-zpr-visaservice/vs/src/visa_mgr.rs` |
| J5a, J5b, J9 (`issue_vsapi_visas`, `issue_vss_visa`, `renew_core_visas`); the issuance lock and its lock order (`lock_issuance`); a join's core-visa revocation (`revoke_previous_join`) | `zl-zpr-visaservice/vs/src/core_visas.rs` |
| The policy pass's pushes (`push_policy_to_nodes`) and the cut-off rule (`nodes_cut_off`); stopping a VSS worker without waiting (`ConnectionControl::stop_vss_worker`) | `zl-zpr-visaservice/vs/src/event_mgr.rs`, `vs/src/connection_control.rs` |
| Topology without visas (`topology_links`) | `zl-zpr-visaservice/vs/src/vss_worker.rs` |

**Constants**, each with a comment:

- `BOOTSTRAP_REQUEST_TIMEOUT` = 5 s, the relay's bound on the request (J7),
  `bootstrap_handoff.rs`. It is libnode2's `BOOTSTRAP_VISAS_TIMEOUT`, the
  VS-API call's own bound, so a request the relay gave up on is not sent.
- `HANDOFF_VISA_COUNT` = 2, `bootstrap_handoff.rs`; common's
  `BootstrapVisas::VISAS_PER_LIST` = 2.
- `REGISTER_VSS_ATTEMPTS` = 5, one `VSCONN_RETRY_WAIT` (1 s) apart, a joiner's
  `registerVss` tries while the visa service answers `internal`,
  `vs_worker.rs`.
- In the visa service: `BOOTSTRAP_VISA_LIFETIME` = 5 min
  (`visa_bootstrap.rs`), `RENEW_AT` = 2/3 (`core_visas.rs`),
  `CORE_VISA_RETRY_INTERVAL` = 1 s for a failed J5a issue
  (`vsapi_worker.rs`), and the J9 check on every `VSS_HEARTBEAT_INTERVAL`
  (800 ms, `config.rs`).

**Wire.** ZDP type 146 `Bootstrap Visas` ([ZDP.md](ZDP.md)). In `vs.capnp`,
`VSHandle.requestBootstrapVisas @10 (peerCn :Text) -> (res
:Result(BootstrapVisas))` and `struct BootstrapVisas { peer @0 :List(Visa);
relay @1 :List(Visa); }`; common refuses any list that does not hold exactly
two visas. `Link.visas` and `VSHandle.updateSubstrateAddr` are gone. The
members after them were renumbered (`Link.zprAddr` @3, `Link.peerCn` @4,
`getLoginNonce` @9) as a flag day: a binary built against `zodes.3` fails
against one built against `zodes.4` with a decode or "unimplemented" error.

**Tests.** Unit tests in each PR, written first. In netns,
`zode-link-test.sh` covers the join: the relay hands zb two new visa IDs
(1000 or above), zb holds only those before it authenticates, then it
authenticates, reports its link to the relay up, registers its VSS and gets
topology. The J5a VS-API visa supersedes zb's forward bootstrap visa, and the
relay's own pair is superseded the same way; no rebind fails and nothing
rejoins. It also covers a restart and a forced link drop (a rejoin, with new
IDs each time), a CN outside the relay's topology (no request), J7 (the relay
cut off from its visa service closes zb's link), and the relay never dialing.
Before the g101 fix, the policy scenario (a policy removes, then restores, the
relay-zb link) failed in about 2 of 8 local runs; with it, 7 of 7 passed.
Since #254, `zode-join-test.sh` runs two joiners at once: each gets its own
hand-off, none of whose IDs is the other's, holds only those before it
authenticates, joins fully and pings the visa service.
`zode-wrong-ca-test.sh` has a Zode in the policy whose certificate comes
from another CA: the relay refuses it in the handshake, it never links, gets
a hand-off or registers, and a joiner beside it is unaffected. No Zode in
either script refuses a Stream ID Request. In `zode-line-test.sh` that holds
for every Zode with the relay's `forwarding` on; with it off, the relay
refuses the forward-only binds by design, so only zb and zc are checked.

**Deviations** (each is flagged inline above):

- J2: a separate call, not a `visaRequest` (gate g89 q2).
- J5a: delivered by ordered VSS push, not in a call's answer; the switch is
  J11's supersede.
- J11: only the ingress swaps its forwarding entry; other hops bind the new
  visa beside the old (gate g89 q10). A key change refuses the rebind.
- "`reconnect` after a link wipe": the previous join's VS-API and VSS visas
  are revoked and replaced, not pushed again. `vs.capnp`'s comment still says
  the VSS visa is pushed again.
- `requestBootstrapVisas` errors: `vs.capnp` lists `invalidOperation`,
  `notFound`, `internal` and `temporarilyUnavailable`. The visa service also
  answers `authRequired` on a dead handle, and never answers
  `temporarilyUnavailable`. The relay closes the link on any error, so the
  difference has no effect.

Visa-service pre-releases for core CI while set C is open are in
[BUILD.md](BUILD.md).

### Links from known addresses (#255)

Rule 13b with reported addresses. The schema is zl-zpr-vsapi#7 and
zl-zpr-common#16 (tag `zl-v0.32.0-zodes.5`), both merged. The visa service's half
(zl-zpr-visaservice#68), core (zl-zpr-core#121) and the compiler (zl-zpr-compiler#27,
zplc 0.21.0) are p37's merge set, which merges together with this document.

| Concern | Location |
|---|---|
| The one rule for a reported address on `connect` and `open` (`accept_reported_substrate`, `require_dialable_substrate`) | `zl-zpr-visaservice/vs/src/connection_control.rs`, called from `vs/src/vsapi_worker.rs` |
| A node's known address (`known_substrate_addr`); which links are proposed and with which `peer` (`proposed_links`) | `zl-zpr-visaservice/vs/src/vss_worker.rs` |
| A peer with no policy address kept as `None` (`resolve_peers`) | `zl-zpr-visaservice/vs/src/policy_mgr.rs` |
| Re-proposal on session start (`repropose_links_to_peers`) | `zl-zpr-visaservice/vs/src/vsapi_worker.rs` |
| `Link.peer: Option<SockAddr>`, the decoder's refusal of an unspecified IP or port 0 | `zl-zpr-common` `vsapi_types` |
| An owned link with no `peer` skipped (`apply_topology`); `LinkRecord.dial_addr` is an `Option` | `zl-zpr-core/adapter/ph/src/zode_links.rs`, `zode_link_record.rs` |

**Tests.** Unit tests in each PR, written first. In netns, `zodes.zplc` gives zb
no substrate address (gate g115 q6). `zode-link-test.sh` checks that the
relay's topology names zb with `peer` unset before zb connects, that zb joins
and authenticates, and that the relay's next topology carries the address zb
reported. A direct link between two endpoints from reported addresses has unit
tests only. Its netns scenario is left to #254 or later.

**Known limit.** A rejoin ends a session but keeps the node's actor, so the
address it reported stays known until its next `connect` or `open` replaces it.
In that window a direct peer may be proposed the old address. The dial fails
and the pair falls to rule 13a, which that next session start clears (a
`ponytail` note on `known_substrate_addr`).

### Network change (#230)

A Zode survives a change of its own substrate address by the rejoin above:
rules 11, 11a and 16, with the keep-alive as the detector (gate g114 q2).
There is no new protocol and no visa-service change. Built in zl-zpr-core#119
(Windows pktinfo, which lets a Windows Zode bind the wildcard) and
zl-zpr-core#123.

| Concern | Location |
|---|---|
| The reported substrate address: `advertised_substrate_addr` if set, else a concrete `self_addr`, else, for a Zode with a wildcard `self_addr` and an initial link, the initial link's current local address with `self_addr`'s port. A wildcard relay with no advertised address is a startup error (`ReportedSubstrateAddr`) | `zl-zpr-core/adapter/ph/src/config.rs` |
| The initial link's local address (`initial_link_local_addr`) | `zl-zpr-core/adapter/ph/src/zode_links.rs` |
| `connect` and `open` read the address at each call, not once at startup (`SubstrateAddrSource`) | `zl-zpr-core/libnode2/src/vsconn.rs`, wired in `adapter/ph/src/main.rs` |
| A send error from a vanished source address is dropped, counted in `OutPacksDrop` and logged at most once per interval, instead of stopping `ph` (`is_network_change_send_error`): network or host unreachable, network down, address not available, and `EINVAL` for a send that named its source (Linux IPv6). Every other send error is still fatal | `zl-zpr-core/adapter/ph/src/fastpath_io.rs` |
| Detection: the keep-alive closes the initial link after `KEEP_ALIVE_MAX_MISSES`. A comment there notes that OS address or route notifications could detect it sooner | `zl-zpr-core/adapter/ph/src/link_state.rs` |
| netns scenario 8: zb, with a wildcard `self_addr` and no advertised address, moves to another subnet. It keeps running, drops its sends from the old address, rejoins with a new hand-off, and the relay's next topology carries zb's new address | `zl-zpr-core/integration-test/zode-link-test.sh` |

A concrete `self_addr` is still allowed for a Zode with a static address, and
does not follow a network change (gate g114 q5). Detection takes up to about
12 s, plus the redial backoff (at most 30 s) after a long outage. Direct links
to a Zode at its new address need peers to receive the new `Link.peer`, which
the visa service sends with the topology it re-proposes on that Zode's session
start (#255). Windows and macOS were checked by hand: a Zode with a wildcard
`self_addr` and no advertised address starts, joins through the relay and
reports its real local address (`windows-node-test.md` and
`macos-node-test.md`, section 6). Network-change runs on those two hosts are
[zl-zipline#263](https://github.com/org-zpr/zl-zipline/issues/263).

### The relay hosts the visa service (#229)

Built in zl-zpr-core#118, with no visa-service change (core CI runs visa
service `v0.22.0-zodes.8`). The design is "The relay hosts the visa service
(#229)" above; the code is in "Where the code lives".

- The netns Zode layout (`lib/zode_funcs.sh`) has no `zpr-vs` namespace and no
  visa-service adapter. The relay's `ph node` gets `--zpr-addr` for its own
  address and then for `vs.zpr`, and applies both to `tun0` itself. The
  harness waits for `vs.zpr` on `tun0`, then starts valkey and the visa
  service in the relay's namespace, then waits for the relay to register.
  All five Zode scripts use it.
- `zode-link-test.sh` asserts §7 check 1 at the end of scenario 1: in the
  relay's namespace, `ip -6 route get fd5a:5052::1` gives `dev lo`, and `ss`
  shows the relay's VS-API connection from its own address to
  `[fd5a:5052::1]:5002` and the visa service's VSS connection from
  `fd5a:5052::1` to zb's `:8183`.
- J7 (scenario 6) stops the visa service with SIGSTOP. The relay's bootstrap
  request times out after `BOOTSTRAP_REQUEST_TIMEOUT` (5 s) and it closes zb's
  link. SIGCONT follows at once, so the relay's own session never drops.

### On real machines (org-zpr/zipline#255)

The demo has run on real machines against a ZIPLINE dev server: relay and
DNS Zodes in containers, Ubuntu and Windows Zodes on adapter 2.0.4, and the
visa service at `65fe475` (`v0.22.0-zodes.9`).
- **The runner and the recordings.** The runner is zipline
  `test/manual/zodes-demo/demo.sh`, and the recordings are in its `runs/`.
- **Linux-Linux passed every step:** the direct link, a 256 MiB transfer that
  leaves the relay container's received bytes flat, a firewall block that
  sends the next one through the relay, and recovery.
- **Linux-Windows passed the same steps but one.** A Linux client
  downloaded from a Windows server. After the recovery restart, the direct
  link was Active on both ends again, but the restarted Linux client's DNS
  lookups got no answer (#271), so the last download failed. The cause
  was an expired visa, and the fix is in core (see "Visa expiry at match
  time (#271)" below) but not yet in a ZIPLINE build.
- **Windows was not tested as the client.** It couldn't reach the DNS Zode
  (#272): `ph` applied its ZPR address as a /64. The code fix (a /32, see
  SYSTEM_OVERVIEW.md, *Design decisions*) is in core but not yet in a
  ZIPLINE build, so Windows-client DNS on real machines is still unverified.
  The #271 retest is that check.
- **macOS has not been run.** The Mac was unavailable; the Linux-macOS and
  macOS-Windows pairs will be tested by hand later.

What the real machines showed beyond the netns test:

- **A Windows Zode must accept inbound substrate UDP.** Under rule 1 the
  lower ZPR address dials, so a Windows Zode registered after its peer must
  accept the peer's dial. Windows Firewall dropped it, the attempt ended
  `failed`, and rule 13a kept the pair on the relay. The MSI now installs an
  inbound rule for `ph.exe` on UDP 5000, on every profile
  (org-zpr/zipline#268; SECURITY_MODEL.md, *Implementation status*).
- **Recovery after a long block needs a reconnect.** A silent block is
  reported `down` only after keep-alive gives up (about 9-12 s). A block
  that then outlasts the one re-proposal (about 15 s from that first `down`
  report: `LINK_REPROPOSE_DELAY` plus the owner's `LINK_UP_WINDOW`) leaves
  the pair marked failed until either end reconnects (rule 13a). The demo restarts one Zode. A policy push does not
  clear the mark.
- **Block inbound only.** A host firewall that drops a Zode's *outgoing*
  substrate UDP makes the kernel fail `ph`'s send with EPERM, and `ph`
  panics on it (#270). The demo's block drops only what comes in from the
  peer, which silences the link both ways.

### Visa expiry at match time (#271)

Built in zl-zpr-core#128; no visa-service change. The design decision is "A
Zode enforces visa expiry when it matches a flow, with no sweep" above.

**The root cause** (pd plan p50, t300, reproduced on two Linux Zodes):
`ph` never enforced visa expiry when binding a flow (only J4's check of a
bootstrap hand-off). `VisaTable::handle_expirations` had no
production caller, and `match_traffic` did not check expiry. The visa
service forgets a visa when it expires, so the revocations a rejoin or a
link loss triggers (`end_previous_join`, `revoke_visas_over_link`) never
include an expired one. A Zode kept matching every new flow to its expired
copy. Once a next hop had lost its own copy (its link dropped, or it
restarted), every bind was refused ("visa N not found", a refused `Stream
ID Request`), and the Zode never asked for a new visa. A ZIPLINE network's
DNS visas live at most 120 s: the DNS rule needs the client's
`device.hostname`, whose trusted service sets `expiration_seconds = 120`. So
after an endpoint rejoined, the DNS Zode's expired reply visa `dns:53 ->
zode:*` dropped every answer. Restarting only the DNS container broke every
endpoint whose own DNS visa had expired, in the same way. netns did not
show it because no visa expired during a test.

**The fix**, as decided above: `match_traffic` ejects an expired visa and
looks again, and the ejected visa's adapter streams are withdrawn;
`request_stream` refuses a `Stream ID Request` for an expired visa it
holds, without ejecting or fetching it. There is no sweep
([#274](https://github.com/org-zpr/zl-zipline/issues/274)). The same PR made
a rebind superseded mid-flight an INFO line rather than a `RebindFailed`
(J11, gate q5). On the real hosts that misleading warning had pointed the
triage at J11.

| Concern | Location (`zl-zpr-core/`) |
|---|---|
| Expiry at a flow's ingress: `Visa::has_expired`, `match_traffic` (eject and look again), `eject_if_expired` through `revoke`; `handle_expirations` test-only | `adapter/ph/src/visa_table.rs` |
| The ejected visa's adapter streams withdrawn outside the lock (`bind_actor_address`, `Assembly::withdraw_streams`); a `Stream ID Request` for an expired held visa refused (`request_stream`); the J11 superseded-rebind INFO line (`rebind_granted`) | `adapter/ph/src/mgmt/node.rs`, `adapter/ph/src/assembly.rs` |
| Unit tests: `test_an_expired_visa_never_matches_a_new_flow`, `test_matching_an_expired_visa_ejects_it_cleanly`, `test_a_bind_matching_an_expired_visa_asks_for_a_new_visa`, `test_a_stream_request_for_an_expired_visa_is_refused`, `test_a_refused_rebind_with_no_newer_successor_is_a_failure` | `adapter/ph/src/visa_table.rs`, `adapter/ph/src/mgmt/node.rs` |
| netns: zb - relay - zc with a UDP/53 service on zc that needs zb's `device.hostname`, a file attribute that lives 90 s (fixture `pregen/zodes-rejoin`). After the visas expire, zb restarts (A), then zc (B); zb gets every reply and the other Zode logs the ejection. It fails on core 6e7baba | `integration-test/zode-rejoin-test.sh`, `integration-test/lib/udp_echo.py` |

**Known gap in the visa service**
([#275](https://github.com/org-zpr/zl-zipline/issues/275)). The visa
service refreshes an attribute only once it has expired, and denies a visa
whose lifetime would be under `MIN_VISA_LIFETIME` (30 s) with `NoReason`.
Its cause is logged only at DEBUG; the deny itself is counted and listed by
`GET /admin/visas/denies`. So in the last 30 s of each attribute lifetime,
unless a refresh of the attribute's source intervenes, every visa request that needs the attribute is denied: on ZIPLINE, a Zode's
DNS queries that no held visa covers, in the last 30 s of each 120 s
`device.hostname`. A query right after a rejoin can land there.
`zode-rejoin-test.sh` queries only after the visas expire, clear of that
window.

### What waits for which issue

- **#257** a visa granted for a next hop whose link is already down: its
  bind is refused, but the visa stays in the table until the visa service
  revokes it or it expires, so F6's re-request does not happen. It should be
  revoked locally, as F3 does for a link lost during a bind.
- **#258** an index of the visa-table bindings by five-tuple, so a supersede
  finds the bindings to move in O(1). Today an insert skips the scan in O(1)
  when this Zode holds no other visa for the five-tuple (gate g98), and scans
  the bindings otherwise.
- **#259** TLS on the VS-API and VSS connections that authenticates both ends,
  with ZIPLINE-provisioned per-customer certificates. Since gate g100 that TLS
  is the only end-to-end protection of a node's traffic to and from the visa
  service: its core visas carry no A2A keys.
- **#264** whether a Zode can misuse the substrate address it reports
  (rule 13b).
- **#270** `ph` treats an EPERM from a substrate send as fatal, so a host
  firewall that drops one peer's outgoing traffic crash-loops the Zode.
- **#273** deleting a service hosted on a Zode makes the visa service
  disconnect that Zode ("no longer approved by policy"); it rejoins about
  9 s later.
- **#274** a periodic visa expiry sweep. A Zode ejects an expired visa only
  when a new flow matches it (#271). Leaving aside an R16 reset, which
  drops every visa but the VS-API pair, an idle expired visa stays in the
  table until then: F3's binding-based cleanup on a link loss does not
  find it (rule 9's bootstrap visas are the exception: a lost initial
  link revokes its recorded hand-off). One with live streams also goes
  when a link they cross drops (F3).
- **#275** (visa service) attributes are refreshed only after they expire,
  so a visa request that needs one is denied in the attribute's last 30 s
  (see "Visa expiry at match time (#271)").
- **org-zpr/zipline#278** the generated policy allows DNS on UDP 53 only, so
  a resolver's TCP fallback is denied.
- **#234** retiring adapters: the `one-node-*` scripts still dock the visa
  service as an adapter on a node that is not a relay, and the relay's unused
  self-granted VS-API pair (#229) goes then.
- **#266** (visa service): the visa service should exit
  when a listener cannot bind `vs_addr`. Today it keeps running without
  VS-API, which is why the relay's startup order matters.
- **v2**: dialing a Zode that can't be reached (ZBOLT, rendezvous), unbounded
  background retries of direct links, more than one relay, and relays that
  don't host the visa service. The bounded retry, one re-proposal per `down`
  report, is in v1 (zl-zipline#268, visaservice#71, visa service
  `v0.22.0-zodes.9`).

## Related

- #251 node-to-node forwarding, #252 relay join, #253 gap 7, #254 netns
  scripts. These build on this lifecycle.
- #242 (removing "never reported = up") is folded into rule 14.
- v2: dialing a Zode that can't be reached, ZBOLT and rendezvous, unbounded retries.
