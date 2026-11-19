# ZPR Repository Inventory

The repositories that make up the standard ZPR development workspace, what
each one holds, and how they depend on one another.

The authoritative list is [`workspace.yaml`](../workspace.yaml) at the root of
this repository — `zpr-dev setup` clones exactly what it names. This document
describes that set; when the two disagree, `workspace.yaml` is correct and this
file needs updating.

---

## Workspace layout

`zpr-dev setup` clones every repository side by side under the workspace root,
each into a directory matching its repository name:

```text
~/src/zl_zpr/
├── zl-zpr-dev-context/     this repository — shared context, cloned separately
├── zl-zpr-core/
├── zl-zpr-common/
├── zl-zpr-visaservice/
├── zl-zpr-vsapi/
├── zl-zpr-compiler/
├── zl-zpr-policy/
├── zl-zpr-coredns/
├── zl-zpr-rfcs/
├── zl-zpr-demo/
├── zl-zpr-utils/
└── zl-zpr-dev-tools/
```

All are public repositories in the [`org-zpr`](https://github.com/org-zpr)
organization at `git@github.com:org-zpr/<name>.git`. They were created under the
personal `mkolehmainen` account and transferred on 2026-10-29
(`org-zpr/zl-zipline#207`); every old `mkolehmainen/...` URL redirects.

All but one are forks of the corresponding upstream repository in the
[`org-zpr`](https://github.com/org-zpr) organization — `zl-zpr-core` forks
`org-zpr/zpr-core`, `zl-zpr-common` forks `org-zpr/zpr-common`, and so on — and
all of those check out `zipline` (see "Branch model" below). The exception is
`zl-zpr-coredns`, which has no upstream: it is a new repository and its working
branch is `main`.

> **Cross-repository references point at the forks in `org-zpr`**
> (`org-zpr/zl-zipline#17`, `#18`, `#207`, `#209`): Cargo Git dependencies,
> `zl-zpr-common`'s submodules, and `zl-zpr-core`'s `adapter.yml`. Most other CI
> workflows deliberately call the upstream reusable workflows in
> `org-zpr/zpr-dev-tools`. See "Cross-repository
> dependencies" in `BUILD.md` for the detail.

Use `zpr-dev status` to see the state of every checkout at once.

---

## Branch model

Every fork carries two long-lived branches (`zl-zpr-coredns` is not a fork and
has only `main`):

| Branch | Role |
|---|---|
| `zipline` | **The working branch, and the repository default.** Feature branches start here and pull requests target it. |
| `main` | **A read-only mirror of upstream `main`.** Never commit to it. |

Keeping `main` pristine is what makes the fork maintainable: syncing upstream is
always a fast-forward that can never conflict, `git diff main...zipline` is
exactly the zipline delta, and contributing a change back upstream means
branching off `main` and cherry-picking rather than untangling merges.

### Taking upstream changes into zipline

Two steps. A checkout made by `zpr-dev` has only one remote — `origin`, the fork
— and no `upstream` remote, so step 1 is done on GitHub rather than locally.

**1. Fast-forward the fork's `main` from upstream.** No local checkout needed:

```bash
gh repo sync org-zpr/zl-zpr-<name> --source org-zpr/zpr-<name> --branch main
```

Pass `--branch main` explicitly. `gh repo sync` defaults to a default branch,
and ours is now `zipline` — being explicit removes the ambiguity. (Aiming it at
`zipline` by mistake is not destructive without `--force`: upstream `main` will
not fast-forward onto a `zipline` that has commits, so it errors out.)

**2. Merge it into `zipline` locally, then push:**

```bash
git fetch origin
git checkout zipline
git merge origin/main
git push origin zipline
```

Merging `origin/main` directly means local `main` never has to be checked out or
updated. If you want it current anyway — for a local `git diff main...zipline` —
add `git checkout main && git merge --ff-only origin/main` before the merge.

**Merge, never rebase.** `zipline` is long-lived and published, so rebasing it
rewrites history other checkouts already have. Merge commits on `zipline` are
expected and fine.

Do **not** point `main` at the upstream remote with
`git branch main --set-upstream-to=upstream/main` — a later `git push` on `main`
would then try to write to upstream `org-zpr/zpr-<name>`. Since the transfer the
forks and upstream share an organization, so an org member's push there is no
longer refused for lack of access; only upstream's branch protection stops it.
**Never push to an `org-zpr/zpr-*` repository.**

Note that `zpr-dev update` is *not* part of this. It fetches and fast-forwards
the current branch from `origin` only, so it picks up `zipline` commits already
pushed to the fork; it never contacts upstream.

### Tag names need a `zl-` prefix

The forks inherited upstream's tags (`zl-zpr-common` arrived carrying `v0.7.1`
through `v0.9.0`), and upstream keeps adding more. Because the Cargo
dependencies pin *tags*, an unprefixed zipline tag can collide with an upstream
tag of the same name and make `git fetch --tags` ambiguous. Tag zipline releases
as `zl-v0.26.0`, not `v0.26.0`.

---

## At a glance

| Repository | Language | What it is |
|---|---|---|
| [`zl-zpr-core`](#zl-zpr-core) | Rust | Core ZPR components — `ph` (the Zode) and `ph-cli` |
| [`zl-zpr-common`](#zl-zpr-common) | Rust | Shared `zpr` crate: protocol types, constants, IDL |
| [`zl-zpr-visaservice`](#zl-zpr-visaservice) | Rust | The Visa Service and its policy evaluator |
| [`zl-zpr-vsapi`](#zl-zpr-vsapi) | Cap'n Proto | IDL for the Visa Service API |
| [`zl-zpr-compiler`](#zl-zpr-compiler) | Rust | `zplc`, the ZPL policy compiler |
| [`zl-zpr-policy`](#zl-zpr-policy) | Cap'n Proto | IDL for the binary policy descriptor |
| [`zl-zpr-coredns`](#zl-zpr-coredns) | Go | CoreDNS with the `zpr` resolver plugin |
| [`zl-zpr-rfcs`](#zl-zpr-rfcs) | LaTeX / Docker | Public ZPR RFCs — the architectural reference |
| [`zl-zpr-demo`](#zl-zpr-demo) | HCL | Runnable ZPRnet demonstrations |
| [`zl-zpr-utils`](#zl-zpr-utils) | Rust | Non-ZPR-specific utility crates |
| [`zl-zpr-dev-tools`](#zl-zpr-dev-tools) | Docker | Development and build tooling |

---

## How they fit together

```text
zl-zpr-policy ─┐                     (Cap'n Proto schemas, vendored as
zl-zpr-vsapi  ─┴─► zl-zpr-common      submodules inside zl-zpr-common)
                       │
                       ├──► zl-zpr-core          the node
                       └──► zl-zpr-visaservice   the visa service
                                    ▲
                 zl-zpr-compiler ───┘   emits the signed binary policy
                                        the visa service evaluates
```

- **`zl-zpr-common` is the hub.** It packages the shared types — addresses,
  distinguished names, packet metadata, the NODE–VS API structures, and the
  binary policy format — and vendors `zl-zpr-policy` and `zl-zpr-vsapi` as Git
  submodules so the Cap'n Proto schemas travel with it.
- **`zl-zpr-core` and `zl-zpr-visaservice` both depend on `zl-zpr-common`**, pulled via
  Git in `Cargo.toml`; no manual setup is required.
- **`zl-zpr-compiler` closes the policy loop.** It compiles ZPL source into a
  signed binary policy that the visa service loads; the signing key must match
  the one the visa service is configured with.
- **`zl-zpr-coredns` depends on nothing in this workspace at build time.** It
  talks to the visa service over its admin HTTP API at run time, so the API
  contract — not a crate — is the coupling.
- **`zl-zpr-utils` and `zl-zpr-dev-tools` are leaves** — nothing in the protocol path
  depends on them.
- **`zl-zpr-rfcs` is the specification**, not code. Read it before changing
  protocol behavior.

---

## The repositories

### `zl-zpr-core`

Core ZPR components: `ph`, which runs a Zode (`ph zode`, its only mode
since zl-zipline#234), its admin CLI `ph-cli`, and the integration tests that
exercise a running ZPRnet.

```text
adapter/            ph, ph-cli and the admin API schema (the directory
                    name is historical: there is no separate adapter)
libnode2/           the node library
integration-test/   end-to-end tests and the platform test guides
docs/               SETUP.md, how to set up and run Zodes
diagrams/           architecture diagrams
```

A Cargo workspace driven by `make`. The build pulls tools and libraries from
several of the repositories below, so it expects the standard workspace layout.

> Pre-release: breaking changes may land without notice, and the end-to-end
> security features are not all implemented yet.

### `zl-zpr-common`

The shared `zpr` crate. Anything used by more than one ZPR service belongs
here rather than being duplicated.

- Shared Rust types: addresses, DNs, packet metadata, and the helpers for
  writing and serializing them.
- Feature-gated wrappers over the policy and VSAPI types.
- IDL sources for the ZPR sub-protocols, included as **Git submodules** — the
  directories are still named `zpr-policy/` and `zpr-vsapi/`, but `.gitmodules`
  now points at `org-zpr/zl-zpr-policy` and `org-zpr/zl-zpr-vsapi`.
  Clone recursively, or run `git submodule update --init`, or the build will not
  find the schemas. An existing checkout needs `git submodule sync --recursive`
  to pick up the new URLs; the pinned commits did not move.

### `zl-zpr-visaservice`

The ZPR Visa Service: policy evaluation and visa issuance.

```text
vs                  the visa service itself (aka "v2vs")
libeval             the evaluator — compares described traffic to policy
                    to decide whether a visa is issued
zpt                 ZPR Policy Tester, a CLI for exercising libeval
vs-admin            CLI administration client for the vs HTTPS admin API
zpr-dashboard       CLI dashboard for monitoring visa service activity
admin-api-types     data structures shared by vs and vs-admin
integration-test    shell-based integration tests, including libeval
                    evaluation tests driven through zpt
tools               helper scripts, including zpr-pki for PKI operations
admin-http-api.txt  reference for the vs HTTPS admin API
config-example.yaml annotated example vs configuration
```

Depends on `zl-zpr-common` for the NODE–VS API structures and the binary policy
format. Requires Rust edition 2024, `make`, OpenSSL, and a running
Redis/Valkey at runtime. Build and test with `make` / `make test`.

`libeval` is where an issuance decision is actually made — changes there change
the security posture of the whole system.

### `zl-zpr-vsapi`

`vs.capnp`: the Cap'n Proto IDL describing the Visa Service API.

Consumed as a submodule of `zl-zpr-common` rather than depended on directly.
Schemas are pre-release and field names, types, and structure are all still
subject to revision.

### `zl-zpr-compiler`

The ZPL compiler. `zplc` translates ZPL source into the binary policy the visa
service evaluates, and `zpdump` inspects a compiled policy.

```bash
./zplc -k path/to/rsa-key.pem path/to/policy.zpl
```

The RSA key signs the binary policy and must match the key the visa service is
configured with. Configuration defaults to a `.zplc` file beside the `.zpl`
source; `-c` overrides it. Built with `make`.

`zpl.bnf` is the grammar, and `test-data/` holds the compiler's ZPL fixtures.

### `zl-zpr-policy`

`policy.capnp`: the Cap'n Proto IDL for the ZPR policy descriptor — the wire
format `zl-zpr-compiler` emits and `libeval` consumes.

Like `zl-zpr-vsapi`, it is consumed as a submodule of `zl-zpr-common`, and its
schemas are pre-release.

### `zl-zpr-coredns`

CoreDNS built with the `zpr` plugin: it answers AAAA queries for ZPL service
names by asking the visa service's admin API for the service's ZPR address, so
a ZPR endpoint can reach a service by name.

The only repository here that is not a fork, and the only one written in Go.
`make build` clones CoreDNS at a pinned tag and compiles the plugin into
`bin/coredns`; the demo bakes that binary into its own image. See
[DNS.md](DNS.md) for how it fits together.

### `zl-zpr-rfcs`

The publicly available RFCs for Zero-trust Packet Routing. This is the
architectural source of truth; the code implements what the RFCs describe.

Start here:

| RFC | Topic |
|---|---|
| 12 | ZPR overview — the problem and the approach |
| 4 | Terminology — the glossary for everything else |
| 15 | ZPL policy language overview |
| 16 | ZPR's concept of identity |

PDFs live under `pdf/`; they build from source via the Docker image in
`tools/`. Not every internal RFC is published.

### `zl-zpr-demo`

Runnable demonstrations of ZPR. Each has its own `README.md`.

```text
containerized-demo/   a running ZPRnet in containers
multinode-demo/       multiple nodes
iot-demo/             ZPR integrated with the Oracle IoT platform
```

The fastest way to see a working ZPRnet without assembling one by hand.

### `zl-zpr-utils`

Utility crates that are not ZPR-specific and could stand alone.

```text
cbpf-rs/     classic BPF handling
cslab/       an RCU-friendly concurrent slab allocator
rcu/         read-copy-update primitives
zpr-ext/     extension helpers
zpr-utils/   the utility crate itself
```

`cslab` and `rcu` are lock-free data structures verified under
[`loom`](https://docs.rs/loom); treat changes to them as concurrency-critical.

### `zl-zpr-dev-tools`

Development and build tooling for the project, currently the `docker/`
images used to produce reproducible build environments.

---

## Repositories outside the workspace

| Repository | Why it is not cloned |
|---|---|
| `zl-zpr-dev-context` | This repository. It is the context checkout, cloned by `zpr-dev setup` before the manifest is read, so it is not listed as a manifest entry. |
| `org-zpr/zpr-bas` | ZPR Basic Authentication Service. Deprecated. Not forked, and excluded from the default workspace by request; clone it from upstream by hand if you need it. |

The upstream organization also holds several private repositories that are not
part of the standard workspace, are not forked, and are not listed here.

---

## Adding a repository to the workspace

1. Add a `name` / `url` entry to [`workspace.yaml`](../workspace.yaml).
   `default_branch` defaults to `main`, so omit it unless it differs.
2. Add the repository to the table and a section to this document.
3. Run `zpr-dev setup` — existing checkouts are left untouched and only the
   new repository is cloned.

Repository-specific instructions belong in that repository's `AGENTS.repo.md`,
not here. Cross-repository architecture belongs in this `docs/` directory.
