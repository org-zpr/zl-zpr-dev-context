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
the compiler leaves its end of the `Peering` unset. The visa service then uses
the address the node reports when its VS-API session starts (see "Substrate
address changes"). A peer on such a node must not name an `interface`.
`transit = true` on `[nodes.<ID>]` puts the node in `transitNodes`; the
compiler writes the list sorted, and leaves it unset when no node is marked
(`zl-zpr-compiler/README_ZPLC.md`).

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

An installed link is not yet a usable one: the router routes over it only
once both ends report it up (see "Link-state reports" below). Every install
path — `add_linked_node`, `restore_from_state` and `revalidate_against_policy`
— starts a link down until then.

Two caveats, both documented in the code as TODOs:

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
([#224](https://github.com/org-zpr/zl-zipline/issues/224)), and a Zode sends it
([#250](https://github.com/org-zpr/zl-zipline/issues/250)). How a Zode brings
its links up, and what it reports when, is
[ZODE_LINKS.md](ZODE_LINKS.md); this section is the visa service's side.

- **`VSHandle.reportLinkStates(links :List(LinkStateReport))`.** Each
  `LinkStateReport` names a peer by its link-certificate CN (`peerCn`, the same
  value as `Link.peerCn`) and gives a `LinkState`: `up` (established, and the
  peer's certificate verified with that CN), `down` (was up, now lost or
  closed) or `failed` (could not be established: no answer, handshake or
  certificate refused).
- **Snapshot semantics.** Each call replaces everything the node reported
  before. A link the node does not list is not up. A Zode lists each link in
  its current topology, plus its initial link, that has an outcome
  (ZODE_LINKS.md "Reporting summary" and "Reporting contract"):
  - the owner of a topology link lists it `up`, `down` once an up link is lost,
    or `failed` once a 5-second link-up window ends without the link coming
    up. It lists nothing during the first window, or while a new
    `setTopology` retries a `failed` link. A link retried from `down` stays
    listed `down` during its window;
  - the other end lists it `up`, or `down` after a loss, and nothing before it
    was first up. It never lists `failed`;
  - the initial link is listed `up`, or `down` after a loss, and nothing before
    it was first up;
  - a link removed from the topology is not listed.

  The Zode sends a snapshot whenever the listed set changes (changes within
  100 ms go out as one), and again after every VS-API registration. It sends
  nothing while it has no VS-API session.
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
  end can veto a link by reporting it down or leaving it out. An end agrees
  only if its latest snapshot lists the peer's CN as `up`; a peer whose CN
  policy does not know never matches. **An end that has not reported counts as
  down** (ZODE_LINKS.md rule 14, `LinkStateTable::end_agrees`). This replaced
  #224's transitional "never reported = up" rule
  ([#242](https://github.com/org-zpr/zl-zipline/issues/242), done by
  [#250](https://github.com/org-zpr/zl-zipline/issues/250)).
- **Failed pairs (rule 13a).** The visa service owns the retry policy for
  direct links. A `failed` entry marks the unordered pair {reporter, the
  reporter's declared peer with that CN}, but only on a **new** failure: one
  the reporter's previous snapshot did not already list as `failed`
  (`LinkStateTable::record_failures`). A snapshot that repeats an old `failed`,
  sent for some other change, marks nothing, so a link proposed again is not
  dropped before it is tried. `topology_links`, which every `setTopology` goes
  through, leaves a marked pair out of **both** ends' topology, and dropping
  it closes the link on the nodes (rule 12). A `failed` report never itself
  sends topology, so a node and the visa service cannot loop. `up` and `down`
  set and clear no mark. `TopologyMgr::clear_failed_pairs_for` is the one entry
  point that clears a node's marks, when something about its pairs changed.
  Today that is the node (re)connecting its VS-API session (`authenticate` and
  `open`), and the other end of each cleared pair is sent `setTopology` again.
  A changed substrate address also arrives only that way (see "Substrate
  address changes"). The marks live in
  memory: a restart forgets them, so every link is proposed again.
- **What a change does.** A link that is not usable stays in the graph and in
  persisted state with its id, attributes and cost; both route queries simply
  skip it, so policy revalidation does not mistake it for drift, and a policy
  rebuild of the link keeps it down. When a link goes from up to down, every
  live visa whose recorded node path crosses it is marked `PendingRevoke`, VSS
  housekeeping revokes it, and the flow re-requests. It gets a route around the
  link if another usable path exists — the relay path, for a direct endpoint
  link — and is denied `NoRoute` otherwise. A visa whose route was chosen just
  before the link went down, but stored just after that pass ran, is caught as
  it is stored: `create_visa` re-checks the route once the visa is committed —
  every link must still exist, be up and join a hop of the visa's recorded node
  path, and that path must end at the destination's docking node (a policy
  change can move a link id to another edge) — and on any mismatch revokes the
  visa and fails the issuance, so no `Allow` is ever answered with a visa on a
  downed or changed route. If a visa's
  revocation write fails — when a link goes down or at commit — the visa is
  quarantined: it is never handed to a node, and every pending-visa read retries
  its revocation, independent of the link (which policy may remove meanwhile)
  and of further reports. A link is installed with its flag, and every report
  is applied through its last router update, under the link-state lock, so
  reports arriving at the same moment are never lost or applied out of order.
  A report is evaluated against one snapshot of the policy current when it is
  applied, read under that lock, so one link never mixes two policies' CNs.
  Reports are ordered by VS-API session: each session id carries a generation
  that only goes up, and a report from an older generation than the node's
  latest is refused, so a late report from a session the node has replaced
  (a soft reconnect keeps its router node) never overwrites the new session's.
  The old snapshot stands until the new session's first report, which a node
  sends after every reconnect. A link coming back up affects new visas
  only; existing flows stay where they are until their visas expire or a later
  policy sweep moves them. The admin network view shows a declared link that
  reports put down as DOWN, although it stays in the graph.
- **Policy changes.** CNs come from policy, so a new policy can change which
  reports match a link even when the link itself is unchanged. Policy
  revalidation re-evaluates every link's flag against the current policy and
  rebuilds any link whose flag changes in the same atomic batch. A pass whose
  policy was superseded while it ran applies nothing — no removals, no flag
  changes, no revocations — and leaves the topology to the pass for the current
  policy. The links it takes from up to down, or
  removes while up, have their visas revoked the same way as a reported link
  going down: the policy sweep alone would miss a visa committed under the new
  policy just before the link went down, because its generation gate counts
  that visa as current.
- **Transit changes.** A policy pass first takes transit away from every node
  the new policy no longer lists, under a short topology guard of its own and
  before any step that can fail: an error later in the pass ends it with no
  retry, so the router would otherwise keep routing through that node. It then
  revokes every live visa whose recorded path passes through a node that is
  not transit-capable. Transit the policy grants waits until the pass has
  reconciled the links, because the new policy may remove a link of that
  node; it is applied before the cut-off check below. The revocation checks
  every visa against the transit set as it is, not against what changed, so a
  later run finds whatever an earlier one missed. Startup runs it after
  restoring the topology: a service that stopped between a policy install and
  its pass restores a router with the new transit set, so no pass would see
  the change (`restore_topology`,
  `visa_reconciler::revoke_visas_through_non_transit`;
  [#255](https://github.com/org-zpr/zl-zipline/issues/255)).
- **A node the pass cut off gets nothing.** A policy pass pushes each node's
  topology, services and re-authentication request from a detached task of
  its own, so a slow node never delays the pass or the install queued behind
  it. A node at the end of a link the pass took down, or a node with a link up
  that a transit withdrawal left unreachable, with no route left from the node
  the visa service docks on (`event_mgr::nodes_cut_off`), is sent nothing:
  its VSS worker is stopped, and the node rejoins through its relay
  (ZODE_LINKS.md J10). Pushing to it would only wait out each call's timeout
  over a link that is gone (gate g101,
  [#252](https://github.com/org-zpr/zl-zipline/issues/252); VISA_SERVICE.md,
  "A policy pass never waits on a node's VSS").
- **One topology operation at a time.** The visa service runs its topology
  operations one at a time under a single lock, each held from the operation's
  first read to its last local write, including the visa revocations that write
  calls for. Those operations are: authentication and connect (from the policy
  recheck to the link install), `open` (actor lookup to session persist), a
  link-state report (from the call's arrival to the last revocation, so reports
  a node pipelines on one handle apply in the order sent), a policy pass
  (policy, live-node set and node validity through disconnecting invalid nodes,
  link reconciliation and revocations), a disconnect (actor lookup to the last
  state removal), and the startup restore. Policy installs commit under the same
  lock, so the policy an operation reads stays current until it finishes.
  Authentication can query trusted services, so it reads policy before taking
  the lock. Under the lock it checks that the policy it was approved under is
  still current, and refuses with `temporarilyUnavailable` if not; the node
  retries. A call a node makes on its VS-API handle (connecting a newcomer,
  `notifyDisconnect`, `registerVss`, a link-state report) rechecks under the lock
  that the handle's session is still current, and is refused with `authRequired`
  if a reconnect replaced it. A policy pass whose policy was replaced while it
  refreshed actor attributes applies nothing: the newer policy's own pass does
  the work. Node RPCs and event-queue sends run after the lock is released:
  stopping a disconnected node's VSS worker, `setTopology`, and `ActorJoins`.
  Visa issuance is the one path outside the lock. It is not a topology write,
  and the commit-time route check above covers it.
- **Lifetime.** Reports live in the visa service's memory. A node's reports go
  when it disconnects or is reset; after a reconnect, the new session's first
  report replaces them. After a visa-service restart every link is down until
  both its ends re-send their snapshots, which they do after every
  reconnect; `restore_from_state` installs the persisted links down and
  revokes the persisted visas over them.

### Substrate address changes

A node's substrate address — the address peers dial, as in the
`substrate_addr` connect parameter — comes from its VS-API session and is fixed
for the life of that session. An endpoint that changes networks loses its
initial link, redials from the new network and rejoins (ZODE_LINKS.md rule
11a), and the new session carries the new address in its `connect` or `open`.
There is no call to change it inside a session: `VSHandle.updateSubstrateAddr`
was removed from the schema in
[#252](https://github.com/org-zpr/zl-zipline/issues/252), after the node side
([#223](https://github.com/org-zpr/zl-zipline/issues/223)) was closed without
it.

`connect` and `open` both record the reported address through one rule,
`accept_reported_substrate`
([#255](https://github.com/org-zpr/zl-zipline/issues/255)):

- an unspecified IP or port 0 is refused with `paramError`; there is no
  fallback;
- if policy declares the node's address, the report must equal it, or the call
  is refused with `paramError`;
- otherwise the reported address stands.

A node's **known address** is its policy address, DNS-resolved, or failing that
the address it reported in its last `connect` or `open`, stored on its node
actor. Only the node's next `connect` or `open` replaces it: a rejoin keeps the
actor, so until then a direct peer may be proposed the old address, and the
pair falls to rule 13a if the dial fails (ZODE_LINKS.md, "Links from known
addresses (#255)"). The visa service puts it in `Link.peer` and
decides from it which links to propose (ZODE_LINKS.md rule 13b,
`vss_worker::proposed_links`). A relay link is always proposed, with `peer`
unset when the address is unknown. A direct link is proposed only when both
ends' addresses are known. If a lookup fails, the send fails and the topology
is sent again later: no address is guessed. When a node's session starts,
its policy peers that have a live VSS connection get a new `setTopology`
(`vsapi_worker::repropose_links_to_peers`). Whether a node can misuse the
address it reports is [#264](https://github.com/org-zpr/zl-zipline/issues/264).

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

**Only transit-capable nodes are passed through**
([#255](https://github.com/org-zpr/zl-zipline/issues/255)). The router holds a
transit set, the nodes policy lists in `Policy.transitNodes`. Both queries reach
any node, so any node can start or end a route, but they leave a node again only
if it is the route's start or in the set. A route through a node policy does not
list does not exist, and the request is denied `NoRoute`. The set starts empty, so
a router nobody has told about transit routes only over single links (fail
closed). Changing it recomputes the best routes and flushes the route cache under
the same write lock (`Router::set_transit_nodes`). The commit-time route check
(`route_matches`, through `route_still_valid`) also refuses a recorded path with
an interior node that is no longer transit-capable.

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
| **Ingress** | first | Full visa (dock PEP, session keys) **plus** `FwdPep { next_hop }`; no `prevHop` |
| **Intermediary** | middle | `forwardOnly` visa: id, `FwdPep`, and `prevHop` |
| **Egress** | last | Full visa, no `FwdPep` — the flow terminates here — and `prevHop` |

`Visa.prevHop` (schema `@6`, [zipline#243](https://github.com/org-zpr/zl-zipline/issues/243))
is the **previous node's ZPR address** on the path, `path[pos-1]`. A node
accepts a `Stream ID Request` for a visa only on the link whose peer's topology
ZPR address equals `prevHop`, or, on a joiner until a `setTopology` names its
relay, on the initial link whose bootstrap hand-off recorded `prevHop` as the relay's
address (the F1 lookup in [ZODE_LINKS.md](ZODE_LINKS.md), used both ways). The
ingress visa has none and never accepts one (gap 7). It sits on `Visa` rather than `FwdPep` because the egress visa — where a
`Stream ID Request` matters most — carries no `FwdPep`. The visa service sets it on
each copy it actualizes, bootstrap visas included ([zipline#245](https://github.com/org-zpr/zl-zipline/issues/245)); a visa
whose path has fewer than two nodes has none. Core enforces it since [zipline#253](https://github.com/org-zpr/zl-zipline/issues/253).
Reverse-direction binding for symmetric visas is undefined and refused
([zipline#246](https://github.com/org-zpr/zl-zipline/issues/246)).

`FwdPep.next_hop` is the **next node's ZPR address**, not a link id and not an
interface. `FwdPep.symmetric` exists in the schema; the code always sets
`OneWay` with a `TODO: Not sure when to set this to symmetric`.

On policy install, `visa_reconciler` re-checks each live visa via
`recheck_visa_allowed`: denied → revoke; still allowed but the newly derived
path differs from the stored one → **revoke**. Routes are never edited in place.
A bootstrap visa is the exception: its path was stitched from the peering,
not routed, so it is rechecked against the peering, not the route, and kept
while the relay's topology still names the joiner (gate g94; see "Bootstrap
routing").

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

**`setTopology` — drives the Zode's links.**
libnode2 decodes each `Link` on its own: one without `zprAddr` or with an empty
`peerCn` is logged and skipped, and the rest are applied; the call still answers
`ok` (`vss.rs`, `decode_topology_links`). `vss_worker::process_topology` logs
each link, stores the list as the node's current topology (`Assembly.topology`),
and hands it to `zode_links::apply_topology`
([#250](https://github.com/org-zpr/zl-zipline/issues/250)):

```rust
fn process_topology(asm: &Arc<Assembly>, links: Vec<Link>) -> SetTopologyResponse {
    ...
    *asm.topology.write().unwrap() = links;
    crate::zode_links::apply_topology(asm);
    Ok(())
}
```

`apply_topology` keeps one link record per peer. A link that left the topology
is closed and forgotten (ZODE_LINKS.md rule 12). A new link this Zode owns is
dialed (rule 6), and a `failed` or `down` link still in the topology gets one
more attempt (rule 13); links that are up are untouched. The owner is the end
with the lower ZPR address; a relay owns nothing and never dials. So a node is
told its peers, their substrate addresses, their ZPR addresses and CNs, and
brings its links up from that. Topology carries no visas: when a joiner's link
to the relay comes up, the relay asks the visa service for the joiner's
bootstrap visas (see "Bootstrap routing"). Forwarding over a Zode link is
implemented ([#251](https://github.com/org-zpr/zl-zipline/issues/251),
ZODE_LINKS.md "Forwarding over Zode links"): a visa's next hop resolves to the
up Zode link whose peer has that topology address, and an intermediary node
binds a `forwardOnly` visa from one Zode link to the next. It is covered by unit
tests and core CI only. With the relay join
([#252](https://github.com/org-zpr/zl-zipline/issues/252)) every Zode gets its
own VS-API session, and a joiner's VS-API flow crosses its link to the relay
in netns; a flow between two Zodes across a Zode link is
[#254](https://github.com/org-zpr/zl-zipline/issues/254).

**`Link.peerCn`.** Each `Link` in `setTopology` also carries `peerCn`: the CN
on the peer's CA-signed link certificate
([#217](https://github.com/org-zpr/zl-zipline/issues/217)); it is required, and
a link without it is skipped. The receiving node accepts an inbound Zode link
only from a peer whose verified certificate carries exactly a `peerCn` in its
current topology, and only if that peer owns the link. It checks when keying
finishes, before Hello, and closes the links whose peer left the topology on
every update ([#220](https://github.com/org-zpr/zl-zipline/issues/220),
[#250](https://github.com/org-zpr/zl-zipline/issues/250); see SECURITY_MODEL.md
and ZODE_LINKS.md rules 3 and 12). Because a deleted Zode disappears from every topology, this
is what makes deletion effective without a revocation list. The visa service
fills it ([#221](https://github.com/org-zpr/zl-zipline/issues/221)) from the peer's
node join policy, which pins the peer's `zpr.addr` and names its
`device.zpr.adapter.cn` (`Policy::node_cn_for_addr`). A peer whose address has
no CN in policy — no node join policy pins that address together with a
`device.zpr.adapter.cn`, or two pin it with different CNs — is left out of the
topology message, with a warning.

The consequence is worth stating plainly: **the control plane computes
multi-hop routes that no netns run has carried end to end yet.** Everything
above — Dijkstra, all-paths DFS, path orientation, per-role visa actualization —
is built, tested, and exercised by unit tests, and the data plane can forward
over Zode links (#251, unit-tested), while no `zl-zpr-core` integration test
runs a flow across a Zode link (that is #254). `zl-zpr-demo/multinode-demo`
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
service's VSAPI port to connect at all. `visa_bootstrap` resolves the
circularity, on demand
([#252](https://github.com/org-zpr/zl-zipline/issues/252); ZODE_LINKS.md
"Joining through the relay"):

- **The relay hands off.** When a joiner's initial link comes up, the relay
  (the node the visa service docks on) calls `requestBootstrapVisas` with the
  CN from the joiner's verified certificate. The visa service mints **two**
  visas: one for the joiner's own SYN to VSAPI, one for the visa service's
  reply. Both are needed: with only the forward visa the reply is dropped,
  since no dock PEP matches it and no node on the forward path has a route
  back. The relay installs its own copies, bound to that link, and hands the
  joiner its copies in ZDP message 146.
- **The joiner's address comes from the relay's topology**, the `zprAddr` it
  gives that CN, never from the joiner.
- **Stitched, not routed.** The joiner's link is not in the router yet (it has
  not reported), so the path is stitched from the peering, `[joiner, relay]`,
  reversed for the reply. Each Zode's copy is cut to its role as usual: the
  joiner's forward copy has a `FwdPep` whose next hop is the relay, and the
  relay's copy of the forward visa has `prevHop` set to the joiner. Until its
  first `setTopology` names the relay, the joiner finds its egress for that
  next hop on the initial link.
- **Only the relay.** Any other caller is refused, so a non-relay neighbour of
  an unconnected node never carries a joiner's traffic. Joining across more
  than one hop is deferred until the visa service runs on its own Zode.
- **The peering is the authorization.** Minting bypasses policy evaluation,
  and a policy sweep rechecks the visas against the peering, not the route
  (gate g94).
- **Short-lived.** They live 5 minutes. Once the joiner has authenticated, the
  visa service issues its own VS-API visas, which are routed like any visa and
  supersede the bootstrap pair (ZODE_LINKS.md J5a, J11).
- **Every request is a join** and mints new visas, after revoking every visa
  between the joiner and the visa service. Nothing is minted ahead of time or
  from the joiner's traffic, and topology carries no visas (`Link.visas` is
  gone).

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
Since #252 the rule is enforced where the visas are asked for: only the relay
may call `requestBootstrapVisas`
([#252](https://github.com/org-zpr/zl-zipline/issues/252)).

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

**Topology operations are serialized under one lock.** Every operation that
reads policy, link, actor or session state to decide a topology, link-state or
session write holds one lock (`TopoOps` in `topology_mgr.rs`) from its first read
to its last write, revocations included. Every mutating `TopologyMgr` method takes
the guard as an argument, so an unlocked topology mutation does not compile.
Policy installs commit under the same lock (`PolicyMgr::bind_topology_lock`).
They take it only around the local commit, after the network-bound build, so a
guard holder's policy cannot change underneath it.

The alternative was a generation or session recheck before each write. That has
to be repeated correctly after every await on every path, and nine review rounds
on #224 each found one more path that missed it. With the lock, a reviewer
checks one rule, and the compiler covers the mutations. The cost is one global
lock over operations that are rare: node connects and disconnects, link-state
reports and policy installs.

Rules that come with it:
- Read policy, actor and session state only after taking the lock.
- The lock is not re-entrant: a caller that holds it passes the guard down
  (`disconnect_locked`, the authentication undo).
- An operation holding it waits only on local state writes. It never waits on
  the visa request queue, a trusted service, an event-queue send or a node RPC:
  those can be slow, and can wait on another operation that wants the lock.
- An actor attribute refresh queries trusted services outside the lock and
  commits under it. The commit applies only the attributes it changed onto the
  actor as stored then, and only if that is still the same actor (identity, CN
  and role), so it never rolls back a session or claim written meanwhile, nor
  hands one actor's claims to another at a recycled address. Services are
  recomputed at commit under the current policy.
- Work that must read policy before the lock because it waits on the network
  rechecks under the lock that the policy is still current. Today that is
  authenticating a node against trusted services.

Visa issuance stays outside the lock and is closed by ordering: a link going
down sets its flag before its revocation pass lists visas. A visa stored before
the listing is revoked by the pass, and one stored after it fails its
commit-time route check, which also refuses a visa revoked while it was being
issued even if a down-and-up cycle made its route valid again. Such a revocation clears the visa's policy stamp, and
a later policy deny leaves it unstamped, so no later policy allow can cancel it.
The grant itself is not yet atomic against revocation: the final check is a
read taken before the reply, and a revoke sent over VSS can overtake its grant
in the VS-API reply. Deferred to
[zl-zipline#248](https://github.com/org-zpr/zl-zipline/issues/248).
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

**An unreported link counts as down.** #224 let an end that had never reported
agree, so that links kept working until nodes reported (operator decision on
pd gate g43). Once Zodes report
([#250](https://github.com/org-zpr/zl-zipline/issues/250)), that rule only
lets a link be routed before anyone has seen it come up, so it is gone: an end
with no report agrees to none of its links (ZODE_LINKS.md rule 14). After a
visa-service restart this takes every link down until both ends report again,
which they do on reconnect.
([#242](https://github.com/org-zpr/zl-zipline/issues/242), done by #250)

**The visa service owns the retry policy for direct links.** A Zode tries a
topology link once per `setTopology` and keeps no retry logic of its own. The
visa service remembers pairs reported `failed` and leaves them out of both
ends' topology until something about the pair changes (ZODE_LINKS.md rule 13a).
A `failed` report never sends topology itself, so the two cannot loop. Only a
**new** failure marks a pair: snapshots repeat entries, and a repeated `failed`
from before the link was proposed again must not drop the retry before it is
tried. This also is the v2 hook: once a rendezvous step says a pair can try
again, the visa service just proposes the link again.
([#250](https://github.com/org-zpr/zl-zipline/issues/250))

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
They did, in [#256](https://github.com/org-zpr/zl-zipline/issues/256): zplc
0.21.0 and a visa-service minimum of 0.21.0, together with the enforcement
([#255](https://github.com/org-zpr/zl-zipline/issues/255)).

**A policy pass withdraws transit at once and grants it late.** Taking transit
away can only remove routes, so it is applied first, before anything in the
pass can fail. Granting it before the links are reconciled could build a route
through the node over a link the new policy has removed, so a grant waits for
the reconciliation. Until then the node stays non-transit, which fails closed.
([#255](https://github.com/org-zpr/zl-zipline/issues/255), visaservice#68
review)

---

## Implementation status

Implemented on `feature/zodes`:

- **Node-side topology** drives Zode links
  ([#250](https://github.com/org-zpr/zl-zipline/issues/250), see
  [ZODE_LINKS.md](ZODE_LINKS.md) "Implementation status").
- **The relay join**
  ([#252](https://github.com/org-zpr/zl-zipline/issues/252)): every Zode gets
  a VS-API session over its initial link, so each receives `setTopology` and
  reports its own links. Bootstrap visas are requested on demand (see
  "Bootstrap routing"), `Link.visas` is gone, and a policy pass sends nothing
  to a node it cut off. `zode-link-test.sh` runs the join and the rejoin in
  netns.
- **Non-transit routing in the visa service**
  ([#255](https://github.com/org-zpr/zl-zipline/issues/255)). Routes pass only
  through nodes in `Policy.transitNodes` (see "Path computation"). A policy pass
  and startup revoke the visas through a node that is not transit-capable (see
  "Policy changes" and "Transit changes" under "Link-state reports").
- **Peerings without a substrate address, and reported addresses**
  ([#255](https://github.com/org-zpr/zl-zipline/issues/255)). A peer with no
  policy address is kept, and its link uses the address the node reported in
  `connect` or `open` (see "Substrate address changes", ZODE_LINKS.md rule 13b).
  `zode-link-test.sh` joins a Zode that has no policy address.

Specified or designed, not working:

- **Paths through an intermediary node, end to end.** `forwardOnly` visas
  install and bind on a node with `forwarding` on ([zipline#219](https://github.com/org-zpr/zl-zipline/issues/219)),
  and forwarding over Zode links is implemented ([#251](https://github.com/org-zpr/zl-zipline/issues/251)),
  but only unit tests and core CI exercise it: no such path has run end to
  end in a netns run. The relay join it needed is in place
  ([#252](https://github.com/org-zpr/zl-zipline/issues/252)); the scenario is
  [#254](https://github.com/org-zpr/zl-zipline/issues/254).
- **Route constraints** (`over`) are compiled and never enforced.
- **Next-hop selection modes** from ZRFC 6.4 §5.11.2 — a visa specifying an
  exact next hop, or a constrained set of permissible next hops, versus leaving
  the choice to the forwarder. The implementation always computes the whole path
  centrally and pins every hop.
- **Link/route liveness.** The visa service acts on `reportLinkStates` (see
  "Link-state reports"; [#224](https://github.com/org-zpr/zl-zipline/issues/224)),
  and Zodes send it ([#250](https://github.com/org-zpr/zl-zipline/issues/250));
  `zode-link-test.sh` checks both ends' reports in netns. The end-to-end
  check — block a direct link, the flow moves to the relay — is
  [#254](https://github.com/org-zpr/zl-zipline/issues/254), and the demo
  integration test
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
| Graph, Dijkstra, all-paths DFS, route cache, link up/down flag, transit set | `zl-zpr-visaservice/vs/src/router.rs` |
| `Policy.transitNodes` loaded (`is_transit_node`) | `zl-zpr-visaservice/libeval/src/policy.rs` |
| Transit in a policy pass (`withdraw_policy_transit`, `apply_policy_transit`) and at startup (`restore_topology`); revoking visas through a non-transit node | `vs/src/event_mgr.rs`, `vs/src/main.rs`, `vs/src/visa_reconciler.rs` (`revoke_visas_through_non_transit`) |
| A reported substrate address (`accept_reported_substrate`); known addresses and which links are proposed (`proposed_links`); re-proposal on session start | `vs/src/connection_control.rs`, `vs/src/vss_worker.rs`, `vs/src/vsapi_worker.rs` (`repropose_links_to_peers`) |
| Link-state snapshots, the both-ends rule, failed pairs (rule 13a) | `vs/src/link_state.rs` (`end_agrees`, `record_failures`), `vs/src/topology_mgr.rs` (`clear_failed_pairs_for`), `vs/src/vss_worker.rs` (`topology_links`); applied by `vs/src/topology_mgr.rs`, handled in `vs/src/vsapi_worker.rs` (`report_link_states`), revocation in `vs/src/visa_reconciler.rs` (`revoke_visas_over_link`) |
| Live graph, persistence, policy revalidation | `vs/src/topology_mgr.rs`, `vs/src/db/link.rs` |
| DNS resolution of substrate hostnames | `vs/src/policy_mgr.rs` (`PolicyResolver`) |
| Docking-node resolution, route selection, `NoRoute` | `vs/src/visa_policy.rs` |
| Path orientation, per-node visa actualization, re-check | `vs/src/visa_mgr.rs` |
| Bootstrap visas for a joining peer (`requestBootstrapVisas`) | `vs/src/visa_bootstrap.rs`, `vs/src/visa_mgr.rs` (`mint_bootstrap_visa`) |
| Policy-pass pushes; nodes cut off by a removed link | `vs/src/event_mgr.rs` (`push_policy_to_nodes`, `nodes_cut_off`) |
| Node: the relay's request and the joiner's hand-off (ZDP 146) | `zl-zpr-core/adapter/ph/src/bootstrap_handoff.rs` |
| Link install on node connect | `vs/src/vsapi_worker.rs` |
| `setTopology` send path | `vs/src/vss_worker.rs` |
| Address pools, AAA prefixes | `vs/src/net_mgr.rs` |
| Wire contract (`FwdPep`, `Link`, `setTopology`) | `zl-zpr-vsapi/vs.capnp` |
| Node: visa install, next hop → egress link | `zl-zpr-core/adapter/ph/src/visa_mgmt.rs`, `visa_table.rs`, `assembly.rs` |
| Node: stream binding, incl. `forwardOnly` link to link | `zl-zpr-core/adapter/ph/src/mgmt/node.rs` |
| Node: `forwarding` setting | `zl-zpr-core/adapter/ph/src/config.rs`, `main_args.rs` |
| Node: `setTopology` receive and store; Zode-link CN check | `zl-zpr-core/adapter/ph/src/vss_worker.rs`, `link_state.rs`, `assembly.rs`, `libnode2/src/vss.rs` |
| Node: Zode link owner, acceptance, per-link record, `setTopology` application, link-state snapshots and reports | `zl-zpr-core/adapter/ph/src/zode_links.rs`, `zode_link_record.rs`, `libnode2/src/vsconn.rs` (`report_link_states`) |
| Node: Zode-link certificate verification (both ends, validity window) | `zl-zpr-core/adapter/ph/src/km_noise.rs`, `km_multiplexor.rs`, `pki.rs` |
| Two-node deployment | `zl-zpr-demo/multinode-demo/` |

A change to the topology model usually touches all four repositories: the schema
in `zl-zpr-compiler`, the wire format in `zl-zpr-policy` and/or `zl-zpr-vsapi`, the graph
and route selection in `zl-zpr-visaservice`, and the consumer in `zl-zpr-core`. See
[REPOSITORIES.md](REPOSITORIES.md).
