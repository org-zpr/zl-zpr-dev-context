# Zode links: lifecycle (v1)

**Status:** DESIGN DRAFT for zl-zipline#250 (2026-11-12). Not yet implemented.

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

## Left to the builder

- The link-up timeout: 5 s is the default (my call). It is a constant with a comment.
- The initial-link backoff schedule: start around 1 s, double, cap around 30 s,
  with jitter (my call).
- Where the per-link state machine lives in `ph`, and its exact state names. The
  states have to cover at least: dialing, keying, CN check, up, closing, closed.
  Every transition gets a unit test, and the rules above are the acceptance criteria.
- How rules 9 and 12 find "state tied to a link". Recommended: hang everything
  link-scoped off one per-link object, so dropping it clears everything.

## Related

- #251 node-to-node forwarding, #252 relay join, #253 gap 7, #254 netns
  scripts. These build on this lifecycle.
- #242 (removing "never reported = up") is folded into rule 14.
- v2: dialing a Zode that can't be reached, ZBOLT and rendezvous, retries.
