# Routing

ZPR nodes do not run a routing protocol. There is no OSPF, no BGP, no
adjacency discovery, no route advertisement. The topology of a ZPRnet is
**declared in the configuration description** (the `.zplc` file), compiled into
the policy binary, and handed to the visa service. The visa service is the only
component that computes paths, and it computes them **per visa, at issuance**:
the chosen route is frozen into the visa as a next hop.

A node therefore learns exactly one routing fact, and only for flows it is
authorized to carry: "for this visa, hand the packets to that neighbour."

Read this before changing the topology schema, the `Router`/`TopologyMgr` pair,
or how a visa's next hop is chosen — and before adding a second node to a test
setup.

## Authoritative sources

| Source | What it governs |
|---|---|
| **ZRFC 6.4** §5.12 "Route Generation and Distribution", §5.11.2 "Next Hop Selection" | The intended design. Two pages, and its own editorial note says routing "should be moved into a new document that just covers ZPR Routing" — that document does not exist. |
| **`zl-zpr-compiler/README_ZPLC.md`**, §Nodes / §Topology / §Link attributes | The topology schema as the compiler accepts it. |
| **`zl-zpr-policy/policy.capnp`** — `Peering`, `CPolicy.linkConds` | How topology and link constraints are carried in a compiled policy. |
| **`zl-zpr-visaservice/vs/src/router.rs`**, `topology_mgr.rs` | Path computation and the live graph, as implemented. |
| **`zl-zpr-visaservice/vs/src/visa_policy.rs`**, `visa_mgr.rs` | Route selection and how a route becomes per-node visas. |
| **`zl-zpr-vsapi/vs.capnp`** — `FwdPep`, `Link`, `setTopology` | The wire contract between visa service and node. |

Related: [ZPL.md](ZPL.md) for the `over` clause, [ZDP.md](ZDP.md) for what the
data plane does with a next hop, [TERMINOLOGY.md](TERMINOLOGY.md).

---

## The model

Three distinct things get called "routing" in ZPR. Keep them apart:

| | Where it lives | What it is |
|---|---|---|
| **Intended topology** | policy (from `.zplc`) | Every link an administrator has declared, with its attributes and cost. Static until a new policy is installed. |
| **Actual topology** | `Router`'s in-memory graph, persisted edges | The subset of declared links believed to be up right now. |
| **A route** | one visa | An ordered list of link IDs from the ingress node to the egress node, plus its total cost. |

*"Note that this is topology as it exists. For topology as it is intended to
be, you need to look at policy."* — `topology_mgr.rs`

A route is **not** a routing-table entry. It is a decision made once, for one
flow, and distributed as a next hop stamped into each node's copy of that
flow's visa. Nothing recomputes it in the data plane. If the topology changes,
the affected visas are revoked and the flow re-requests.

Flow setup is unidirectional, so each direction of a conversation gets its own
route, computed from its own ingress node. They need not be symmetric.

---

## Declaring topology in `.zplc`

```toml
[nodes.n0]
provider = [["device.zpr.adapter.cn", "node0.zpr.org"]]
zpr_address = "fd5a:5052:90de::1"

[nodes.n0.substrate_addrs]
i0 = "10.0.0.1:5000"

[nodes.n1]
provider = [["device.zpr.adapter.cn", "node1.zpr.org"]]
zpr_address = "fd5a:5052:90de::2"

[nodes.n1.substrate_addrs]
i0 = "10.0.0.2:5000"

[links.n0n1]
attributes = [["zpr.cost", "10"], ["#secure", ""], ["location", "usa"]]
peers = [{ node = "n0" }, { node = "n1", interface = "i0" }]
```

- **`provider`** is the attribute set an adapter must present to be recognized
  as that node. It cannot come from a trusted service — in practice it is
  `device.zpr.adapter.cn`, the CN of the node's Noise certificate.
- **`zpr_address`** identifies the node everywhere else in the system: it is the
  `NodeId`, the routing destination, and the key of every topology lookup. The
  node itself must be configured with the same address in its own `ph` config —
  nothing reconciles a mismatch.
- **`substrate_addrs`** are named interfaces (`i0`) in `HOST:PORT` form. `HOST`
  may be a hostname; the visa service resolves it via DNS when it loads the
  policy (`PolicyResolver::resolve_topology`). Multiple per node are supported
  in the schema; `peers` selects one with `interface = "i3"`. The `interface`
  key may be omitted only when the node has exactly one substrate address —
  with several it is a compile error, not a default.
- **`links.<LINKID>`** is one bidirectional edge with exactly two `peers`. The
  `<LINKID>` is a free-form string and travels all the way to the node in the
  `setTopology` message.
- **`attributes`** are key/value pairs, or **tags** written `["#name", ""]`.
  Giving a `#`-prefixed key a value is an error.

Link attributes land in the `link` attribute domain, so `zpr.cost` in the file
becomes **`link.zpr.cost`** in the compiled policy. **Every link gets a cost**:
if none is declared the compiler appends `zpr.cost = 1`, and
`DEFAULT_LINK_COST = 1` in `libeval` covers a link whose policy carries no cost
attribute. Costs are `u32` and simply summed along a path.

`README_ZPLC.md` §Nodes opens with "Only one node is supported currently in a
ZPRnet" and then documents multi-node topology below. Read it as a statement
about the data plane: the compiler and visa service both handle many nodes (see
§Implementation status). Relatedly, the undocumented `[visa_service]` section —
whose only key is `dock_node` — may be omitted, and the compiler fills it in on
the assumption there is one node (`zl-zpr-compiler` issue 100).

`README_ZPLC.md` states that only `device`, `user`, and `service` namespaces are
valid for attributes. That rule is about actor attributes; link attributes are
namespaced `link` by the compiler and are never resolved against a trusted
service — they are topology data, not vouched-for claims.

