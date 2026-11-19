# DNS in ZPR

How a name inside a ZPRnet becomes a ZPR address. This is the orientation
document: read it before touching the resolver, the hosts index, or a demo's
`Corefile`, then go to the code it points at.

**Authoritative sources.** The code wins. Then, in order:
`zl-zpr-coredns/README.md` (the plugin's own reference — Corefile syntax,
name semantics, resolution order) and
`zl-zpr-visaservice/admin-http-api.txt` (the three endpoints). This
document summarizes both and does not restate their detail; why the design
is shaped this way is under [Design decisions](#design-decisions).

---

## The model

DNS in ZPR is a **read-only view of the visa service's live state**, served
over the overlay by an ordinary DNS server. There is no zone file, no
secondary, and no push: every query becomes an HTTPS call to the visa
service's admin API, and short TTLs are what make the answer track reality.

```
 ┌──────────┐  DNS UDP/53         ┌────────────────────────┐  HTTPS TCP/8182   ┌──────────────┐
 │ client   │  visa: client →     │  CoreDNS + zpr plugin  │  visa: zpr-dns →  │  admin API   │
 │ (Zode)   │  zpr-dns service    │  granted ZPR address   │  vs-admin service │ fd5a:5052::1 │
 └──────────┘ ───────────────────▶└────────────────────────┘ ─────────────────▶└──────────────┘
   dig AAAA web.demo                GET /admin/services/web        200 { zpr_addr }
                                    then /admin/hosts/web on 404
```

Both hops are ZPR flows governed by policy and carried by visas. The resolver
is not privileged infrastructure: it is an actor on the ZPRnet that holds a
visa to the `vs-admin` service, and it is reachable only by clients holding a
visa to the `zpr-dns` service.

**A name never authorizes.** Resolution tells you *where* something is;
whether you may reach it is still decided by policy, per protocol and port,
at visa issuance and at enforcement. The hostname index is never consulted by
the enforcement path. Resolving `webhost.demo` and then being denied ICMP6 is
the system working correctly.

## Two namespaces, one flat name space

| Kind | Source of truth | Claimed how |
|---|---|---|
| **Service name** (`web`) | the compiled policy's `[services.*]` plus a live provider | declared by the policy author |
| **Machine name** (`webhost`) | the `device.hostname` attribute on an actor record | vended by a trusted service ("naming authority") at authentication |

Both are a **single label under the server's zone**, in one flat namespace.
The service lookup runs first and always wins; the host lookup is attempted
only on a clean 404. Collisions are prevented at claim time, not at
resolution time: a hostname claim equal to a policy service name is rejected,
and installing a policy that introduces such a service releases the held
name. `a.b.<zone>` is NXDOMAIN with no HTTP call.

Names are matched **exactly as stored** — a single lowercase DNS label,
`[a-z0-9]([a-z0-9-]*[a-z0-9])?`, 1–63 bytes. The plugin lowercases the query
label; nothing else is transformed on either side. An invalid hostname claim
is rejected and logged, never mangled into a valid one, so it is simply never
in the index.

## The four moving parts

| Part | Repository | Role |
|---|---|---|
| The `zpr` CoreDNS plugin | `zl-zpr-coredns` | Answers `AAAA <name>.<zone>` by calling the admin API. Stateless per query; the stock `cache` plugin sits in front of it. The only Go repository, and not a fork — its branch is `main`. |
| The resolve surface of the admin API | `zl-zpr-visaservice` | `GET /admin/services/{name}`, `GET /admin/hosts/{name}`, `GET /admin/services`. Nothing else. |
| The hostname index | `zl-zpr-visaservice` | `host:<NAME>` → actor ZPR address, filled from authenticated `device.hostname` claims, first-claim-wins per value. |
| The naming authority | operator-side | A trusted service that returns `hostnames -> device.hostname{}`. In `dns-demo` this is the `file` api over `machines.json`; in production it is the control plane. |

## Configuring it

Four things must line up. Each is small; the failure modes come from one of
them being missing.

**1. Policy** must declare the resolver as a service, declare the admin API as
a service, and allow both hops. The resolver appears as the *subject* of an
`Allow` — this is a supported ZPL shape:

```
Define zpr-dns as a service.        # UDP/53, at the resolver Zode's node address
Define vs-admin as a service.       # TCP/8182, provided by the VS's own actor (vs.zpr)
Allow access:all users to access zpr-dns.
Allow zpr-dns to access vs-admin.
```

In the `.zplc`, the resolver is a Zode, and its static address is its
`[nodes]` entry's `zpr_address`, pinned to its CN (an authored
`["zpr.addr", ...]` provider pin is a compile error since zipline#109), so
clients can be pointed at a fixed address. `vs-admin` is provided by the visa
service's own actor, CN `vs.zpr`, which is what puts `fd5a:5052::1` behind the
name: the visa service runs on the relay Zode's TUN, with that address as the
relay's second ZPR address. This is the `dns-demo` layout (its relay node, links
and the other Zodes are left out):

