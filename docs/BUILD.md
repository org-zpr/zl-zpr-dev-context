# Building and Testing ZPR

How to build, test, and check every repository in the workspace, and what the
cross-repository build dependencies actually are.

Each repository's own `README.md` and `Makefile` are authoritative for that
repository; when they disagree with this document, they are correct and this
file needs updating. See [`REPOSITORIES.md`](REPOSITORIES.md) for what each
repository contains.

---

## Prerequisites

| Tool | Why | Needed by |
|---|---|---|
| Rust, stable toolchain ([rustup](https://rustup.rs/)) | Edition 2024 | every Rust repo |
| `make` | drives the builds | every repo |
| `build-essential` / Xcode CLI tools | C toolchain for native crates | every Rust repo |
| `pkg-config`, `libssl-dev` | the `openssl` crate | `zl-zpr-visaservice`, `zl-zpr-compiler` |
| `capnproto` (the `capnp` binary) | compiles the Cap'n Proto schemas | `zl-zpr-common`, `zl-zpr-core`, `zl-zpr-compiler` |
| `libpcap-dev` | packet capture in `ph-cli` | `zl-zpr-core/adapter/cli` |
| Go | the `zpr-dashboard` CLI | `zl-zpr-visaservice` |
| Valkey or Redis | required by `vs` **at runtime** | running a ZPRnet, `zl-zpr-core` integration tests |
| `openssl` CLI | generating keys and certificates | running a ZPRnet |
| `plantuml` | architecture diagrams | `zl-zpr-core` (`make diagrams`) |
| Docker | reproducible build env, RFC PDFs, demos | optional |

Debian/Ubuntu:

```bash
sudo apt install build-essential make pkg-config libssl-dev capnproto \
                 libpcap-dev golang valkey
```

macOS:

```bash
brew install make pkg-config openssl capnp go valkey
```

The canonical prerequisite list is the dev-env image in
`zl-zpr-dev-tools/docker/dev-env/Dockerfile` — if a build needs something new, it
is added there.

Windows (building `ph`/`ph-cli` natively, `x86_64-pc-windows-msvc`): Visual
Studio Build Tools with the "Desktop development with C++" workload (MSVC +
Windows SDK + CMake), Rust via rustup, NASM (`choco install -y nasm`, for
`aws-lc-rs`), and Cap'n Proto (`choco install -y capnproto`). `ph-cli` builds
with `--no-default-features` there (no libpcap). Details and the runtime
`wintun.dll` requirement: `zl-zpr-core/docs/SETUP.md`, "Windows". To
cross-check from Linux without a Windows box:
`cargo xwin check -p ph --target x86_64-pc-windows-msvc` and
`cargo xwin check -p ph-cli --no-default-features --target x86_64-pc-windows-msvc`
(two invocations: `ph-cli` must drop its default `pcap` feature, but `ph`
needs its defaults — a workspace-wide `--no-default-features` strips `ph`'s
mandatory `rcu-*` backend selection and fails to compile).

---

## Git access

Rust dependencies between ZPR repositories are declared with **HTTPS Git URLs**
in `Cargo.toml`. If you authenticate to GitHub over SSH, tell Git to rewrite
them:

```bash
git config --global url.git@github.com:.insteadOf https://github.com/
```

If Cargo's built-in Git client fails to authenticate, have it shell out to
`git` instead — this is what CI does:

```bash
export CARGO_NET_GIT_FETCH_WITH_CLI=true
```

Go work additionally needs:

```bash
go env -w GOPRIVATE="github.com/org-zpr/*,github.com/mkolehmainen/*"
```

---

## Common conventions

Every Rust repository exposes the same `make` targets, so the same four
commands work everywhere:

```bash
make          # build (default goal)
make test     # unit tests
make check    # cargo fmt --check, then build with warnings denied
make clean    # cargo clean
```

`make check` is what CI enforces, and it is stricter than a plain build:
formatting must be clean and **warnings are errors**. Run it before pushing.
The CI equivalent is:

```bash
cargo fmt --check
cargo build --all-targets --config 'build.rustflags = ["-D", "warnings"]'
```

**`zl-zpr-core` has no root `make check`** — only its `libnode2` member does.
Run the CI equivalent above from the workspace root there instead.

CI builds every Rust repository through the shared reusable workflows in
`zl-zpr-dev-tools/.github/workflows/` (`rust-build-test.yml`, `rust-test.yml`), so
a green local `make test && make check` is a good predictor of a green CI run.

**Actions is enabled on the forks again since the move to `org-zpr`
(2026-10-29), but CI is a gate only in `zl-zpr-core`,** where it has been seen
fully green (`zl-zpr-core#88`). CI there runs every script in the netns tier,
one job per `integration-test/*-test.sh` (`org-zpr/zl-zipline#210`). In every other fork the local gate stays the
gate until its CI has been seen green, and CI's result is quoted beside it (see
`skills/zpr/SKILL.md`, "Definition of done"). The `zl-zpr-core` integration
jobs download the visa service release named by `VISA_SERVICE_RELEASE`, and
the containerized jobs pull the private `ghcr.io/org-zpr/dev-env` image; see
`skills/zpr/SKILL.md` ("Git / PR conventions") for what each needs
(`org-zpr/zl-zipline#208`).

**On `feature/zodes`, core CI takes the visa service from a feature
pre-release**, `v0.22.0-zodes.N`, cut from the visa service's `feature/zodes`
(`make release`, then `gh release create v0.22.0-zodes.N --prerelease --target
<full sha> release-linux-x86_64.tar.gz`: core's integration jobs download that
asset, so a release without it fails every job at **Download Asset**). The first, `v0.22.0-zodes.1`, is `ef03e95`; core moved to it in
org-zpr/zl-zipline#250, because `v0.21.2` sends no `Link.peerCn`. **Re-cut
rule** (org-zpr/zl-zipline#254): when the visa service changes in a way core CI
needs, cut the next `-zodes.N` and move `VISA_SERVICE_RELEASE` in
`zl-zpr-core/.github/workflows/adapter.yml` to it. `v0.22.0-zodes.2` is
`c94d2fc`, after #250. For #252, core's join and rejoin scenario needs
`requestBootstrapVisas` before the visa service's PR merges, so the next ones
were cut from the head of org-zpr/zl-zpr-visaservice#67 **before it merged**
(gate g89 q9): `v0.22.0-zodes.3` (`7c373b8`), `v0.22.0-zodes.4` (`06fe9ba`,
adding the g100 fix: no session keys on a node's core visas) and
`v0.22.0-zodes.5` (`0fc740e`, adding the g101 fix: a policy pass never
waits on a node's VSS). Core's #252 PR points `VISA_SERVICE_RELEASE` at the
latest of them. Once #252's merge set C had merged, `v0.22.0-zodes.6` was
cut from the merged commit (`e1dd1c7`) and core moved to it
(zl-zpr-core#114). The re-cut rule stands: the next visa-service change that
core CI needs gets the next `-zodes.N`.

From 2026-09 until the transfer, Actions was switched off, because none of that
CI could run. `pr-notify.yml` needs `SLACK_ALERT_WEBHOOK_URL`, and
every caller of the shared `rust-build-test.yml` / `rust-test.yml` /
`go-build-test.yml` reusable workflows needs `ZPR_CICD_RO_TOKEN`, which the
reusable workflow declares **required** — and a fork inherits no secrets, so
those jobs failed within seconds, before any step ran, leaving every PR red for
a reason no PR could fix.

The switch is repository-level, so the workflow files are untouched and nothing
in `.github/` diverges from upstream. `skills/zpr/scripts/fork-ci.sh` reports the
state and reverses it (`--enable`). Inside the organization both secrets reach
the forks, so re-enabling no longer needs them supplied by hand.

### Compiler caching with sccache (optional, per-host)

The pipeline creates many worktrees and build directories whose dependency
graphs are identical at the same tip, so the same `rustc` invocations run over
and over. [`sccache`](https://github.com/mozilla/sccache) caches compiler
invocations content-hashed: an identical invocation is served from the cache
instead of re-executed. This changes **nothing** about what is built or tested
— only whether an identical `rustc` run is repeated — so it is safe to combine
with the no-reuse rule for build directories (zipline#60): the build directory
is still fresh, every crate is still "compiled", but unchanged dependency
compiles come back from the cache. Measured on this workspace's host: an
identical dependency compile dropped from 4.33 s to 0.59 s.

The setup is **host-level, not committed to any repository** — each host opts
in for itself. On a host with `sccache` installed (`cargo install sccache`, or
a release binary on `PATH`), a cargo config at the workspace root covers every
checkout and worktree beneath it:

```toml
# ~/zl_src/.cargo/config.toml  (workspace root; applies to all checkouts under it)
[build]
rustc-wrapper = "/path/to/sccache"
```

The cache lives in `~/.cache/sccache` (10 GiB default cap);
`sccache --show-stats` reports hit rates and cache size, and
`sccache --zero-stats` resets the counters before a measurement. To disable,
remove the `rustc-wrapper` line (or set `RUSTC_WRAPPER=""` for one command) —
builds behave exactly as before, just slower.

Caveats:

- The config file lives *above* the repositories, so `zpr-dev validate` and
  `git status` never see it; a host without it simply builds uncached. Do not
  commit a `rustc-wrapper` path into a repository's own `.cargo/config.toml` —
  the path is host-specific and would break every other machine.
- `cargo fmt` and `cargo clippy` are unaffected (sccache wraps `rustc`, not
  the tools), so gate results do not change.
- Incremental compiles are not cacheable; sccache passes them through
  unchanged (they run exactly as without it). The wins come from the
  non-incremental dependency compiles, which dominate a fresh build dir.

### Integration tier: how to run it

Two invocations run the netns integration scripts, and they are not the same
speed. **The recommended one is `zpr-dev build --test netns`**, with `--jobs N`
to widen it:

```bash
zpr-dev build --tip --test netns --jobs 8 --no-tarball
```

On a host where unprivileged user namespaces work (`unshare -nr true`
succeeds) and `valkey-server` is on `PATH`, it runs the scripts directly on
the host, **with no sudo**; otherwise it falls back automatically to
per-script privileged Docker containers and records the fallback in the
emitted manifest — see "Integration tests" below and
`zpr-dev/docs/specs/spec-003-build.md` §6. Either route runs `--jobs N`
scripts at a time (default 4): each script's namespaces are private to it,
so parallel runs cannot collide. That parallelism is what makes it the fast
route.

**`make docker-test` (in `zl-zpr-core`, also `make integration-test-docker` at
its root) is the fallback for hosts without unprivileged user namespaces and
without `zpr-dev`**, and it is much slower: it runs every script serially inside a
single privileged container. Reach for it when you need one script under
debugging (`TEST=<script>`), a root shell (`docker-shell`), or a run outside
any build; do not reach for it as the routine tier run.

Measured on this workspace's host (8 cores, 2026-10-21), the eleven
netns scripts only (no repo builds), 4-wide, against the same binaries
(zipline#191's acceptance runs):

| Route | Wall clock |
|---|---|
| Host, unprivileged user namespaces, no sudo (`./<script>`, `xargs -P4`) | **6:00** (360s) |
| Docker fallback (`make docker-test TEST=<script>`, `xargs -P4`, image cached) | **6:33** (393s) |

The host route is a little faster, with no image to build or containers to
start, and it is the one `zpr-dev` picks whenever user namespaces work. The
difference is small because most of each script's time is fixed `sleep`s:
the tier takes about as long as its slowest script,
`one-node-policy-reauth-test.sh`. Width matters far more than the route.

Earlier measurements on the same host (8 cores, Docker fallback route in
both cases, 2026-09-29, before the host route could run in parallel):

| Invocation | What ran | Wall clock |
|---|---|---|
| `zpr-dev build --tip --test netns --jobs 5 --no-tarball` | all repo builds + 10 scripts (11 since zipline#178 gated `node-restart-test.sh`), 5-wide | **10:44** total (repo builds alone measured 2:41 warm, so the tier itself was roughly 8 minutes) |
| `make docker-test` (`zl-zpr-core`, image cached) | 9 scripts, serial | **~17:08** for the script phase alone (22:53:32Z -> 23:10:40Z), no repo builds included |

Sources: the first row is the acceptance run quoted in zl-zpr-dev-context PR
#44 (`/usr/bin/time -v`, elapsed 10:44.63); the second is the docker gate run
for zipline#137 the same day, quoted 9/9 PASS on zl-zpr-visaservice PR #44
(timestamps from its run log). The script sets differ by one
(`a2a-pubkey-test.sh` ran only in the first), which does not change the
conclusion: the serial single-container route pays roughly the whole tier's
cost again in lost parallelism.

Renaming `make docker-test` or speeding the target itself is `zl-zpr-core`'s
Makefile, not this repository, and is staged as a follow-up — see the comment
thread on zipline#144.

---

## Per-repository builds

### `zl-zpr-common`

A Cargo workspace whose features gate the Cap'n Proto bindings, plus **two Git
submodules** carrying the IDL. Initialize them first or the build cannot find
the schemas:

```bash
make submodules-pull      # git submodule update --init --recursive
make build                # cargo build --all-targets -F all
make test                 # cargo test -F policy,vsapi,rcu-crossbeam-epoch
make bench                # cargo bench --features vsapi,rcu-aarc
```

Features: `policy`, `vsapi`, `all`. `build.rs` compiles the schemas from
`zl-zpr-policy/` and `zl-zpr-vsapi/`, so `capnproto` must be installed.

`make submodules-update` moves the submodules to the latest upstream commit —
that is a deliberate dependency bump, not routine setup.

Two invocation traps here, both of which look like a broken repository:

- A bare `cargo build --all-targets` gates off `serde` and fails in
  `packet_info.rs` with a misleading "unresolved import `serde`". Use
  `make build`, which passes `-F all`.
- `make check` (`cargo rustc --lib -- -D warnings`) does *not* pass `-F all`, so
  it fails the same way on clean `main`. That is a known pre-existing issue in
  this repository, not something your change caused.

### `zl-zpr-core`

A Cargo workspace with members `adapter/admin-api`, `adapter/ph`,
`adapter/cli`, and `libnode2`.

```bash
make          # cargo build && cargo build -p libnode2 --all-features
make test
make diagrams # PlantUML diagrams, needs the plantuml command
```

Binaries land in `./target/debug`. The packet handler `ph` is the one thing you
need to run a node or an adapter. `adapter/cli` (`ph-cli`) needs
`libpcap-dev`.

Each member also has its own `Makefile`, so a single component can be built in
isolation — CI does exactly this, one job per member.

### `zl-zpr-visaservice`

A Cargo workspace plus one Go component.

```bash
make            # build-rs (cargo build --all-targets) + build-go (zpr-dashboard)
make test       # cargo test, Go tests, then the shell integration tests
make check      # fmt and warning checks across every member
make release    # release tarball in build-release/, plus release-linux-<arch>.tar.gz
```

`make release` collects `vs`, `vs-admin`, `vsapikey`, `zpt`, `zpr-dashboard`,
and `zpr-attr-server` into `build-release/` and tars it up. That tarball is
what `zl-zpr-core`'s integration tests consume.

`vs` needs a running Valkey/Redis at runtime, but not to build or to run the
unit tests.

### `zl-zpr-compiler`

```bash
make        # cargo build --all-targets
make test   # cargo test --lib, --bins, then the full suite
```

Produces `zplc` (ZPL → binary policy) and `zpdump` (inspect a compiled
policy):

```bash
./zplc -k path/to/rsa-key.pem path/to/policy.zpl
```

The RSA key signs the policy and must match the key the visa service is
configured with. Configuration defaults to the `.zplc` file beside the `.zpl`
source; `-c` overrides it.

### `zl-zpr-utils`

Independent crates — `cbpf-rs`, `cslab`, `rcu`, `zpr-ext`, `zl-zpr-utils` — each
built and CI-checked on its own:

```bash
cd cslab && cargo build --all-targets && cargo test
```

`cslab` and `rcu` are lock-free and carry [`loom`](https://docs.rs/loom) models
behind `cfg(loom)`. No CI job runs them, so exercise concurrency changes
locally:

```bash
RUSTFLAGS="--cfg loom" cargo test --release
```

### `zl-zpr-vsapi`, `zl-zpr-policy`

Cap'n Proto schemas only — nothing to build. They are consumed as submodules of
`zl-zpr-common`, and `zl-zpr-common`'s `build.rs` compiles them.

### `zl-zpr-rfcs`

PDFs are built from Markdown with pandoc, via the Docker image in `tools/`:

```bash
cd tools && docker build . --tag rfcgen:latest && cd ..
docker run --rm -v "$PWD":/work -w /work rfcgen:latest \
    sh -lc "git config --global --add safe.directory /work && make"
```

Output goes to `pdf/`. Unix line endings are enforced, so on a machine that has
ever been configured otherwise: `git config --global core.autocrlf input`.
Feedback on an RFC happens in GitHub Discussions on that repository.

### `zl-zpr-demo`

Each demo has its own `README.md`; `containerized-demo` also has a
`README-DEV.md` covering how to cut a new release. The demo build compiles a
policy with `zplc` and packages binaries with configuration into a versioned
release, optionally inside Docker (`USE_DOCKER=1`).

Running a published demo needs no build at all — image, binaries, and config
are released together and must share the same `YYYYMMDD` version.

### `zl-zpr-dev-tools`

Holds the Docker images (`docker/dev-env`) and the reusable CI workflows. Not a
build target itself.

### `zl-zpr-dev-context`

This repository. The `zpr-dev` tool is an ordinary Cargo project:

```bash
cd zpr-dev && cargo build && cargo test
```

---

## Cross-repository dependencies

Shared Rust crates are consumed **from Git by tag**, not by local path:

```toml
zpr       = { git = "https://github.com/org-zpr/zl-zpr-common.git", tag = "v0.30.0", ... }
zpr-ext   = { git = "https://github.com/org-zpr/zl-zpr-utils.git",  tag = "zpr-ext-v0.5.3" }
cbpf-rs   = { git = "https://github.com/org-zpr/zl-zpr-utils.git",  tag = "cbpf-rs-v0.2.0" }
```

> **Every ZPR-family URL points at the forks in `org-zpr`.** The forks moved
> from `mkolehmainen` to `org-zpr` on 2026-10-29 (`org-zpr/zl-zipline#207`), and
> the Cargo URLs followed bottom-up (`org-zpr/zl-zipline#209`): `rcu-v0.1.3`,
> then `zl-zpr-common` `v0.30.2`, then the consumers. The order was forced
> because a released tag embeds its dependencies' URLs, and changing a
> consumer's URL alone gives Cargo two sources for a crate that a tagged
> dependency also pulls in. Any future owner or URL move has to go the same way.
> `zipline#17` repointed
> the `zpr` crate and `zl-zpr-common`'s submodules; `zipline#18` finished the
> job for everything sourced from `zpr-utils` — `cbpf-rs`, `zpr-ext`,
> `zpr-utils`, and `cslab`/`rcu` in `zl-zpr-core/adapter/ph` — by repointing
> and re-tagging `zl-zpr-utils` first, so its crates cross-pin each other
> inside the fork and cargo sees one source per crate. One residue outlived
> #18: `zl-zpr-common` pinned `rcu` by the repo-wide tag `zpr-utils-v0.1.0`
> rather than `rcu`'s own tag line, so it never followed `rcu`'s releases;
> `zl-zpr-common` `v0.28.0` re-pins it to `rcu-v0.1.2` (same source, and for
> `rcu/src/` an identical tree). Why it was fixed rather than suppressed is in
> `zpr-dev/docs/specs/spec-003-build.md`, §10 ("Design decisions").
>
> Gate 1 of `zpr-dev build` (see "Compatible build sets" below) now checks
> this class of drift on every set: the same crate at two tags, or at one tag
> from two URLs, is an error before anything compiles.

Two consequences worth knowing:

1. **A local edit to `zl-zpr-common` does not affect a `zl-zpr-core` build.** The
   consumer pins a tag — and, in the forks, a tag in a different repository. To test a change across repositories, either tag and
   push it, or temporarily point the dependency at your checkout:

   ```toml
   zpr = { path = "../zl-zpr-common", features = ["vsapi", "policy"] }
   ```

   Revert that before committing — the pin is intentional.

   Do **not** reach for `[patch]` in `.cargo/config.toml` instead. A bare patch
   entry is *silently ignored*: cargo prints "patch was not used in the crate
   graph" and keeps building against the pinned tag, so the build looks like it
   picked up your change when it did not. Forcing the patch to take requires
   `cargo update -p zpr`, which rewrites the tracked `Cargo.lock` to a local
   absolute path. **A `Cargo.lock` containing a local path must never reach a
   PR** — strip the patch and restore the lockfile before committing.

   A type change in `zl-zpr-common` ripples into both `zl-zpr-core` and
   `zl-zpr-visaservice`. Search usages across every affected checkout, not just
   the repository you started in.

2. **Version bumps are explicit.** Updating shared types means tagging
   `zl-zpr-common` and bumping the tag in each consumer. Bump **every**
   consumer in the same round: `zpr-dev build`'s gate 1 (below) treats
   consumers pinning different tags of the same crate as an error, so a set
   cannot be cut while a bump is half-applied.

   **On `feature/zodes`** (the Zodes epic, org-zpr/zl-zipline#216), the
   consumers pin `zl-zpr-common` at a pre-release tag cut from that branch,
   `zl-v0.32.0-zodes.N`. The first is `zl-v0.32.0-zodes.1`
   (org-zpr/zl-zipline#218). The second is `zl-v0.32.0-zodes.2`
   (org-zpr/zl-zipline#244, the visa `prevHop` field); the consumers moved to
   it together in org-zpr/zl-zipline#245. The third is `zl-v0.32.0-zodes.3`
   (org-zpr/zl-zipline#250, `Link.peerCn` required); core, visaservice and
   compiler move to it together in #250. The fourth is `zl-v0.32.0-zodes.4`
   (org-zpr/zl-zipline#252, `BootstrapVisas`, `Link` without `visas`,
   `updateSubstrateAddr` gone); core, visaservice and compiler move to it
   together in #252. It is on common `bdbcc18`. All three consumers (`zl-zpr-core`,
   `zl-zpr-visaservice`, `zl-zpr-compiler`) move their pin together in
   org-zpr/zl-zipline#238, never one by one inside feature issues. The general
   rule for tags cut from a feature branch is
   [org-zpr/zl-zpr-dev-context#82](https://github.com/org-zpr/zl-zpr-dev-context/pull/82).

Cap'n Proto crates (`capnp`, `capnp-rpc`, `capnpc`) are **lockstep on one minor
across all four Rust repositories** (`zl-zpr-common`, `zl-zpr-compiler`,
`zl-zpr-visaservice`, `zl-zpr-core`), because `zl-zpr-common` publicly
re-exports the generated capnp modules and a `[patch]` only replaces
requirements it is semver-compatible with. Currently **0.27**. Until
zl-zpr-core#93 the FD-passing fork (see "Design decisions") was pinned there
and held all four repositories at 0.26. The 0.25 -> 0.26 upgrade landed as one
round (zipline#135): `zl-zpr-common` moved first and was tagged **`v0.30.0`**,
then `zl-zpr-compiler`, `zl-zpr-visaservice` and `zl-zpr-core` bumped their
`capnp*` crates and their `zpr` pin to that tag in the same set, because
generated capnp code and the `zpr` re-exports must agree on the capnp version.
The 0.26 -> 0.27 upgrade had the same shape (zl-zipline#211): `zl-zpr-common`
moved first and was tagged **`zl-v0.31.0`**, then `zl-zpr-compiler`,
`zl-zpr-visaservice` and `zl-zpr-core` bumped their `capnp*` crates and their
`zpr` pin to that tag in the same set. A future capnp bump follows the same
shape: common first, tag it, then every consumer in one round.

Write the `capnp` and `capnp-rpc` requirements with a patch-level floor
(`"0.27.3"`, not `"0.27"`). A bare minor lets a lockfile resolve a release
older than the upstream security fixes that the dropped fork used to carry (see
the fork's
[`ZIPLINE.md`](https://github.com/mkolehmainen/capnproto-rust/blob/zipline/ZIPLINE.md)).
`capnpc` is a build dependency and takes the minor only (`"0.27"`).

Gate 1 catches a half-applied **`zpr` tag** bump only — it compares git
dependency pins, and the lock scan likewise ignores registry crates — so the
direct crates.io `capnp*` requirements are invisible to it and must be checked
separately: verify every consumer's `capnp`, `capnp-rpc` and `capnpc`
requirements moved in the same round.

**All four repositories resolve the capnp crates to mainline crates.io.**
`zl-zpr-core` no longer patches them to the `mkolehmainen/capnproto-rust`
FD-passing fork (zl-zpr-core#93), so packet capture to a file is compiled out
of normal builds: `ph` answers `setCaptureFile` with Unsupported on every
platform, and `integration-test/capture-test.sh` skips. The fork is 0.26 only
and cannot be combined with 0.27: a `[patch]` does not apply to the
`capnp = "0.27.3"` requirement, and `zpr` `zl-v0.31.0` re-exports 0.27
generated code. So a capture-enabled debug build needs the pre-0.27 dependency
set. In a scratch branch, start from a `zl-zpr-core` revision before the 0.27
bump (`capnp` and `capnp-rpc` 0.26, `zpr` pinned to `zl-v0.30.3`), or revert
the 0.27 bump first. Then revert zl-zpr-core#93's commit. The revert restores
the `[patch.crates-io]` block, the `capnp-ancillary` feature and the capture
test together. Then re-resolve the lockfile
(`cargo update -p capnp -p capnp-futures -p capnp-rpc -p capnpc`), because
against an existing lockfile a new patch is ignored apart from a Cargo warning.
Archive the fork rather than deleting it, so that its pinned rev stays
reachable; its `ZIPLINE.md` records what it carries. The decision record is
under "Design decisions" below.

### Versions and tags

What a repository's own `[package] version` in `Cargo.toml` means, and when a
PR moves it. The per-repository rules:

1. **A version means compatibility, not activity.** Bump it when something
   *outside* the repository has to care — a wire format, a container format, a
   published crate API — not because code changed. "What am I running?" is
   answered by the `git describe` suffix that every binary stamps into its
   `--version` output (`zipline#64`), not by the version number.

2. **`zl-zpr-compiler`'s version is load-bearing — the only one anything
   reads.** The compiler stamps its `CARGO_PKG_VERSION` into every
   `PolicyContainer` it emits (`zl-zpr-compiler/src/compiler.rs:1-2`, written at
   `src/policybinaryv2.rs:208-210`), and the visa service refuses to load a
   container that fails `libeval::pio::check_version`
   (`zl-zpr-visaservice/libeval/src/pio.rs:74`). That check is a near-exact
   match, not a floor: the container's **major must equal** the minimum's, the
   **minor must equal** it too, and the **patch must be greater than or
   equal**. The minimum is `POLICY_MIN_COMPILER_{MAJOR,MINOR,PATCH}`
   (`zl-zpr-visaservice/vs/src/config.rs:29-31`).

   So: bump the compiler's **minor** when the container's meaning or format
   changes, and move `POLICY_MIN_COMPILER_*` with it **in the same build set**.
   **Never move the minor for a non-policy reason** — doing so makes the visa
   service reject policy at runtime for a reason unrelated to policy. Patch
   bumps are free: an old minimum accepts a newer patch.

3. **`zl-zpr-common`, and the `zl-zpr-vsapi` / `zl-zpr-policy` submodules it
   carries: unchanged from consequence 2 above** — tag the crate, then bump
   the pin in every consumer in the same round. That rule is stated there;
   this list only places it among the others.

4. **Everything else** (`zl-zpr-visaservice`, `zl-zpr-core`, `zl-zpr-coredns`,
   `zl-zpr-demo`): no bump per merge. Bump when compatibility changes, or when
   cutting a tag.

5. **A tag matches the cargo version.** Existing practice, now written down:
   when cutting a tag, tag the version the `Cargo.toml` already carries (the
   compiler's `v0.18.1` tags version `0.18.1`). Where a repository's version
   has moved past its newest tag, the next tag continues from the version, not
   from the old tag line.

Why not bump-per-change: a version line that moves in every PR makes every PR
conflict with every other on that line, and two branches rebased past each
other can ship the same number for different code. It would also dilute the
compiler's version — the only one with a runtime consumer — into a change
counter. Build identity is the `git describe` stamp's job (`zipline#64`).

---

## Compatible build sets

A **build set** is the answer to "these binaries go together": one YAML file
naming a ref — tag, branch, or sha — per binary-producing repository, which
`zpr-dev build` proves coherent, builds, tests, and turns into a manifest that
rebuilds the same set later. The full specification is
`zl-zpr-dev-context/zpr-dev/docs/specs/spec-003-build.md`; this section is the
working knowledge.

### The two manifests

- **Input manifest** — `zl-zpr-dev-context/build-sets/<name>.yaml`, committed
  and reviewed. Names the five binary-producing repositories
  (`zl-zpr-compiler`, `zl-zpr-visaservice`, `zl-zpr-core`, `zl-zpr-coredns`,
  `zl-zpr-demo`) with one ref each. **The build is named after the file** —
  the file name without its extension, so `2026-09-25.yaml` builds
  `.zpr-build/2026-09-25/`; the `name:` field inside is optional and ignored
  on input (zipline#115). `zl-zpr-common`, `zl-zpr-policy` and
  `zl-zpr-vsapi` are deliberately absent: cargo consumes them from Git by tag,
  so the set's `zl-zpr-common` version is *derived* from what the consumers
  agree on. An optional `allow_pin_drift:` list tolerates a named crate's pin
  disagreement; every entry must carry a non-empty `reason` — the reviewed
  record of a divergence someone has decided to live with.
- **Emitted manifest** — `dist/zpr-set-<name>.yaml`, written by every run
  whose gates pass (even when a build step or test tier later fails — the most
  interesting set to reproduce is the one that broke). Same schema with every
  ref resolved to a 40-character sha — and the derived name recorded in
  `name:` — plus a diagnostic `resolved:` block
  (pins, versions, host toolchain, per-binary sha256, tier results) that a
  later read ignores. Feeding it back as `--manifest` rebuilds the same set:
  a sha resolves to itself (into `.zpr-build/zpr-set-<name>/`, named after
  the emitted file, so it never collides with the original's directory).

Refs resolve against the **local checkouts, with no fetch** — `zpr-dev`
treats a fetch as a mutation. A tag pushed but never fetched fails as an
unknown ref; fetch first (e.g. `zpr-dev update --all`).

### The three gates

Gates run **before compilation** — a set whose pins disagree must fail in
seconds, not after fifteen minutes of cargo. Findings accumulate into one
`zpr-dev validate`-style report (`[OK]`/`[WARN]`/`[ERROR]`); any error exits 1.

1. **Shared-dep agreement (error).** Every ZPR-family or `rev`-pinned git
   dependency in every manifest of the set — plus `zl-zpr-common`'s own
   manifest, at the revision the consumers pin — must resolve to a single
   `(crate, url, tag-or-rev)`. Two tags for one crate, or one tag from two
   URLs, is an error naming every pinning file and line. An
   `allow_pin_drift` entry suppresses one crate's finding and echoes its
   reason; `--allow-pin-drift` downgrades all findings to warnings for a
   one-off.
2. **Freshness (warning, never an error).** A pin sitting behind the newest
   tag in its repository's checkout warns, ordered by semantic version. A set
   may legitimately sit behind; the warning makes that a decision, not an
   accident.
3. **Compiler / visa-service version (error).** `zplc`'s package version must
   satisfy the visa service's `POLICY_MIN_COMPILER_*` (major ==, minor ==,
   patch >=). A set that fails this builds perfectly and then loads no policy
   at runtime, so it is checked statically here and dynamically again by the
   `unit` tier's `pregen`.

**Transitive pins.** Gate 1 reads manifests only — the root `Cargo.toml` and
literal workspace members — so a pin carried *transitively* by a git
dependency (`zpr-utils-v0.2.2`'s own manifest pinning `zpr v0.8.1`) never
appears in it. A complementary scan
([zipline#69](https://github.com/org-zpr/zl-zipline/issues/69),
`gates::gate_lock_dual_versions`) reads each repository's resolved
`Cargo.lock` and reports any ZPR-family crate present at two versions. Its
finding names the lock and the resolved sources, not the `Cargo.toml` line
that pinned the stale version, which lives inside a tagged dependency the
workspace does not check out.

### The test tiers

| Tier | What runs | Prerequisites | Probe-gated |
|---|---|---|---|
| `unit` | `make test` per built repository, with `make pregen ZPLC=<dist>/zplc` first in `zl-zpr-visaservice` | none beyond the build | no |
| `netns` | the ten `zl-zpr-core/integration-test/` scripts, against `dist/` binaries | Linux, and either (unprivileged user namespaces, `valkey-server`, `python3`) or a reachable Docker daemon | yes |
| `docker` | `dns-demo` deploy + `test-dns.sh` + `docker compose down -v` | `docker`, `docker compose` | yes |

`--test` selects: `none`, `default` (the flag absent means the same), `all`,
or a comma-separated list. `default` selects every tier above with the
probe-gated ones allowed to skip: a tier whose prerequisites are missing is
**skipped with the reason recorded** — in the output and in the emitted
manifest, so a green run never overstates coverage. The same failure on an
explicitly requested tier (named in a list, or via `--test all`) is an error:
the machine cannot run what was asked.

A tier left *out* of the selection is stated too. The emitted manifest
records the literal request as `tests_requested`, lists every tier that did
not run under `tests_skipped` with its reason (`not selected (--test unit)`,
the probe's reason, or `not run: build failed`), and spells each gap out as a
sentence under `notes` — so a set cut with `--test unit` says in its own
words that the integration tiers never ran. `--note "<text>"` (repeatable)
appends operator context to those notes, e.g. why a tier was run by hand
instead.

### Cutting a new set

1. Fetch everything the set will pin: `zpr-dev update --all` (resolution
   itself never fetches).
2. `zpr-dev build --tip --test all` — `--tip` ignores refs and resolves
   `origin/<default_branch>` everywhere (`zipline`; `main` for
   `zl-zpr-coredns`). On a machine that cannot run a tier, drop it from the
   list and let the skip be recorded instead.
3. Review `dist/zpr-set-<name>.yaml`: the `notes:` first — they name every
   tier that did not run and why — then the resolved shas, the `pins:` block,
   and the tier results.
4. Copy it into `build-sets/<date>.yaml` and commit it, unchanged. There is
   no separate authoring step; the emitted manifest *is* the set, and the
   **file name you give the copy is what names the next build** — the
   `name:` field inside is ignored (zipline#115). Default selection picks
   the lexicographically last file name, so when the day already has a set,
   suffix the time as `YYYY-MM-DDTHHMM` (e.g. `2026-09-25T1430.yaml`):
   `T` sorts after `.yaml`'s dot, where `-2` or `.2` would sort before it
   and never be selected.
5. Reproduce before relying on it: `zpr-dev build --manifest
   build-sets/<date>.yaml` must resolve to the same shas and recompute the
   same `pins:` block.

Quick checks along the way: `zpr-dev build --tip --gates-only` runs just the
gates against the live checkouts (read-only, seconds), and `--dry-run` prints
the resolved shas, build order, tier probes and target `dist/` without
creating anything.

Build directory: `<workspace>/.zpr-build/<name>` — `<name>` being the
manifest's file stem, or `tip` (`--build-dir` overrides);
sources come from detached worktrees, so the live checkouts are never
touched. A directory left by a previous run is refused; `--force` removes and
recreates it. Exit codes are `zpr-dev`'s usual contract: 0 success or
warnings, 1 gate/build/test failure, 2 usage or configuration error.

---

## Integration tests

### `zl-zpr-visaservice`

Shell-based, driven by `make test` at the repository root, or directly:

```bash
cd integration-test && make test    # zpt-test.sh, zpt-test-connect.sh, tag-test.sh
```

These exercise `libeval` through `zpt` and need no node and no Valkey.

Test policies are pre-compiled and checked in. Regenerating them needs `zplc`
on `PATH` (or `ZPLC=/path/to/zplc`):

```bash
make pregen     # integration-test/pregen: recompile the .zpl fixtures
```

### `zl-zpr-core`

`integration-test/` stands up a real ZPRnet — node, visa service, and adapters
— on network namespaces. It is not run by `make test`; it needs binaries from
*other* repositories placed next to the scripts:

```text
integration-test/vs                 from zl-zpr-visaservice
integration-test/vs-admin           from zl-zpr-visaservice
integration-test/valkey-server      or set VALKEY_SERVER_BIN
target/debug/ph, target/debug/ph-cli   built here by make
```

The simplest source for the visa-service binaries is a `zl-zpr-visaservice`
release tarball (`make release` there, or download a published release) unpacked
into `integration-test/`. Then:

```bash
ZPR_TEST_VERBOSE=1 VALKEY_SERVER_BIN=/usr/bin/valkey-server \
    integration-test/one-node-v6-test.sh
```

Other entry points, each a standalone script:

| Script | Covers |
|---|---|
| `one-node-test.sh`, `one-node-v6-test.sh` | device-only authentication, IPv4 / IPv6 |
| `capture-test.sh` | packet capture |
| `one-node-oidc-test.sh` | OIDC login through the fake IdP, plus JWKS key rotation |
| `oidc-file-interplay-test.sh` | an `oidc` and a `file` trusted service in one policy |
| `one-node-oidc-renewal-test.sh` | silent OIDC renewal and disconnect-on-revocation. Takes several minutes: the renewal cadence is a real wall clock. It was the acceptance criterion for the node-to-adapter credential request, which landed as zipline#66; its CI job is no longer gated off, but the test has not yet been observed passing (see `docs/OIDC.md`, *Not yet*) |
| `one-node-policy-reauth-test.sh` | policy-install re-authentication end to end (zipline#124): every policy install obliges every connected actor — the node and each docked adapter, OIDC and bootstrap-key arms both — to re-authenticate under the new policy generation within `reauth_deadline`, and whoever cannot is revoked. Takes several minutes: real installs under a continuous ping |
| `node-restart-test.sh` | a node restarting (killed and graceful) under a running visa service brings every link back to Active (zipline#167) |
| `zode-link-test.sh` | the Zode link lifecycle (zl-zipline#250, `docs/ZODE_LINKS.md`): a relay Zode docking the visa service, a Zode with an initial link to it, and a Zode outside the relay's topology. Covers join, restart (rule 5), refusal of an unknown CN (rule 3), substrate loss and redial (rules 9-11), a policy that removes the link (rule 12) and link-state reports |
| `zode-join-test.sh` | two Zodes joining through the relay at once (zl-zipline#254, `docs/ZODE_LINKS.md` J1-J5, J11): each gets its own hand-off of two visa-service IDs, none of them the other's, starts with only those in its table, authenticates, reports its link, registers its VSS, receives topology, has its bootstrap visa superseded, and pings the visa service. No Zode refuses a Stream ID Request |
| `zode-line-test.sh` | zb - relay - zc in a line (zl-zipline#254, Zodes plan W1 and W4), in two legs from a clean start. Relay forwarding on: zb and zc ping each other under their own identities, the relay carries the flow on forward-only visas, and no Zode refuses a Stream ID Request. Forwarding off: both still reach the visa service, zb cannot reach zc, and the relay logs that it refuses the forward-only visas |
| `zode-reroute-test.sh` | #251's re-route (zl-zipline#254, `docs/ZODE_LINKS.md` rules 9, 10, 14, 15 and F3, F4, F6): zb and zc bring up a direct link and ping over it with no forward-only visa on the relay; the zb-zc substrate is blocked, both ends report the link down, and the ping comes back through the relay on forward-only visas. Not covered: the link coming back, which needs a new `setTopology` |
| `zode-wrong-ca-test.sh` | a Zode in the policy whose certificate comes from another CA (zl-zipline#254, Zodes plan W2): the relay refuses it in the handshake, no link exists at either end, it never gets a hand-off or registers, and a control Zode signed by the right CA joins normally |
| `fake-idp-smoke-test.sh` | the fake IdP's own endpoints. **Needs no root and no netns** — run it first when an OIDC test misbehaves |

Useful overrides: `DEBUG_TARGETS` (default `all=INFO`), `PH_BIN`, `VS_BIN`,
`NETEM_PARAMS` for link impairment.

The OIDC tests need a `vs` built from a `zl-zpr-visaservice` that carries the
feature under test; a stale binary left in `integration-test/` from an earlier
checkout is the usual cause of a confusing failure.

These tests are Linux-only and **need no sudo** (zipline#189). Each script
re-execs itself in an unprivileged user + network namespace (`unshare -nr`)
where it is mapped root, builds its network out of anonymous namespaces
pinned by holder processes, and runs `ph` as that namespace's root. A
crashed run leaves nothing behind in `/run/netns`, and two runs at once
cannot see each other. The one host prerequisite is that unprivileged user
namespaces are allowed. Ubuntu 24.04+ blocks them through AppArmor; the fix
is a one-time admin step, which the scripts print when it is missing:

```bash
echo kernel.apparmor_restrict_unprivileged_userns=0 | sudo tee /etc/sysctl.d/60-zpr-userns.conf
sudo sysctl --system
```

Do not start the scripts under `sudo`: they gain nothing from it.
(`remote-node-host-env.sh`, the host side of the manual Windows/macOS node
documents, is the exception: it stays root-based, because its namespaces
must be reachable from a real remote machine — see its header.)

**Where user namespaces are blocked: run them in Docker.** `zpr-dev build`
does this automatically: when the host route's prerequisites are missing but
a Docker daemon is reachable, it runs the netns tier through `make
docker-test` per script, four containers at a time, and records the
provenance as `privilege: container` in the emitted
manifest, with a coverage note naming the real host gap (see
`zpr-dev/docs/specs/spec-003-build.md` §6). For runs outside a build, the
same route is available by hand: `integration-test/Makefile` runs the
same scripts as root inside a throwaway privileged container, so the only host
prerequisite is a running Docker daemon and membership in the `docker` group.
The binaries are still built on the host and bind-mounted in; the image
(`integration-test/Dockerfile`, Ubuntu 26.04 by default so its glibc is at least
as new as a current host's) only supplies the tooling and `valkey-server`:

```bash
make -C integration-test docker-test                        # every *-test.sh, PASS/FAIL summary
make -C integration-test docker-test TEST=one-node-test.sh  # one script, args allowed
make -C integration-test docker-shell                       # root shell for debugging
```

`make integration-test-docker` at the repository root is the same as the first
line. Note this route runs the scripts **serially in one container** — for a
full-tier run prefer `zpr-dev build --test netns --jobs N`, which parallelizes
it; see "Integration tier: how to run it" above for the measured difference.
The container runs `--privileged` because `unshare`, `/dev/net/tun` and
io_uring (which Docker's default seccomp profile blocks) all need it. If the
host's glibc is newer than the image's, the binaries will not load; pass
`BASE_IMAGE=ubuntu:<host release>` to `docker-image` / `docker-test`.

---

## What to build to run a ZPRnet

A minimal ZPRnet is a node and a visa service, so three binaries:

| Binary | Repository | Role |
|---|---|---|
| `ph` | `zl-zpr-core` (`adapter/ph`) | packet handler — runs as node *or* adapter |
| `vs` | `zl-zpr-visaservice` (`vs`) | the visa service |
| `zplc` | `zl-zpr-compiler` | compiles the policy `vs` evaluates |

Plus a running Valkey/Redis for `vs`.

`zl-zpr-core/README.md` has the full walkthrough: generating the bootstrap RSA
keys, the CA and signed noise certificates, the node and adapter TOML configs,
the visa service TLS credentials, and starting everything in order. That
procedure is runtime setup rather than build, so it is not duplicated here.

---

## Building in the container

For a build environment identical to CI, use the dev-env image from
`zl-zpr-dev-tools/docker/dev-env` — Debian 12 with the toolchain, `capnproto`,
`libpcap`, and Valkey already installed. Published images are in the upstream
[org-zpr packages area](https://github.com/orgs/org-zpr/packages); forking a
repository does not copy its packages, so pull the images from there.

```bash
docker run --rm -it -v "$PWD":/work -w /work <dev-env-image> make
```

---

## Troubleshooting

| Symptom | Cause | Fix |
|---|---|---|
| `capnp: command not found`, or `build.rs` fails in `zl-zpr-common` | Cap'n Proto compiler missing | install `capnproto` |
| `zl-zpr-common` build cannot find `.capnp` files | submodules not initialized | `make submodules-pull` |
| Cargo cannot authenticate to `github.com/org-zpr/...` or `github.com/mkolehmainen/...` | SSH-only credentials, or Cargo's Git client | set `url.insteadOf`, or `CARGO_NET_GIT_FETCH_WITH_CLI=true` |
| `openssl` crate fails to build | dev headers missing | install `libssl-dev` / `pkg-config` |
| `pcap` crate fails to build | headers missing | install `libpcap-dev` |
| `vs` exits at startup | no Valkey/Redis | `systemctl start valkey-server`, or run `valkey-server` |
| Visa service rejects a policy | policy signed with the wrong key | re-run `zplc -k` with the key `vs` is configured with |
| CI fails but the local build passed | `make check` not run — warnings are errors in CI | `make check` |
| A change in `zl-zpr-common` has no effect on a consumer | the consumer pins a Git tag | tag and bump, or use a temporary `path` dependency |
| "patch was not used in the crate graph" | a bare `[patch]` cannot override a tag pin | use a temporary `path` dependency instead; never commit a locally-pathed `Cargo.lock` |
| `unresolved import `serde`` in `zl-zpr-common/packet_info.rs` | built without `-F all` | `make build`; `make check` fails this way on clean `main` too |

---

## Design decisions

Rationale carried over from completed master plans, retired once shipped. Full
plan text is in git history (the architecture decisions from the same plan are
in [SYSTEM_OVERVIEW.md](SYSTEM_OVERVIEW.md) "Design decisions"):

- `git show 44e37d0:docs/plans/2026-09-28-windows.md` — umbrella
  [zipline#126](https://github.com/org-zpr/zl-zipline/issues/126)

**Crypto stays on `aws-lc-rs` for Windows.** It supports Windows MSVC; the
build cost is CMake and NASM on the machine (or the
`aws-lc-sys/prebuilt-nasm` feature). Switching to `ring` for Windows was
rejected: two crypto libraries in a security product is worse than one
build-toolchain step.
([zipline#126](https://github.com/org-zpr/zl-zipline/issues/126))

**`ph-cli` builds without `pcap` on Windows.** The `pcap` default feature is
disabled there (`--no-default-features`), so capture-filter-string compilation
is unavailable on Windows until Npcap is worth adding
([zipline#151](https://github.com/org-zpr/zl-zipline/issues/151));
`cbpf-rs` itself is portable.
([zipline#132](https://github.com/org-zpr/zl-zipline/issues/132))

**Mainline capnp; `capnp-ancillary` deleted.** The former `[patch.crates-io]`
fork (`emilazy/capnproto-rust`) did not compile for Windows and blocked the
port; core moved to mainline crates.io capnp and the optional fd-passing
feature was deleted with it — `setCaptureFile` answers Unsupported (no
capture) or redirects to `capture.sock` (unix). *Superseded:* once the port
finished, the fork came back fixed for every target and `capture.sock` was
deleted — see the capnp FD-passing entries below.
([zipline#134](https://github.com/org-zpr/zl-zipline/issues/134))

- `git show c7f15ed:docs/plans/2026-09-29-capnp-fd-passing-unix.md` — umbrella
  [zipline#140](https://github.com/org-zpr/zl-zipline/issues/140)

**Fork base: stay on capnp 0.26, not rebase onto 0.27 (operator direction,
2026-09-29).** The FD-passing fork uses emilazy's PR
capnproto/capnproto-rust#666 (0.26.2 base) plus eight fix/backport commits;
the four repos moved 0.25 → 0.26 in one round (zipline#135, retargeted from
0.27). The alternative — rebasing the 13 fork commits onto 0.27.2 — conflicts
at commit 2 of 13, costs more up front, and diverges further from #666.
([zipline#141](https://github.com/org-zpr/zl-zipline/issues/141))

**Security cost of 0.26 instead of 0.27: nothing security-relevant lost (F1
audit).** Core, on the fork, has every security fix upstream shipped from
0.26.2 through 0.27.2 — either in the base (UB with `pointer::add()`, zeroing
miss, `BufferSegments::new()` panic, `read_message_no_alloc()` desync,
packed-stream EOF bugs, bad-question-ID RPC panic) or cherry-picked (the
peer-triggered re-import `unreachable!()` panic in capnp-rpc, bool
`PrimitiveElement` bounds, `get_data_field` overflow + its regression test).
The upstream changes not taken are a performance fix, an RPC shutdown-ordering
change, and 0.27 API/schema-equality work — none a security fix. The other
three repos, on crates.io 0.26.x, miss only the two 0.27-only
defence-in-depth fixes, which guard internal APIs that generated code does
not call with attacker-controlled values. If upstream ships a security fix
later, follow the fork's
[`ZIPLINE.md`](https://github.com/mkolehmainen/capnproto-rust/blob/zipline/ZIPLINE.md).
*Superseded:* zl-zpr-core#93 dropped the fork; zl-zipline#211 moved all four
repos to capnp 0.27.3.
([zipline#141](https://github.com/org-zpr/zl-zipline/issues/141))

**The fork lives at `mkolehmainen/capnproto-rust`, branch `zipline`, pinned by
`rev`.** Consumers never pin a branch: emilazy's fork was force-pushed once
already, orphaning org-zpr's old pin `cfbcb9b` — GitHub still serves an
orphaned SHA but it can be garbage-collected, so we pin a rev on a branch we
control. The fork also carries copies of emilazy's branches so their SHAs
survive whatever happens upstream. Maintenance rules (fast-forward only, no
version bumps, cherry-pick fixes with their tests) are in the fork's
`ZIPLINE.md`, not here.
([zipline#141](https://github.com/org-zpr/zl-zipline/issues/141))

**Feature shape: unconditional patch, unix-gated use.** Cargo cannot scope a
`[patch]` to a target or feature, and a target-scoped renamed fork package
would drag in a second, type-incompatible `capnp`. So the patch applies
everywhere, the fork builds on every target, and `capnp-ancillary` (a
`ph`/`ph-cli` default feature) pulls `capnp-futures/tokio-unix-fd-stream`
only under `[target.'cfg(unix)'.dependencies]`, with code gated
`cfg(all(unix, feature = "capnp-ancillary"))`; elsewhere `setCaptureFile`
returns Unsupported. A mainline (fork-less) build needs two manifest edits —
delete the patch block *and* delete the feature with its optional
`capnp-futures` dependency — because Cargo rejects any manifest naming a
feature mainline `capnp-futures` does not declare.
([zipline#142](https://github.com/org-zpr/zl-zipline/issues/142))

**`capture.sock` is deleted, not kept as a fallback.** Per org-zpr/zpr-core#1399:
the worker, `--capture-path`, ph-cli `-c/--cap-socket`, the `*_CAP_SOCK`
integration-test variables and the legacy ancillary helper all went; unix
capture hands the FD over the existing `setCaptureFile` admin RPC.
([zipline#142](https://github.com/org-zpr/zl-zipline/issues/142))

**Upstreaming the fork's fixes: neither (operator decision, 2026-10-01).**
The fixes are offered neither to emilazy (capnproto/capnproto-rust#666) nor
to org-zpr. Recorded verbatim from the operator: "Option (D) — neither."
([zipline#143](https://github.com/org-zpr/zl-zipline/issues/143))

**Fork dropped; packet capture compiled out (operator decision,
2026-11-04).** *Supersedes the FD-passing entries above for normal builds.*
`zl-zpr-core` no longer depends on `mkolehmainen/capnproto-rust`: the root
`[patch.crates-io]` block, the `capnp-futures` workspace dependency, and the
`capnp-ancillary` feature in `ph` and `ph-cli` are deleted, and every `capnp*`
crate resolves to crates.io. Turning the feature off was not enough on its own.
Cargo resolves optional git dependencies into `Cargo.lock` whatever features
are on, and a `[patch]` applies regardless of features, so the fork would
still be fetched by every build. The capture code stays in the tree behind
`cfg(all(unix, feature = "capnp-ancillary"))`, and the existing `check-cfg`
lint keeps it warning-free. Why: the fork served only packet capture, a debug
facility, at the cost of a hand-maintained capnp fork that also pinned all four
repositories to capnp 0.26. A capture-enabled debug build reverts the
zl-zpr-core#93 commit (see "Cross-repository dependencies" above).
([zl-zpr-core#93](https://github.com/org-zpr/zl-zpr-core/pull/93))