### `over` clauses are checked here

Because ZPL and ZPLC are compiled together, the compiler can check an `over`
clause against the declared links:

- naming a link **attribute** no configured link carries is an **error** — the
  statement could never match;
- naming a **value** no configured link carries, when the attribute does exist,
  is a **warning** (fatal with `--werror`) — topology data may legitimately
  gain that value later.

This is the only place link constraints are enforced today. See
[§Route constraints](#route-constraints-in-policy).

---

## From `.zplc` to the visa service

```text
.zplc  --[compiler]-->  policy.capnp        --[libeval]-->  visa service
[links.X]              topology: [Peering]                  peer_table
  peers                  linkId, nodeA/B (ZPR addrs)        link_attrs
  attributes             nodeASubstrate/nodeBSubstrate      resolved_peers (DNS)
                         attrs                              Router graph
```

The compiler builds a `FabricLink` per `[links.*]` entry (`weaver::add_topology`)
resolving each `(node, interface)` pair to a `NodeLinkAddr { zpr_addr,
substrate }`, and emits it as a `Peering` in `Policy.topology`.

`libeval::policy` turns those peerings into two lookups:

- **`peer_table`**: node ZPR address → `Vec<Peer>`, each `Peer` holding the link
  id, the remote node's ZPR address, the remote substrate address, and *this*
  node's own substrate address on that link. One peering produces an entry under
  both endpoints, each oriented from that endpoint's point of view.
- **`link_attrs`**: link id → attributes.

`Policy::describe_link(node_a, node_b)` combines them into the
`LinkDescription { link_id, attrs, cost }` that the router installs.

Two parts of `policy.capnp` exist for Zodes (see "Design decisions" and
"Implementation status" for what reads them today):

- **A `Peering` end may have no substrate address.** `nodeASubstrate` or
  `nodeBSubstrate` is left unset when policy does not know that node's
  substrate address, as for a laptop endpoint that changes networks. That node
  reports its own address when it authenticates (zpr-core#1381). Both ends may
  be unset, for a direct link between two endpoints. `zl-zpr-common` decodes an
  unset end as `None` (`Peering.substrate_a`/`substrate_b` are
  `Option<NetAddr>`), and writes `None` by leaving the field unset.
- **`Policy.transitNodes`** lists the nodes, by ZPR address, that may carry
  traffic between other nodes. It fails closed: a node that is not listed is
  not transit-capable, and an unset or empty list means no node is.

In the `.zplc`, a node with no `substrate_addrs` may still be a link peer, and
the compiler leaves its end of the `Peering` unset. Filling that end from the
address the node reports is not implemented yet (see "Implementation status"). A peer on such a node must
not name an `interface`. `transit = true` on `[nodes.<ID>]` puts the node in
`transitNodes`; the compiler writes the list sorted, and leaves it unset when no
node is marked (`zl-zpr-compiler/README_ZPLC.md`).

A `PolicySnapshot` additionally caches `resolved_peers_by_node`: the same peer
list with hostnames already resolved to `SocketAddr`. Resolution happens once,
at policy load — a failing lookup fails the whole policy install, and DNS
changes are not picked up until the next install.

---

## The live graph

`TopologyMgr` owns the in-memory `Router` graph and a `LinkRepo` that persists
node-link adjacency so the graph survives a restart. Nodes themselves are
persisted by `ActorMgr`/`NodeRepo`; only edges are persisted here.

A link becomes live when **both** of its endpoints are connected nodes:

1. A node authenticates over VSAPI (`connect`/`open`) and is added to the graph
   as a bare vertex (`add_node_if_not_exists`).
2. `install_policy_links_for_node` walks the peerings policy declares for that
   node and, for each peer already in the connected-node list, calls
   `add_linked_node` — which installs the edge in the router from the policy's
   `LinkDescription` and write-throughs the adjacency to state. Peers not yet
   connected are logged and deferred until they connect and run the same code.
3. On restart, `restore_from_state` rebuilds from persisted edges; on every
   policy install, `revalidate_against_policy` reconciles the union of persisted
   and live edges against the new policy — refreshing id/attrs/cost, garbage-
   collecting links policy no longer declares, repairing router/persistence
   drift, and reporting orphaned nodes.

Three caveats, all documented in the code as TODOs:

- **For a node that does not report link state, a declared peering between two
  connected nodes is taken as evidence the link is up.** The visa service cannot
  see which peer forwarded a node's VSAPI connection, so such a node gets edges
  for all its connected peers. Nodes that report (see "Link-state reports"
  below) replace that assumption with what they actually see; until the node
  side lands ([#223](https://github.com/org-zpr/zl-zipline/issues/223)) no node
  reports, and every link follows this legacy rule.
- **A failed `add_linked_node` is logged, not retried.** It leaves the edge in
  neither the router nor state, and `revalidate_against_policy` only works the
  union of those two — so the link stays missing until an endpoint reconnects.
  Meanwhile the node *is* still told the link exists (topology messages come
  from policy, not the router), so its visa requests over that link deny
  `NoRoute` with nothing node-side to explain why.
  (`zl-zpr-visaservice` issue 302.)
- **Multi-homed nodes are not supported** by `substrate_addr_from_topology`,
  which takes a node's own substrate address from the first resolved link it
  finds.

### Link-state reports

A node tells the visa service which of its links are up with
`VSHandle.reportLinkStates` (schema
[#217](https://github.com/org-zpr/zl-zipline/issues/217)). The visa service
implements it on `feature/zodes`
([#224](https://github.com/org-zpr/zl-zipline/issues/224)); the node side, which
sends the reports, is [#223](https://github.com/org-zpr/zl-zipline/issues/223).

- **`VSHandle.reportLinkStates(links :List(LinkStateReport))`.** Each
  `LinkStateReport` names a peer by its link-certificate CN (`peerCn`, the same
  value as `Link.peerCn`) and gives a `LinkState`: `up` (established, and the
  peer's certificate verified with that CN), `down` (was up, now lost or
  closed) or `failed` (could not be established: no answer, handshake or
  certificate refused).
- **Snapshot semantics.** Each call lists every link in the node's current
  topology plus its configured initial links, and replaces everything the node
  reported before. A link the node does not list is not up. The node sends a
  snapshot whenever a link changes state, and again after every (re)connect to
  the visa service.
- **What the visa service accepts.** A report from a live session, with at most
  as many entries as policy declares links for the node, each with a non-empty
  `peerCn` that appears once. Anything else is refused: `authRequired` for a
  dead handle, `paramError` otherwise, and nothing is stored. Entries are kept
  by CN exactly as sent and matched to a peer only when a link is evaluated,
  using the CN policy declares for the peer's address
  (`Policy::node_cn_for_addr`). So a report about a link that is not installed
  yet — the relay link reported before the endpoint has authenticated, or a
  link `setTopology` has not delivered — applies as soon as the link is. An
  entry whose CN matches none of the node's declared peers cannot make a link
  usable under the current policy; it is logged at debug, not refused, because
  a report can race a policy change. It is kept, so if a later policy declares a
  peer with that CN, the entry counts from then on.
- **The rule.** The router uses a link only while both of its ends agree it is
  up, so one node cannot keep a link usable that its peer denies — and either
  end can veto a link by reporting it down or leaving it out. An end that has reported
  agrees only if its latest snapshot lists the peer's CN as `up`; a peer whose
  CN policy does not know never matches. An end that has **never** reported
  agrees — the legacy rule that keeps today's nodes working until they report
  ([#223](https://github.com/org-zpr/zl-zipline/issues/223)); its removal is
  [#242](https://github.com/org-zpr/zl-zipline/issues/242).
- **What a change does.** A link that is not usable stays in the graph and in
  persisted state with its id, attributes and cost; both route queries simply
  skip it, so policy revalidation does not mistake it for drift, and a policy
  rebuild of the link keeps it down. When a link goes from up to down, every
  live visa whose recorded node path crosses it is marked `PendingRevoke`, VSS
  housekeeping revokes it, and the flow re-requests. It gets a route around the
  link if another usable path exists — the relay path, for a direct endpoint
  link — and is denied `NoRoute` otherwise. A visa whose route was chosen just
  before the link went down, but stored just after that pass ran, is caught as
  it is stored: `create_visa` re-checks every link of the route once the visa
  is committed, revokes it if one is down, and fails the issuance, so no
  `Allow` is ever answered with a visa on a downed route. If a revocation's
  state write fails, it is retried: for a link, by the next report from either
  end (the report that hit the failure is answered `internal`, so the node
  re-sends); for a visa caught at commit, by quarantining it — it is never
  handed to a node, and every pending-visa read retries its revocation. A link
  is installed with its flag under the link-state lock, so a report arriving
  at the same moment is never lost. A link coming back up affects new visas
  only; existing flows stay where they are until their visas expire or a later
  policy sweep moves them. The admin network view shows a declared link that
  reports put down as DOWN, although it stays in the graph.
- **Policy changes.** CNs come from policy, so a new policy can change which
  reports match a link even when the link itself is unchanged. Policy
  revalidation re-evaluates every link's flag and rebuilds any link whose flag
  changes in the same atomic batch. The links it takes from up to down, or
  removes while up, have their visas revoked the same way as a reported link
  going down: the policy sweep alone would miss a visa committed under the new
  policy just before the link went down, because its generation gate counts
  that visa as current.
- **Lifetime.** Reports live in the visa service's memory. A node's reports go
  when it disconnects or is reset. After a visa-service restart every node
  follows the legacy rule until it re-sends its snapshot, which it does after
  every reconnect.

### Substrate address changes (schema only)

`VSHandle.updateSubstrateAddr(addr :SockAddr)` tells the visa service that the
node's substrate address — the address peers dial, as in the `substrate_addr`
connect parameter — has changed. It travels over the existing VS-API session,
with no re-authentication. The visa service updates the node's address and
re-sends `setTopology` to the node's peers; an unspecified IP or port 0 is a
`paramError`. This is how an endpoint whose policy entry has no substrate
address (see "From `.zplc` to the visa service") keeps its peers current when
it changes networks. Not implemented: the node side is
[#223](https://github.com/org-zpr/zl-zipline/issues/223), the visa-service side
[#227](https://github.com/org-zpr/zl-zipline/issues/227).

---

## Path computation

`Router` wraps a `Graph` of `nodes: NodeId -> {edges}` and
`edges: LinkId -> {a, b, attributes, cost}` behind one `RwLock`. Two queries:

**`get_best_route(a, b)`** — lowest total cost; a tie goes to the route with
fewer hops, then to the one whose last link id is lexicographically smaller, so
the same graph always yields the same route. Served
from `Graph::best_routes`, an **all-pairs table recomputed by running Dijkstra
from every node** on each topology mutation. `a == b` short-circuits to a
`DirectSameNode` route: both actors dock on the same node, no link is traversed,
cost 0, and there is nothing to forward.

**`get_routes(a, b, hint)`** — *every* simple path, via an explicit-stack DFS
(no recursion limit), for the not-yet-implemented route-constraint evaluator.
This is the expensive one and it carries the interesting machinery:

- Results are memoized on `(src, dst, hint)`. Cache hits *and* DFS on a miss
  both run under a **read** lock, so lookups parallelize; only the insert takes
  the write lock.
- `topo_generation` is bumped on every mutation. If it changed while the DFS
  ran, the result may be stale: the call recomputes under the write lock and
  skips caching.
- `link_to_cache_keys` reverse-indexes which cached routes traverse each link,
  so **link removal** evicts exactly the affected entries. **Link addition**
  cannot use that index — a new edge can invalidate an entry whose routes never
  touched either endpoint — so `add_link` flushes the whole cache. The code
  documents the tighter bounds (topological horizon; and, for shortest-path
  caches only, the cost horizon of incremental SSSP) and defers them until
  profiling justifies the work.

`route_to_path(route, starting_node)` converts a link list into the ordered node
list, using the starting node to fix the direction.

---

## From route to visa

`visa_policy::evaluate_against_policy` is the single funnel — both the visa
request path and the policy-change re-check go through it, so a re-check applies
exactly the rules of the original decision.

1. Resolve each actor's **docking node**: the node its traffic enters the fabric
   at, from the connection table, falling back to the AAA table for fabricated
   anonymous actors. An undocked endpoint denies `SourceNotFound` /
   `DestNotFound`.
2. `get_best_route(docking_node_src, docking_node_dst)`. **No route at all
   denies `NoRoute`** before policy is even evaluated.
3. Evaluate policy. `AllowWithoutRoute` uses the best route as the default.
   `NeedsRoute` would enumerate candidate routes and evaluate each — see below.
4. `route_for_allow` takes the first hit's own route if it has one, else the
   default.

`path_for_flow` then orients the route into a **node path in flow order**:
ingress node first, egress last. It is anchored on the docking node of the
*source of the packets this visa authorizes*, deliberately **not** on the node
that asked for the visa — those coincide for an ordinary request but not for
visas the visa service mints pre-emptively on another node's behalf, and getting
the anchor wrong hands the destination node a next hop pointing back the way the
packets came. A `DirectSameNode` route yields no path.

`actualize_visa_for_target_node` then adapts one stored visa per node, reading
each node's role straight off its position in that path:

| Role | Position | What it gets |
|---|---|---|
| **Ingress** | first | Full visa (dock PEP, session keys) **plus** `FwdPep { next_hop }` |
| **Intermediary** | middle | `forwardOnly` visa: id and `FwdPep` only |
| **Egress** | last | Full visa, no `FwdPep` — the flow terminates here |

`FwdPep.next_hop` is the **next node's ZPR address**, not a link id and not an
interface. `FwdPep.symmetric` exists in the schema; the code always sets
`OneWay` with a `TODO: Not sure when to set this to symmetric`.

On policy install, `visa_reconciler` re-checks each live visa via
`recheck_visa_allowed`: denied → revoke; still allowed but the newly derived
path differs from the stored one → **revoke**. Routes are never edited in place.

---

## What the node does with it

Two halves, and only one of them is wired up.

**Visas — implemented.** `visa_mgmt::insert_visa` is the single entry point for
visas from the visa service, pushed or pulled. It checks that the visa's shape
matches its type (a full visa has a dock PEP; a `forwardOnly` visa has no dock
PEP and a `FwdPep` with a specified next hop) and refuses a malformed one rather
than trusting it. It stores the visa even if its next hop — `fwd_pep.next_hop`,
or the dock PEP's destination address when there is no `FwdPep` — has no link
yet, with a warning: the next hop is resolved to a link again, with
`Assembly::find_egress_link`, when a stream is bound, and a bind whose next hop
still has no link is refused (`DestNotFound`). `find_egress_link` matches the
address against the node's own ZPR addresses first, then against each peer's
registered actor addresses. From there forwarding is pure Stream ID switching
(see [ZDP.md](ZDP.md)); the next hop is consulted when a stream is bound, not
per packet.

**`forwardOnly` visas — implemented on the node, opt-in.** A node installs one
only if its `forwarding` setting is on: `forwarding = true` in the `[node]`
section of the `ph node` config file, or `ph node --forwarding true`, which
overrides the file. The default is off. With it off, `insert_visa` refuses the
visa with `ForwardingDisabled` and logs a warning; a pushed visa is then dropped
but still acknowledged, and a visa fetched for a bind fails that bind. A
`forwardOnly` visa has no five-tuple, so it never enters the five-tuple lookup
table and never matches a new flow. It is used only when the previous node on
the path sends a `Stream ID Request` naming it: the node asks the next hop for a
stream for the same visa and maps the upstream stream to the downstream one in
its PFT. An adapter at either end is refused, because an adapter binds by
traffic classifier, which a `forwardOnly` visa does not have; an adapter
requestor is refused before anything is asked of the next hop. ([zipline#219](https://github.com/org-zpr/zl-zipline/issues/219))

**`setTopology` — stored and checked, no links started.**
`vss_worker::process_topology` logs each link, stores the list as the node's
current topology (`Assembly.topology`), and closes any live Zode link whose
peer's certificate CN is no longer in it
([zipline#220](https://github.com/org-zpr/zl-zipline/issues/220)):

```rust
fn process_topology(asm: &Arc<Assembly>, links: Vec<Link>) -> SetTopologyResponse {
    ...
    *asm.topology.write().unwrap() = links;
    crate::link_state::close_zode_links_outside_topology(asm);
    Ok(())
}
```

So a node is told its peers, their substrate addresses, their ZPR addresses and
CNs — and, on the relay only, holds bootstrap visas for joining peers (see
"Bootstrap routing"). It stores that topology and uses its `peerCn`s to accept
or close Zode links (below), but it brings no link up: no node-to-node link is
ever started, and `LinkType::NodeToNode` is marked *currently unsupported* in
`link_state.rs`. So although an intermediary node can now install and bind a
`forwardOnly` visa, no path through one runs end to end.

**`Link.peerCn`.** Each `Link` in `setTopology` also carries `peerCn`: the CN
on the peer's CA-signed link certificate
([#217](https://github.com/org-zpr/zl-zipline/issues/217)). The receiving node
accepts a Zode link only from a peer whose verified certificate carries exactly
a `peerCn` in its current topology, and an empty `peerCn` (an older sender)
matches no peer. It checks when keying finishes, before Hello, and again after
every topology update, closing links whose CN left
([#220](https://github.com/org-zpr/zl-zipline/issues/220); see
SECURITY_MODEL.md). Because a deleted Zode disappears from every topology, this
is what makes deletion effective without a revocation list. The visa service
fills it ([#221](https://github.com/org-zpr/zl-zipline/issues/221)) from the peer's
node join policy, which pins the peer's `zpr.addr` and names its
`device.zpr.adapter.cn` (`Policy::node_cn_for_addr`). A peer whose address has
no CN in policy — no node join policy pins that address together with a
`device.zpr.adapter.cn`, or two pin it with different CNs — is left out of the
topology message, with a warning. The check still has nothing to act on: no
node-to-node link is started ([#222](https://github.com/org-zpr/zl-zipline/issues/222)).

The consequence is worth stating plainly: **the control plane computes
multi-hop routes that the data plane cannot yet use.** Everything above —
Dijkstra, all-paths DFS, path orientation, per-role visa actualization — is
built, tested, and exercised by unit tests, while `zl-zpr-core`'s integration
tests are `one-node-test.sh` and `one-node-v6-test.sh`. `zl-zpr-demo/multinode-demo`
is the two-node deployment being built out against this gap.

---

## Route constraints in policy

ZPL's `over` clause constrains which links a flow may traverse:

```zpl
allow redhead users to access database over secure, location:usa links.
never allow baldy users to access database over foreign links.
```

The full pipeline exists in outline and is disconnected at every runtime joint:

| Stage | State |
|---|---|
| Compiler parses `over`, checks satisfiability against declared links | **works** |
| Compiler emits `CPolicy.linkConds` into the policy binary | **works** |
| `libeval` reads `linkConds` | **never read** — no reference to the field anywhere in `zl-zpr-visaservice` |
| `RoutePredicate` (`DirectOnly`, `RequireLinkedPath`, `AnyLinkHas`, `NoLinkHas`, `AllLinksHave`, `And`, `Or`) | defined, never constructed |
| `PartialEvalResult::NeedsRoute` | defined, never returned — evaluation always yields `AllowWithoutRoute` or `Deny` |
| `RouteResidualEvaluator::eval_route` / `eval_routes` | `Err(InternalError("route evaluation not implemented"))`, with the intended algorithm written out in comments |
| `RouteHint`-based pruning in `Router::compute_routes` | hint accepted and ignored |
| `TopologyQueryApi::link_has_attr` on `TopologyMgr` | `return false` |

**An `over` clause compiles, is checked against the topology, and is then not
enforced.** A policy relying on one for security is not getting it. The
`link.zpr.cost` attribute is the sole exception: it is read, by
`describe_link`, as a number.

The design intent (`eval_route`'s comments): evaluate each candidate deny hit's
`RoutePredicate` against the route and re-check the actor conditions from the
cached actors; if any deny fires it is a deny; otherwise collect passing allow
hits. The two-phase split exists because the answer depends on which
permission matched —

```zpl
allow admins to access services over secure links.
allow employees to access services.          # implied: over any link
never allow admins to access services over insecure links.
```

— so evaluating a route requires knowing whether the actor matched as an admin
or merely as an employee.

---

## Bootstrap routing

A node that is not yet connected cannot be evaluated by policy — it has no
actor, and no route until its link is up — but it needs to reach the visa
service's VSAPI port to connect at all. `visa_bootstrap` resolves the circularity:

- A `setTopology` message **to the relay** — the node the visa service docks
  on — mints, per link, **two** visas for the *peer* (not the recipient): one for
  the peer's own SYN to VSAPI, one for the visa service's reply. Both are needed
  — with only the forward visa the reply is dropped, since no dock PEP matches it
  and no node on the forward path has a route back.
- They ride only in the `Link.visas` of the link whose `zprAddr` and `peerCn`
  name the joining peer, so the relay can bind them to the link whose
  certificate carries that CN (node side:
  [#222](https://github.com/org-zpr/zl-zipline/issues/222)).
- The relay holds them and hands them off when the peer shows up. The peer's
  own link is not in the router yet, so it is stitched on from the peering
  rather than routed.
- The relay may also *pull* its own copy when it sees the peer's traffic before
  its queued copy landed (`visa_for_future_peer_request`). Any other node gets
  neither: a link from a non-relay node to an unconnected peer carries
  `peerCn` and no bootstrap visas, so an endpoint never relays a joiner's
  traffic. Joining across more than one hop is deferred until the visa service
  runs on its own Zode.
- Minting deliberately bypasses policy evaluation: **a peering declared in
  policy is itself the authorization** for that peer to reach VSAPI. Only the
  visa service mints bootstrap visas; a node accepting none from a peer (none
  in Hello) is [#222](https://github.com/org-zpr/zl-zipline/issues/222).
- An already-connected peer yields no bootstrap visas — it has a VSAPI session
  and asks for what it needs.

This is the defined join path for a Zode in v1
([#221](https://github.com/org-zpr/zl-zipline/issues/221)); the earlier plan to
delete the module (`zl-zpr-visaservice` issue 301) is dropped.

---

## Addressing

Routing destinations are ZPR addresses, and where each kind comes from matters:

| | Source |
|---|---|
| **Node ZPR address** | `zpr_address` in `.zplc`, and the same value in the node's own config. Not allocated. |
| **Adapter/actor address** | Allocated by the visa service from its pool, or requested by the adapter via the `zpr.addr` claim (the one `zpr.*` claim an adapter may send). |
| **AAA address** | Used only during authentication. The ZPRnet AAA network is `fd5a:5052:0:aaa::/64`; each node gets a **/88** carved out using the low 24 bits of its node address, pushed to the node as the `AAA_PREFIX` configuration parameter and used to seed its local `AddressPool`. |
| **Static service address** | A service needing a fixed address sets `zpr_addr` in the adapter config *and* is granted the same address by a trusted service returning `device.zpr_addr` (a `file` store keyed on the adapter CN at minimum). An authored `["zpr.addr", ...]` provider attribute in `.zplc` is a compile error (zipline#109). |

ZRFC 6.4 §4.1 has node addresses as prefixes from which dock and tether
addresses are cut, so a route to a dock covers every tether on it. The
implementation does not do this: `NodeId` is a flat `IpAddr`, routes are
computed strictly between node addresses, and the docking node for an actor
comes from a table lookup rather than from prefix containment.

---

## Observability

- **`GET /admin/network`** returns every link the current policy declares
  between nodes, deduplicated by normalized node pair, each carrying
  `node_a_addr`/`node_b_addr`, both substrate addresses, `link_id`,
  `link_attrs`, and `link_cost` from the policy's link description — plus a
  `ctype`: `UP` if a connected node reports the link, `DOWN` if policy declares
  it but no node reports it, `INVALID` otherwise (and then with no substrate
  addresses, link id, or cost).
- The per-node admin endpoint lists that node's `adapters` and `links`.
- `zpr-dashboard` (TUI) and `vs-admin gui` render the same data;
  `demo-zpr-dashboard` in the multinode demo wraps it.
- Deny reason **`NoRoute`** in the deny log is the routing-specific failure:
  either an endpoint is undocked or no path exists between the two docking
  nodes.
- Counter `LinkInstallFailed` counts the silent-link-loss case described above.

---

## Design decisions

**An unset `Peering` substrate address means "the node reports its own".**
Policy cannot carry the substrate address of an endpoint that changes
networks, so the end is left unset rather than given a placeholder. Readers
must check `has_*()`: before `zl-zpr-common` made the field an `Option`, an
unset `NetAddr` decoded as hostname `""` port 0 and looked like a real address.
([org-zpr/zl-zipline#225](https://github.com/org-zpr/zl-zipline/issues/225),
[#218](https://github.com/org-zpr/zl-zipline/issues/218))

**Transit capability is a fail-closed list in policy.** `Policy.transitNodes`
names the nodes allowed to carry other nodes' traffic, so a subverted endpoint
cannot become a relay by turning forwarding on. A list was chosen over a
per-node struct because policy has no per-node entry today; nodes appear only
as link ends and keys.
([org-zpr/zl-zipline#225](https://github.com/org-zpr/zl-zipline/issues/225))

**Link-state reports are snapshots, not deltas.** Each `reportLinkStates`
call lists every link and replaces the previous report. A lost or reordered
delta would leave the visa service wrong until the next change; a snapshot
heals on the next call, and a reconnect needs no special replay. The cost is a
slightly larger message.
([org-zpr/zl-zipline#217](https://github.com/org-zpr/zl-zipline/issues/217))

**A link-state report carries no free-text reason.** `down` and `failed` are
enough for routing; diagnostics belong in the node's log, not in a field the
visa service would have to treat as untrusted input.
([org-zpr/zl-zipline#217](https://github.com/org-zpr/zl-zipline/issues/217))

**Reports are accepted for policy-declared links, not only delivered ones.**
A Zode's link to the relay is configured, not pushed, and it comes up before
the first `setTopology` arrives over it. Accepting entries for any link policy
declares for the node means that first report is not dropped; entries for
anything else cannot make a link usable under the current policy.
([org-zpr/zl-zipline#217](https://github.com/org-zpr/zl-zipline/issues/217))

**A peer's CN comes from its node join policy.** Policy has no node-to-CN
field. The compiler emits one node join policy per node, matching the node's
provider attributes (which name `device.zpr.adapter.cn`, the attribute
`authenticate_node` asserts) plus `zpr.addr eq <zpr_address>`, so the visa
service indexes that pair at policy load. An address pinned under two different
CNs is treated as unknown rather than letting policy order pick one. Rejected:
a new per-link CN field in policy (a schema change for a value policy already
carries), and the connected peer's actor CN (absent exactly when it matters,
for a joining node). ([#221](https://github.com/org-zpr/zl-zipline/issues/221))

**Bootstrap visas go only to the relay.** Any connected neighbour of an
unconnected node used to receive its bootstrap visas, so an endpoint with a
declared direct link to a laptop that had not joined yet would have carried the
laptop's VSAPI traffic. Limiting them to the node the visa service docks on
keeps endpoints non-transit and matches the v1 shape: every Zode is one hop
from the visa service through the relay. `Policy.transitNodes` was the
alternative, but it stays empty until the compiler fills it
([#226](https://github.com/org-zpr/zl-zipline/issues/226)), so nothing could
join until then. ([#221](https://github.com/org-zpr/zl-zipline/issues/221))

**Reports are stored by CN and resolved when a link is evaluated.** Resolving a
`peerCn` to an address when the report arrives would drop the relay-link entry
an endpoint's relay sends before the endpoint has authenticated, and under
snapshot semantics nothing would re-send it while the link stays up. Storing
the CN and matching it against policy's CN for the peer at evaluation time
applies the report whenever the link appears.
([#224](https://github.com/org-zpr/zl-zipline/issues/224))

**A link is usable only while both ends agree it is up.** Taking one end's
word would let a node keep a link usable that its peer has lost or refuses.
Requiring both ends means neither end can do that. The price is that either end
can veto a link: by reporting it `down` or `failed`, or by leaving it out of
its snapshot, a node takes the link down even while its peer still reports it
`up`. A faulty or subverted endpoint can therefore take down its own links and
divert their traffic to the relay path — which it could also do by dropping
packets on the link — but it cannot keep a link alive that its peer denies.
([#224](https://github.com/org-zpr/zl-zipline/issues/224))

**A down link stays in the graph.** Removing it from the router would make
policy revalidation reinstall it as drift between the router and persisted
state, and would lose its id and cost. A per-link up flag that route queries
skip keeps persistence untouched; a policy rebuild carries the flag over.
([#224](https://github.com/org-zpr/zl-zipline/issues/224))

**Link down revokes; link up does not.** A flow on a dead link is broken, so
its visas are revoked at once and it re-requests around the failure (no
re-heralding, ZRFC 6.4 §5.11, in v1). A flow on the relay path still works when
the direct link comes back, so moving it is left to its visa's expiry or a
later optimization.
([#224](https://github.com/org-zpr/zl-zipline/issues/224))

**Nodes that never report keep the old rule, for now.** Until the node side
lands ([#223](https://github.com/org-zpr/zl-zipline/issues/223)) no node
reports; failing closed would take every multi-node route down. A non-reporting
end agrees, and its peer can still veto by reporting the link down. The rule is
transitional and marked for removal by
[#242](https://github.com/org-zpr/zl-zipline/issues/242). Operator decision on
pd gate g43. ([#224](https://github.com/org-zpr/zl-zipline/issues/224))

**Policy cost decides direct versus relay; the router only breaks ties.** A
direct endpoint link at the default cost (1) already beats two hops through the
relay (1 + 1), so the visa service adds no Zode-specific preference. Ties go to
fewer hops, then to the smaller last-link id, so routing is deterministic.
([#224](https://github.com/org-zpr/zl-zipline/issues/224))

**Carrying other nodes' traffic is opt-in per node.** The `forwarding` setting
(`[node] forwarding`, `ph node --forwarding`) defaults to off, and a node with
it off refuses every `forwardOnly` visa. Nothing in the node's code says
"relay": a Zode relays because it is configured to. The visa service's routing
over `Policy.transitNodes` (above) is meant to be the primary non-transit
check; the node's refusal is defence in depth, and it surfaces a misconfigured
relay early. ([zipline#219](https://github.com/org-zpr/zl-zipline/issues/219))

**A refused pushed visa is acknowledged, not failed.** Reporting the push as
failed makes the visa service retry it and blocks every visa queued behind it.
The node logs the refusal and counts the op as processed instead. ([zipline#219](https://github.com/org-zpr/zl-zipline/issues/219))

**A `forwardOnly` visa never matches a new flow.** It has no dock PEP, so the
node keeps it out of the five-tuple lookup table and uses it only to bind a
stream that the previous node asks for by visa ID. ([zipline#219](https://github.com/org-zpr/zl-zipline/issues/219))

**Transit is marked on the node in the `.zplc`.** `transit = true` sits beside
the node's other settings and defaults to false, so a node is transit-capable
only if someone wrote that line for it. A top-level `transit_nodes` list was
rejected: it would be a second place to keep in step when a node is renamed.
The compiler turns the flags into `Policy.transitNodes`.
([org-zpr/zl-zipline#226](https://github.com/org-zpr/zl-zipline/issues/226))

**The compiler version did not move with the new fields.** The visa service
accepts only a policy whose compiler major and minor equal its minimum, so
bumping the compiler's minor alone would fail `zpr-dev build` gate 3 on the
feature branch. The fields only become load-bearing once the visa service
enforces transit: a policy from an older compiler then has no transit nodes and
every multi-hop path gets `NoRoute`. So the compiler minor and
`POLICY_MIN_COMPILER_*` move together in
[#227](https://github.com/org-zpr/zl-zipline/issues/227).
([org-zpr/zl-zipline#226](https://github.com/org-zpr/zl-zipline/issues/226))

---

## Implementation status

Specified or designed, not working:

- **Node-side topology.** `setTopology` is stored, and its `peerCn`s decide which
  Zode links a node accepts and which it closes
  ([zipline#220](https://github.com/org-zpr/zl-zipline/issues/220)), but no
  node-to-node link is started from it
  ([zipline#222](https://github.com/org-zpr/zl-zipline/issues/222)).
- **Paths through an intermediary node.** `forwardOnly` visas install and bind
  on a node with `forwarding` on ([zipline#219](https://github.com/org-zpr/zl-zipline/issues/219)), but with no node-to-node link no such
  path runs end to end; this is unit-tested in-process only. Two nodes with one
  link between them need only ingress and egress visas, which is why that is
  the frontier.
- **Non-transit routing in the visa service.** Path computation does not read
  `Policy.transitNodes` yet (see the `transitNodes` item below), so it may route through any
  node. Until it does, a node with `forwarding` off refusing `forwardOnly` visas
  is the only guard. See [SECURITY_MODEL.md](SECURITY_MODEL.md).
- **Route constraints** (`over`) are compiled and never enforced.
- **Next-hop selection modes** from ZRFC 6.4 §5.11.2 — a visa specifying an
  exact next hop, or a constrained set of permissible next hops, versus leaving
  the choice to the forwarder. The implementation always computes the whole path
  centrally and pins every hop.
- **Link/route liveness.** The visa service acts on `reportLinkStates` (see
  "Link-state reports"; [#224](https://github.com/org-zpr/zl-zipline/issues/224)),
  but no node sends it yet ([#223](https://github.com/org-zpr/zl-zipline/issues/223)),
  so every link still follows the legacy rule. The end-to-end check — block a
  direct link, the flow moves to the relay — waits for node-to-node links and
  the demo integration test
  ([#222](https://github.com/org-zpr/zl-zipline/issues/222),
  [#231](https://github.com/org-zpr/zl-zipline/issues/231)). ZRFC 6.4 §5.12
  sends these reports to the admin service and §5.11 re-heralds a visa onto a
  new path; the implementation folds the first into the visa service and
  replaces the second with revoke-and-re-request.
- **Backup links.** `LinkRole::backup` exists in the schema; only `Active` is
  ever sent.
- **Multi-homed nodes**, and equal-cost multipath (a tie picks one route
  deterministically; see "Path computation").
- **`FwdPep.symmetric`** — always `OneWay`.
- **Peerings without a substrate address, and `Policy.transitNodes`**: the
  compiler emits both on `feature/zodes`
  ([#226](https://github.com/org-zpr/zl-zipline/issues/226)), and the visa
  service does not use them yet. Since the `zl-zpr-common` pin move
  ([#238](https://github.com/org-zpr/zl-zipline/issues/238)) it decodes an
  unset end as `None` and skips that peer with a WARN, so the link gets no
  substrate address until the visa service fills it from the node's report. It
  does not read `transitNodes`, so path computation still routes through any
  node ([#227](https://github.com/org-zpr/zl-zipline/issues/227)).
- **`Link.peerCn`, `reportLinkStates` and `updateSubstrateAddr`** are in
  `vs.capnp` on `feature/zodes`
  ([#217](https://github.com/org-zpr/zl-zipline/issues/217)). The visa service
  fills `peerCn` from policy
  ([#221](https://github.com/org-zpr/zl-zipline/issues/221)) and the node checks it
  on Zode links ([#220](https://github.com/org-zpr/zl-zipline/issues/220)), but no
  Zode link is started yet. The visa service implements `reportLinkStates`
  ([#224](https://github.com/org-zpr/zl-zipline/issues/224)) and answers
  `updateSubstrateAddr` with the generated default ("unimplemented"); the node
  sends neither call yet. Implemented by
  [#222](https://github.com/org-zpr/zl-zipline/issues/222)–[#223](https://github.com/org-zpr/zl-zipline/issues/223)
  and [#227](https://github.com/org-zpr/zl-zipline/issues/227).

Also note that ZRFC 6.4 assigns route generation to the **Admin Service**, which
distributes `(egress address, next hop)` routes to docks and forwarders as a
routing table. The implementation folds that role into the visa service and
distributes next hops **per visa** instead. Nodes hold no routing table at all.

---

## Where the code lives

| Concern | Location |
|---|---|
| Topology schema (`nodes`, `substrate_addrs`, `links`) | `zl-zpr-compiler/src/config/mod.rs`, `config_api.rs` |
| Link → `Peering` emission, `over` satisfiability checks | `zl-zpr-compiler/src/weaver.rs`, `fabric.rs` |
| Topology and link-condition schema | `zl-zpr-policy/policy.capnp` (`Peering`, `CPolicy.linkConds`) |
| Peer table, link attributes, `describe_link`, default cost | `zl-zpr-visaservice/libeval/src/policy.rs` |
| Route and predicate types | `libeval/src/route.rs` |
| Route-constraint evaluator (unimplemented) | `libeval/src/eval_route.rs` |
| Graph, Dijkstra, all-paths DFS, route cache, link up/down flag | `zl-zpr-visaservice/vs/src/router.rs` |
| Link-state snapshots and the both-ends rule | `vs/src/link_state.rs`; applied by `vs/src/topology_mgr.rs`, handled in `vs/src/vsapi_worker.rs` (`report_link_states`), revocation in `vs/src/visa_reconciler.rs` (`revoke_visas_over_link`) |
| Live graph, persistence, policy revalidation | `vs/src/topology_mgr.rs`, `vs/src/db/link.rs` |
| DNS resolution of substrate hostnames | `vs/src/policy_mgr.rs` (`PolicyResolver`) |
| Docking-node resolution, route selection, `NoRoute` | `vs/src/visa_policy.rs` |
| Path orientation, per-node visa actualization, re-check | `vs/src/visa_mgr.rs` |
| Bootstrap visas for unconnected peers | `vs/src/visa_bootstrap.rs` |
| Link install on node connect | `vs/src/vsapi_worker.rs` |
| `setTopology` send path | `vs/src/vss_worker.rs` |
| Address pools, AAA prefixes | `vs/src/net_mgr.rs` |
| Wire contract (`FwdPep`, `Link`, `setTopology`) | `zl-zpr-vsapi/vs.capnp` |
| Node: visa install, next hop → egress link | `zl-zpr-core/adapter/ph/src/visa_mgmt.rs`, `visa_table.rs`, `assembly.rs` |
| Node: stream binding, incl. `forwardOnly` link to link | `zl-zpr-core/adapter/ph/src/mgmt/node.rs` |
| Node: `forwarding` setting | `zl-zpr-core/adapter/ph/src/config.rs`, `main_args.rs` |
| Node: `setTopology` receive and store; Zode-link CN check | `zl-zpr-core/adapter/ph/src/vss_worker.rs`, `link_state.rs`, `assembly.rs`, `libnode2/src/vss.rs` |
| Node: Zode-link certificate verification (both ends, validity window) | `zl-zpr-core/adapter/ph/src/km_noise.rs`, `km_multiplexor.rs`, `pki.rs` |
| Two-node deployment | `zl-zpr-demo/multinode-demo/` |

A change to the topology model usually touches all four repositories: the schema
in `zl-zpr-compiler`, the wire format in `zl-zpr-policy` and/or `zl-zpr-vsapi`, the graph
and route selection in `zl-zpr-visaservice`, and the consumer in `zl-zpr-core`. See
[REPOSITORIES.md](REPOSITORIES.md).