```toml
[nodes."dns.demo"]
provider = [["device.zpr.adapter.cn", "dns.demo"]]
zpr_address = "fd5a:5052:8888::53"

[protocols.dns]
l4protocol = "UDP"
port = 53

[protocols.vs-admin]
l4protocol = "TCP"
port = 8182

[trusted_services.machines]
api = "file"
returns_attributes = ["hostnames -> device.hostname{}"]
expiration_seconds = 3600

[services.zpr-dns]
protocol = "dns"
port = 53
provider = [["device.zpr.adapter.cn", "dns.demo"]]   # at its node's zpr_address

[services.vs-admin]
protocol = "vs-admin"
port = 8182
provider = [["device.zpr.adapter.cn", "vs.zpr"]]
```

The resolver's CN also needs a `[bootstrap]` key entry. Keep service names
that should resolve as lowercase DNS labels: the visa service's service
lookup is exact-match.

**2. The naming authority**, if machine names are wanted, is a trusted service
returning `hostnames -> device.hostname{}`. In the configuration above this is
the `[trusted_services.machines]` block declared in step 1, over
`machines.json`.

Note on pruning: the compiler drops a trusted service that no ZPL rule
references — but `device.hostname` is visa-service-interpreted, so a store
vending it is retained anyway
([zipline#105](https://github.com/org-zpr/zl-zipline/issues/105)): the
weaver keeps any service whose `returns_attributes` maps to `device.hostname`
or `device.zpr_addr`, emitting an `info` diagnostic. No ZPL reference is
needed to keep the hosts index filling.

**3. An API key** with the least-privilege `resolve` permission, minted on the
visa service host and readable only by the resolver:

```sh
vsapikey create resolve dns /path/to/vs_keys.toml > /run/secrets/vs-resolve-key
```

**4. A Corefile.** The zone comes from the server block, not from the plugin;
`api_key_file` and `tls_ca` are required and there is deliberately no
insecure-skip-verify option:

```
demo.:53 {
    zpr {
        endpoint       https://[fd5a:5052::1]:8182
        api_key_file   /conf/vs-resolve.key
        tls_ca         /conf/include/admin-tls-cert.pem
        tls_servername vs.zpr      # the admin cert is issued for a name, not the IP
        ttl            30
        negative_ttl   10
    }
    cache 30
    errors
    log
}
```

Full option list, defaults and the CoreDNS version pin: `zl-zpr-coredns/README.md`.

**And then the client.** Nothing in ZPR points an endpoint's stub resolver at
the resolver's overlay address — that is endpoint work, still unbuilt. Until
it exists, "seamless" means "seamless once the OS is told where the resolver
is"; `dns-demo` writes `/etc/resolv.conf` in the client's entrypoint.

## Query semantics

| Query | Result |
|---|---|
| `<name>.<zone>` AAAA, service found | AAAA `zpr_addr`, TTL = `ttl` |
| `<name>.<zone>` AAAA, service 404, host found | AAAA `zpr_addr` |
| service 404 and host 404 | NXDOMAIN + SOA, MINIMUM = `negative_ttl` |
| name exists, other qtype | NODATA (NOERROR, SOA in authority) |
| two or more labels under the zone | NXDOMAIN, no HTTP call |
| admin API 5xx / 401 / 403 / unreachable / bad JSON | **SERVFAIL**, never NXDOMAIN |
| a *failed* (non-404) service lookup | SERVFAIL — never falls through to hosts |

That last row is the security-relevant one: a hostname can never shadow or
substitute for a service, not even while the service lookup is erroring.

Liveness is TTL-bounded, not event-driven. A provider that goes away becomes
NXDOMAIN within roughly `ttl + negative_ttl`; a service that moves resolves to
its new address on the same scale. Keep TTLs short — ZPR addresses are
recycled across reconnects.

"Goes away" means the visa service removed the provider's actor, and with it
the services the actor provides. The plugin learns this only by asking again
on a cache miss; the visa service pushes nothing to CoreDNS. For a Zode
([zl-zipline#284](https://github.com/org-zpr/zl-zipline/issues/284)):

- **A graceful stop** (SIGINT, SIGTERM, a Windows console event) deregisters
  the Zode before its links go down, so the name stops resolving within one
  cache period.
- **A crash or a lost Zode** is removed once every link of it has been down for
  `ZODE_DEPARTURE_GRACE` (10 s), so detection (9-12 s), the grace and one cache
  period add up. That is about 52 s with the demo's `ttl 30`.
- `ph` treats SIGHUP as a crash. zl-zpr-demo's `demo-stop-ph` kills ph's tmux
  session, so `test-dns.sh` section 3 exercises the crash path. It saw
  NXDOMAIN 25 s after the stop, against its 60 s deadline.
- **Not removed:** a Zode that left while the visa service was down. A restart
  schedules no departure checks (ZODE_LINKS.md, "Zode departure (#284)").

## Security properties worth preserving

- **The `resolve` key asks about one name and gets one address.** It reaches
  exactly three endpoints and cannot read actors, visas or policy. Do not
  widen it; add a `read`-gated endpoint instead.
- **The admin API endpoint must be `https`** — rejected at plugin startup
  otherwise, because plain HTTP would send the `X-API-Key` in the clear.
- **The admin certificate is always verified** against `tls_ca`. Certificates
  need a `subjectAltName`: Go's verifier ignores CN, so a CN-only cert (as in
  `multinode-demo`) cannot be pinned.
- **A hostname is never self-asserted.** It must arrive as an authenticated
  trusted-service claim. Unauthenticated adapter claims are committed to the
  actor whenever a join policy matches, so accepting `device.hostname` from
  that set would let any machine claim any name.
- **Never silently rename.** A rejected claim is surfaced (counted, logged,
  and visible on the loser's `ActorDescriptor.hostname_conflicts`), never
  repaired by the visa service.
- **An index entry's lifetime is the actor record's**, not the attribute's, so
  a name does not vanish mid-session when an attribute cache lapses.

---

## Design decisions

Rationale carried over from the completed master plans, which were retired
once shipped. Full plan text is in git history:
`git show a35b224:docs/plans/2026-09-15-dns-integration.md` (service names,
umbrella [zipline#34](https://github.com/org-zpr/zl-zipline/issues/34)) and
`git show a35b224:docs/plans/2026-09-17-machine-hostname-dns.md` (machine
names, umbrella [zipline#49](https://github.com/org-zpr/zl-zipline/issues/49)).

**No new admin surface for services, no schema change.** The existing
`GET /admin/services{,/name}` already carried everything a resolver needs, so
service names shipped as one new API-key permission (`resolve`) plus a Go
plugin; machine names added only `GET /admin/hosts/{name}` under that same
permission. Neither touched `policy.capnp`, the compiler or `zpr-common`.
[zipline#36](https://github.com/org-zpr/zl-zipline/issues/36),
[#54](https://github.com/org-zpr/zl-zipline/issues/54)

**The resolver is an ordinary policy subject, and the VS an ordinary
provider.** `Allow zpr-dns to access vs-admin` (a service as the subject)
and `vs-admin` provided by the VS's own actor were the design's one real
unknown; both compile and load, so no device-class stand-in for the resolver
was needed. [zipline#35](https://github.com/org-zpr/zl-zipline/issues/35)

**The answer is the provider's `zpr_addr`, never `dock_zpr_addr`.** The
latter is the node the provider docks to, not the provider.
[zipline#37](https://github.com/org-zpr/zl-zipline/issues/37)

**Infrastructure failure is SERVFAIL, never NXDOMAIN.** A client must not
cache "does not exist" because the resolver lost its visa or the VS
restarted. [zipline#37](https://github.com/org-zpr/zl-zipline/issues/37)

**Hostnames come from a trusted service, not from policy.** Having the
evaluator grant a hostname the way it grants `zpr.services` would be
authenticated by construction, but it is a three-repository change and makes
every machine addition a policy recompile — wrong for a network where
machines come and go. The `host:<NAME>` index is source-agnostic, so policy
could become a second source later without touching the plugin or the API.
[zipline#49](https://github.com/org-zpr/zl-zipline/issues/49)

**The attribute is `device.hostname`, not `device.zpr.*`.** ZPR owns the
`zpr.` sub-namespace in every class domain and the compiler rejects a
declared trusted service returning one, since it could forge an identity key
or authority marker. A reserved spelling could only be filled from policy.
[zipline#50](https://github.com/org-zpr/zl-zipline/issues/50)

**Not the X.509 CN.** CNs are neither unique nor indexed by design — two
connected actors may share one — and are unconstrained strings that need not
be valid DNS labels. A purpose-built, validated attribute keeps cryptographic
identity and network naming separate.
[zipline#49](https://github.com/org-zpr/zl-zipline/issues/49)

**First claim wins; never mangle.** Renaming a duplicate `somename` to
`somename-1` was rejected: nobody learns the mangled name, which machine
keeps the plain name would depend on connect order (and "connect first, hold
the name" is an attack in a system where the name picks who gets your
traffic), and probing for a free suffix can steal a name another machine is
about to claim. Instead `device.hostname` is multi-valued and claimed per
value, so a naming authority can return a friendly alias plus a value unique
by construction; a collision costs only the alias. A released name is not
handed to the loser — it re-claims on its next attribute refresh.
[zipline#53](https://github.com/org-zpr/zl-zipline/issues/53)

**One naming authority per ZPRnet** (operator decision, 2026-09-17).
Collisions are therefore races or control-plane bugs, not a steady state,
which is why loud first-claim-wins is enough and name scoping is not needed.
[zipline#49](https://github.com/org-zpr/zl-zipline/issues/49)

**One flat namespace, checked at claim time.** A `somename.host.<zone>`
subzone would be unambiguous by construction but makes the common case
uglier to defend against a collision one naming authority can simply avoid.
Ambiguity is rejected where names are assigned, and the plugin's
service-first order keeps the resolver correct even if that check is
bypassed. [zipline#52](https://github.com/org-zpr/zl-zipline/issues/52),
[#53](https://github.com/org-zpr/zl-zipline/issues/53)

**Claims are atomic via `hset_nx` returning whether it set.** That removes
the read-then-write race a hand-rolled claim would have. Releases go through
owner-checked helpers shared by service and host entries, because an
unchecked delete in `try_update_actor` had let one provider's refresh erase
another provider's live `service:<name>` entry.
[zipline#51](https://github.com/org-zpr/zl-zipline/issues/51),
[#53](https://github.com/org-zpr/zl-zipline/issues/53)

---

## Implementation status

Service names and machine names are fully implemented and merged. The end-to-end proof is
`zl-zpr-demo/dns-demo`, on zl-zpr-demo's `feature/zodes`
([zl-zpr-demo#10](https://github.com/org-zpr/zl-zpr-demo/pull/10),
[zl-zipline#235](https://github.com/org-zpr/zl-zipline/issues/235)). Its
`local-compute/deploy-docker.sh` stands up a relay Zode that hosts the visa
service on its own TUN, plus web, client and resolver endpoint Zodes linked to
it, and `local-compute/test-dns.sh` exercises resolution, liveness, both policy
hops, machine names and aliases, and the collision / precedence / invalid-name
controls. It is `zpr-dev build`'s `docker` tier: all eight `test-dns.sh`
sections passed in build set `build-sets/2026-11-19.yaml`. Reproduce it through
that set, with a `zpr-dev` built from zl-zpr-dev-context's `feature/zodes`:
`zpr-dev build --manifest <zl-zpr-dev-context>/build-sets/2026-11-19.yaml
--test docker` (add `--force` to replace an earlier build of the set). The
demo's own `make` builds whatever the source checkouts hold, and the
workspace's default is `zipline`, whose core has no `ph zode`; zl-zpr-demo's
`zipline` branch still docks adapters on one node, so it runs only against a
core from before zl-zipline#234.

On ZIPLINE the resolver runs behind a Zode named `dns.zpr` ([org-zpr/zipline#252](https://github.com/org-zpr/zipline/issues/252)):
a Zode with an initial link to the relay, a server-generated link
certificate and RSA key, and its address `fd5a:5052:90de::2` pinned by its
`[nodes]` entry in policy. It links to the relay only, and endpoint queries
reach it through the relay. The `addresses` file trusted service that used to
grant that address is gone.

Not built, in rough order of likely demand:

- **Client stub-resolver configuration** by the endpoint daemon (above).
- **PTR / reverse lookups** for `fd5a:5052::/32` — would widen `resolve` to an
  address-keyed lookup.
- **`GET /admin/hosts` (list)**, `read`-gated, for operators.
- **SRV records** from `service_endpoints` — cheap, no consumer yet.
- **Push invalidation** on actor departure, and any bulk/zone-transfer mode.
  Short TTLs are the deliberate ceiling; revisit if 30 s staleness bites.
  Departure itself is built: a departed Zode's actor is removed (#284; see
  "Query semantics").
- **Multi-node placement** of the resolver — blocked on the data plane.
- **Pushing rejected claims to the naming authority.** Today conflicts are
  only visible to a poller via `hostname_conflicts`.
- **Policy as a second hostname source** (see Design decisions).

Deliberately rejected, not deferred: naming actors by X.509 CN or node id
(CNs are not unique and not indexed), multi-answer RRsets for a shared name
(`ping6 somename` must not be a coin flip), and scoped or tenant-qualified
names while one naming authority owns the namespace.

## Where the code lives

| Concern | Location |
|---|---|
| Corefile parsing, TLS/key setup, `https`-only check | `zl-zpr-coredns/plugin/zpr/setup.go` |
| Query handling, zone/label rules, SOA synthesis | `zl-zpr-coredns/plugin/zpr/zpr.go` |
| Admin API client, service-then-host lookup, status mapping | `zl-zpr-coredns/plugin/zpr/client.go` |
| CoreDNS version pin and plugin ordering | `zl-zpr-coredns/Makefile`, `plugin.cfg` |
| Resolve endpoints and their gate | `zl-zpr-visaservice/vs/src/admin_service.rs` (`can_resolve`) |
| `resolve` permission level | `vs/src/admin_apikeys.rs`, `vs/src/bin/vsapikey.rs` |
| Hostname claim, validation, first-claim-wins, reconciliation | `vs/src/actor_mgr.rs`, `vs/src/event_mgr.rs`, `vs/src/db/actor.rs` |
| Endpoint reference and types | `zl-zpr-visaservice/admin-http-api.txt`, `admin-api-types/src/admin_api_types.rs` |
| Trusted-service `returns_attributes` parsing | `zl-zpr-compiler/src/config/trusted_service.rs` |
| Worked example: policy, keys, container, tests | `zl-zpr-demo/dns-demo/` |

**Unrelated use of the word.** `PolicyResolver` in
`zl-zpr-visaservice/vs/src/policy_mgr.rs` does ordinary *substrate* DNS —
resolving node hostnames in a topology to underlay IPs at policy load, on the
public internet's DNS, before any ZPRnet exists. It has nothing to do with
this document. See [ROUTING.md](ROUTING.md).
