# SPEC-003: `zpr-dev build` — compatible build sets

Status: implemented (B1–B6, umbrella `org-zpr/zl-zipline#57`, complete
2026-09-21); the netns container fallback followed (`zipline#90`)
Date: 2026-09-17
Parent spec: `spec-001-zpr-dev.md`
Design rationale: §10

`zpr-dev build` builds the whole ZPR binary set — `vs`, `vs-admin`, `vsapikey`,
`zpt`, `zpr-dashboard`, `zpr-attr-server`, `zplc`, `zpdump`, `ph`, `ph-cli`,
`coredns` — from a
manifest that names a tag, branch or commit per repository, or from the tip of
each default branch. Before compiling anything it proves the set is coherent
(§4); after compiling it runs the test tiers (§6) against those exact binaries
and emits a manifest that rebuilds the same set later (§3).

This document is the auditable description of the feature. The gates in §4 are
the security-relevant part: their wording is what a reviewer checks the code
against.

---

## 1. Scope

### 1.1 In scope (this spec)

- The input **build-set** schema (§2) and the **emitted-manifest** schema (§3).
- Ref resolution: every manifest value resolved to a 40-character commit sha
  against the local checkouts, with no fetch (§2.3).
- The three compatibility gates as normative rules (§4).
- Build order, recipes and `dist/` staging (§5).
- Test tiers and their environment contract (§6).
- The CLI surface and exit codes (§7).

### 1.2 Implementation status

The spec was written whole so the gates and schemas could be reviewed
together, and landed in stages:

| Stage | Implements | Status |
|---|---|---|
| B1 | §2, §3 (schema + round-trip), §2.3 resolution, `--dry-run` (§7.2) | implemented |
| B2 | §4 gates | implemented — plus the `Cargo.lock` dual-version scan (zipline#69, §10) |
| B3 | §5 worktrees, builds, `dist/`, emitted manifest, tarball | implemented |
| B4 | §6 `unit` tier | implemented |
| B5 | §6 `netns` and `docker` tiers | implemented — including the netns tier's container fallback (zipline#92/#93) and its unprivileged user-namespace host route (zipline#192) |

### 1.3 Out of scope (recorded, not scheduled; see §10.4)

No `[patch]` or path override of `zl-zpr-common`; no container build; no CI
wiring; no `target/` reuse across builds; no signing or publishing; no
per-repository tagging of a set. (Parallel repository builds were originally
out of scope too; zipline#144 brought them in — see §5.)

---

## 2. Input manifest: the build set

A build set lives at `zl-zpr-dev-context/build-sets/<name>.yaml`.

```yaml
version: 1
name: 2026-09-17              # optional; ignored on input (see below)

repositories:                 # value is a tag, branch or sha; resolved to a sha
  zl-zpr-compiler:    v0.18.0
  zl-zpr-visaservice: v0.19.0-rc.1
  zl-zpr-core:        zpr-0.3.1
  zl-zpr-coredns:     f4202c4
  zl-zpr-demo:        demo-20251010

allow_pin_drift:              # optional; each entry MUST carry a reason
  - crate: rcu
    reason: "zl-zpr-common pins zpr-utils-v0.1.0, ph pins rcu-v0.1.2; zipline#18"
```

### 2.1 Rules

- Only **binary-producing** repositories appear. `zl-zpr-common`,
  `zl-zpr-policy` and `zl-zpr-vsapi` are deliberately absent: cargo consumes
  them from Git by tag, so pinning them here would describe something that
  does not happen. The set's `zl-zpr-common` version is *derived* from what
  the consumers agree on (gate 1, §4.1).
- A key under `repositories:` that is not a repository in `workspace.yaml` is
  an error naming the key. `url` and `default_branch` come from
  `workspace.yaml`; nothing is duplicated here.
- `version: 1` is the only accepted version, and `repositories` must be
  non-empty.
- **The build's name is derived from the manifest's file name** — the file
  name without its `.yaml`/`.yml` extension — never from the `name:` field,
  which is optional and **ignored on input** (zipline#115: two files carrying
  the same stale `name:` shared one build directory and clobbered each
  other's `dist/`). `--tip` has no file, so it keeps the name `tip`. The
  derived name must be a single safe path component (non-empty, no
  separators, not `.` or `..`); it names `.zpr-build/<name>/`, the emitted
  `dist/zpr-set-<name>.yaml` and the tarball. The emitted manifest still
  writes the derived name into `name:` as a record of what it built as.
  Feeding an emitted manifest back in (`--manifest dist/zpr-set-X.yaml`)
  derives `zpr-set-X` — slightly clunky, harmless, and collision-free with
  the original; the `zpr-set-` prefix gets no special handling.
- Unknown keys are ignored, matching `workspace.yaml`'s tolerance
  (spec-001 §3.1: no `deny_unknown_fields` anywhere), so the schema can grow
  ahead of the tool. This is also what makes the emitted manifest (§3) valid
  input: its `resolved:` block is an unknown key by design.
- An `allow_pin_drift` entry without a non-empty `reason` is an error naming
  the crate. The reason is the reviewed record of a known divergence.

### 2.2 Default manifest selection

`--manifest` defaults to the newest file in `<context>/build-sets/`, by file
name in descending lexicographic order — set names are dates, so the newest
name is the newest set, and a rename cannot silently change which set builds.
Only regular files ending `.yaml` or `.yml` count. An absent or empty
`build-sets/` directory is an error telling the user to pass `--manifest` or
`--tip`.

**Two sets cut on the same day** need the second file named so it sorts
*after* the first — the comparison is byte-wise over the whole file name, so
a suffix inserted before `.yaml` compares against the `.` of `.yaml`
(0x2E), and most separators sort *before* it:

```
2026-09-25-2.yaml     sorts before 2026-09-25.yaml   (wrong)
2026-09-25.2.yaml     sorts before 2026-09-25.yaml   (wrong)
2026-09-25.yaml
2026-09-25T1430.yaml  sorts after                    (recommended)
2026-09-25b.yaml      sorts after
```

Recommended: append `T<HHMM>` (`YYYY-MM-DDTHHMM`) whenever a day already has
a set.

### 2.3 Ref resolution

Every `repositories:` value is resolved to a full 40-character commit sha with
`git rev-parse --verify <ref>^{commit}` in the named repository's workspace
checkout:

- A tag, a branch, a short sha and a full sha all resolve; an annotated tag
  resolves to the commit it points at (`^{commit}`).
- Under `--tip` every manifest value is ignored and
  `origin/<default_branch>` from `workspace.yaml` is resolved instead —
  `zipline` for the forks, `main` for `zl-zpr-coredns`.
- **No fetch, ever.** Resolution runs against whatever the local checkout
  already has; `zpr-dev` treats a fetch as a mutation (spec-001 §5.1), and
  `--dry-run` must not perform one. A tag pushed but never fetched therefore
  fails as an unknown ref; an opt-in `--fetch` is recorded in §10.4,
  not part of this spec.
- An unknown ref is an error naming **both** the repository and the ref. A
  repository directory that is missing, or is not a Git repository, is an
  error naming the repository.

---

## 3. Emitted manifest: the reproducibility contract

Written to `dist/zpr-set-<name>.yaml`. The **same schema as the input**, with
every ref replaced by its resolved 40-character sha, plus a diagnostic
`resolved:` block that a later read ignores. Its `name:` field records the
**derived** build name (§2.1) — what this build actually ran as — not
whatever the input file's `name:` said:

```yaml
version: 1
name: 2026-09-17

repositories:
  zl-zpr-compiler:    3f1c0a9e...   # 40-char sha, always
  zl-zpr-visaservice: 2cf9912b...
  zl-zpr-core:        dd43b4b1...
  zl-zpr-coredns:     f4202c4d...
  zl-zpr-demo:        8ef671f0...

allow_pin_drift:
  - crate: rcu
    reason: "..."

resolved:                          # diagnostic; ignored as input
  built_at: 2026-09-17T18:22:04Z
  built_from: { manifest: build-sets/2026-09-17.yaml, context: 8bfd061, tip: false }
  host: { os: linux, arch: x86_64, rustc: 1.9x.y, cargo: 1.9x.y, go: 1.2x.y, capnp: 1.0.2 }
  pins:                            # what cargo actually compiled against
    - { crate: zpr, url: "https://github.com/org-zpr/zl-zpr-common.git", tag: v0.26.0,
        pinned_by: [zl-zpr-core, zl-zpr-core/adapter/ph, zl-zpr-visaservice, zl-zpr-compiler],
        newest_available: v0.27.0 }
  versions:
    zplc: "0.18.0"
    vs_policy_min_compiler: "0.18.0"
  binaries:
    - { name: vs, sha256: "...", bytes: 18234512, from: zl-zpr-visaservice }
  tests_requested: unit,netns    # the literal --test value; "default" when absent
  tests_skipped:                 # every known tier that did not execute, and why
    docker: "not selected (--test unit,netns)"
  notes:                         # the shortcomings, in plain words; read this first
    - "netns integration tests ran in a privileged Docker container, not on the host (host route unavailable: missing: unprivileged user namespaces (unshare -nr failed; on Ubuntu 24.04+ set kernel.apparmor_restrict_unprivileged_userns=0))"
    - "docker end-to-end tests did NOT run: not selected (--test unit,netns)"
    - "docker tier run by hand on the operator workstation"   # from --note
  tiers:
    unit:   { status: passed }
    netns:  { status: passed, privilege: container }  # privilege: userns | container (§6)
```

Normative behavior:

- `zpr-dev build --manifest dist/zpr-set-<name>.yaml` must resolve to exactly
  the shas listed and recompute the same `pins` block. Nothing outside the
  file, and the repositories it names, is needed. Because a sha resolves to
  itself, re-reading an emitted manifest as input is the identity: this is the
  round-trip property, and it is tested (§8).
- The manifest is emitted **whenever the gates pass**, even if a build step or
  a test tier fails, with the failure recorded in `tiers:`. The most
  interesting set to reproduce is the one that broke.
- `sha256` per binary is for *comparison*, not a reproducibility claim.
  Reproducible **inputs** are guaranteed — same sources, same pins; byte
  identical binaries are not, because cargo output varies with toolchain and
  path. The `host:` block records the toolchain that produced them.
- Promoting a `--tip` run to a named set is: run it, review, copy
  `dist/zpr-set-<name>.yaml` into `build-sets/`, commit. No separate
  authoring step, and nothing inside the file to edit: the copy's **file
  name** is what names the next build (§2.1), so pick a date that sorts last
  (§2.2 has the same-day guidance).
- **Coverage is stated, never inferred** (zipline#87). `tests_requested`
  always records the literal `--test` value (`default` when the flag was
  absent). `tests_skipped` names every tier of §6 that did not execute, with
  the reason: `not selected (--test <value>)` for a tier the selection left
  out, the probe's or repository-missing reason for a `skipped` tier (the
  same text as `tiers.<name>.reason`), or `not run: build failed`. `notes`
  lists each shortcoming as a sentence a person can act on — one per
  `tests_skipped` entry (or the single `no tests ran (--test none)`), one per
  failed tier, one when the netns tier ran through the container fallback
  (the generated coverage note of §6), one when `--allow-pin-drift`
  downgraded gate 1 — followed by
  the operator's `--note` text verbatim. Both `tests_skipped` and `notes`
  are omitted when empty, so their absence means a clean, full run. A
  manifest gated on `--test unit` alone therefore says, in its own words,
  that the integration tiers never ran.

---

## 4. The three gates (normative)

Gates run **before compilation**, against the worktrees (or, until B3, the
live checkouts). A set whose pins disagree must fail in seconds, not after
fifteen minutes of cargo. Findings accumulate and print together in the style
of `zpr-dev validate` (`[OK]` / `[WARN]` / `[ERROR]` lines); any surviving
error exits 1.

### 4.1 Gate 1 — shared-dep agreement (error)

Parse every `Cargo.toml` in every worktree, including workspace members, and
collect each dependency declared with `git = <url>` where the URL is
ZPR-family (`github.com/mkolehmainen/*`, `github.com/org-zpr/*`) or otherwise
pinned by `rev` (a third-party fork a repository pins — e.g. `zl-zpr-core`'s
`[patch.crates-io]` of the `capnp*` crates to
`mkolehmainen/capnproto-rust`, rev-pinned per `docs/BUILD.md`
"Cross-repository dependencies"). Group by `(crate, url)`
and require a single `tag` (or `rev`) across the whole set. Report every
disagreement as:

```text
ERROR pin disagreement: crate `zpr` (https://github.com/org-zpr/zl-zpr-common.git)
  v0.26.0  zl-zpr-core/Cargo.toml:29 (+ adapter/ph, inherited)
  v0.27.0  zl-zpr-visaservice/Cargo.toml:29
  => cut one tag and bump every consumer, or record it in allow_pin_drift
```

The same crate at the same tag from **different URLs** is also a
disagreement: that is what gives cargo two `cslab` crates from one commit and
silently breaks `RcuBox<RcuCslabReader<T>>` (`docs/BUILD.md`). An
`allow_pin_drift` entry suppresses one crate's finding and its reason is
echoed in the output and the emitted manifest; `--allow-pin-drift` downgrades
all findings to warnings for a one-off.

### 4.2 Gate 2 — freshness (warning, never an error)

For each agreed pin whose repository is in the workspace, list tags in that
checkout and warn when a newer one exists, ordering tags by semantic version,
not creation date:

```text
WARN  crate `zpr` is pinned at v0.26.0; zl-zpr-common has v0.27.0
```

A set may legitimately sit behind; the warning makes that a decision, not an
accident. A missing local checkout is an `INFO`, not a failure: cargo fetches
the tag from GitHub either way.

### 4.3 Gate 3 — compiler / visa service version (error)

Read `POLICY_MIN_COMPILER_MAJOR`, `_MINOR`, `_PATCH` from the visa-service
worktree's `vs/src/config.rs` and `version` from `[package]` in the compiler
worktree's `Cargo.toml`. Apply `libeval::pio::check_version`'s rule exactly —
**major equal, minor equal, patch greater or equal** — and on failure report:

```text
ERROR zplc 0.19.0 cannot produce policy for this vs
  zl-zpr-compiler/Cargo.toml:3                version = "0.19.0"
  zl-zpr-visaservice/vs/src/config.rs:29-31   POLICY_MIN_COMPILER = 0.18.0
  rule: major ==, minor ==, patch >=  (libeval/src/pio.rs check_version)
```

A value that cannot be parsed from either file is an error naming the file:
silently skipping this check would be worse than failing, because nothing else
checks it. The gate is duplicated dynamically by the `unit` tier's `pregen`
step (§6), which recompiles the visa service's policy fixtures with the set's
own `zplc`.

---

## 5. Build order, recipes and `dist/`

Sources come from `git worktree add --detach` per repository at the resolved
sha — the live checkouts are never modified: no branch switch, no checkout, no
fetch, no build inside them. Dropping a dangling worktree registration is not
a modification in this sense: `worktree_add` runs `git worktree prune` on the
source repository first (zipline#71), so a build directory deleted by hand —
which strands a registration in every source repository — recovers on its own
with no flag and no manual `git worktree prune`. Prune is scoped by git's own
definition: it drops only registrations whose directory is already gone, so a
live worktree, including one a developer created themselves, is never at
risk. Everything is a release build. The table is in claim order:

| # | Repository | Command(s) | Staged into `dist/` |
|---|---|---|---|
| 1 | `zl-zpr-compiler` | `cargo build --release` | `zplc`, `zpdump` |
| 2 | `zl-zpr-visaservice` | `make release` | `vs`, `vs-admin`, `vsapikey`, `zpt`, `zpr-dashboard`, `zpr-attr-server` |
| 3 | `zl-zpr-core` | `cargo build --release` | `ph`, `ph-cli` |
| 4 | `zl-zpr-coredns` | `make build` (Go) | `coredns` |
| 5 | `zl-zpr-demo` | none | none |

- **Recipes run up to two at a time** (zipline#144): workers claim recipes in
  table order — the same claim-next-work pattern as the netns tier's
  container route — and the stop check and the claim are one synchronized
  decision, so no recipe is ever claimed after a failure is recorded. This is
  safe because no recipe consumes another's output *during the build*: the
  consumers of `dist/` (`pregen`, the demo's policy compilation, the test
  tiers) all run after every recipe has completed, worktrees and `target/`
  dirs are disjoint, and staging writes distinct file names into `dist/`.
  Width is fixed at 2, not a flag: each recipe is itself a parallel `cargo
  build` that can saturate a small host, so the win is overlapping one
  build's serial tail with the next build's start (measured ~1.55x
  wall-clock on an 8-core host), not stacking five cargo invocations.
- **A failure stops further claims.** Later repositories may need the failed
  one's output, and a half-built set must not look built. Builds already in
  flight run to completion so their logs are whole, and the reported failure
  is the first in table order, deterministic however the in-flight builds
  finished.
- Step 1 is claimed first because the visa service's `pregen` and the demo's
  policy compilation both need `zplc` — from `dist/`, after all recipes
  complete, so table position orders the claims rather than gating them.
- Step 2 reuses `zl-zpr-visaservice`'s own `make release` rather than
  reimplementing its staging; the double compile that implies (release for
  `dist/`, debug for `make test`) is accepted and recorded.
- Step 5 builds nothing: the `docker` tier copies `dist/` into
  `dns-demo/bin/`, so the DNS test exercises the set's own binaries.
- Each repository's own `Makefile` stays authoritative; recipes here are thin
  wrappers. No manifest of a built repository is ever rewritten: no
  `[patch]`, no path dependency, no `Cargo.lock` edit.
- Per-step output goes to `logs/<repo>-<step>.log`; the last 40 lines are
  echoed on failure. Build directory: `<workspace>/.zpr-build/<name>`
  (`--build-dir` overrides), `dist/` inside it.

## 6. Test tiers

| Tier | What runs | Prerequisites | Default |
|---|---|---|---|
| `unit` | `make test` in each built repository, with `make pregen ZPLC=<dist>/zplc` first in `zl-zpr-visaservice` | none beyond the build | run |
| `netns` | the `zl-zpr-core/integration-test/` scripts in its run list (`NETNS_SCRIPTS`) | Linux, and either (unprivileged user namespaces — `unshare -nr true` succeeds — `valkey-server`, `python3`) or a reachable Docker daemon | `--test netns` |
| `docker` | `dns-demo` deploy, `test-dns.sh`, `docker compose down -v` | `docker`, `docker compose` | `--test docker` |

- A tier that was asked for and cannot run is an error under `--test all`; a
  tier that was not asked for is reported `skipped` with the probe's failure
  text as the reason. A skipped test is always visible in the output **and**
  in the emitted manifest — a green run must never overstate coverage.
- `netns` binaries come from `dist/` via `PH_BIN` / `VS_BIN` /
  `VS_ADMIN_BIN` / `VALKEY_SERVER_BIN`; nothing is copied into
  `integration-test/`. The script list is explicit, not globbed, and it is
  guarded against drift (zipline#103): planning the tier fails when the
  worktree carries a top-level `integration-test/*-test.sh` that is neither
  in the run list (`NETNS_SCRIPTS`) nor deliberately excluded with a reason
  (`NETNS_EXCLUDED`). Only the top level is judged — `lib/` and
  `unused_or_outdated/` are invisible, matching the Makefile's own
  `$(wildcard *-test.sh)`.
  The other direction is not an error: a build set pinning an older core
  lacks the newest listed scripts, so the tier runs the listed scripts that
  revision carries and the emitted manifest's notes name the rest
  ("netns scripts not in this core revision did NOT run: ...";
  zl-zipline#222). A worktree carrying none of them keeps the full list and
  fails, rather than passing with nothing run.
- The netns tier selects its route **host → container → skip**
  (zipline#92/#93/#192). The host route wins whenever its own prerequisites
  — unprivileged user namespaces (the probe is `unshare -nr true`; on Ubuntu
  24.04+ AppArmor blocks them until
  `kernel.apparmor_restrict_unprivileged_userns=0`), `valkey-server`,
  `python3` — all hold. The scripts need no sudo: each re-execs itself
  under `unshare -nr` and runs as that namespace's mapped root
  (zipline#189). Otherwise, when a Docker daemon is reachable (`docker info`
  succeeds), the same scripts run as root inside the privileged
  container of `zl-zpr-core/integration-test/`'s `make docker-test`, one
  `make` invocation per script with `WORKSPACE=<build-dir>`. Either route
  runs up to four scripts at once (`--jobs N` overrides the width on both).
  When neither route works, the tier skips — or errors when explicitly
  requested — with a reason naming both routes' gaps; a blocked user
  namespace is reported with the sysctl fix. The emitted manifest records
  how the tier was privileged: `privilege: userns` on the host,
  `privilege: container` in the fallback, which also adds a generated
  coverage note quoting the host route's real failure — `netns integration
  tests ran in a privileged Docker container, not on the host (host route
  unavailable: <reason>)` — so a green manifest never implies the tests ran
  on the host. Each route also has a floor on the `zl-zpr-core` worktree,
  checked once the worktree exists. The host route requires a worktree at
  or after `3ad48ed` (zipline#191), whose `integration-test/lib/common_funcs.sh`
  defines `pkill_own`; an older worktree's scripts still call `sudo` and
  share fixed namespace names. A host selection on an older worktree falls
  back to the container when a Docker daemon answered at probe time,
  quoting the host floor as the host route's failure; otherwise the tier
  skips or errors with both gaps. The container route requires a worktree
  at or after `c163628`, the commit whose `integration-test/Makefile`
  defines `FORWARD_ENV`; an older worktree refuses the route with a reason
  naming that floor.
- `a2a-pubkey-test.sh` needs a `ph` built with `enable-security-testing`;
  that binary must never be staged into `dist/`.
- The `docker` tier always runs `docker compose down -v`, pass or fail.
## 7. CLI surface

```text
zpr-dev build [--manifest <path> | --tip] [--test <list>] [--repo <name>]
              [--build-dir <path>] [--keep] [--allow-pin-drift] [--no-tarball]
              [--jobs <n>] [--note <text>]... [--clean]

--manifest <path>   build set to build; default: newest file in build-sets/ (§2.2)
--tip               ignore every ref; use origin/<default_branch> everywhere
--test <list>       none | default | unit | netns | docker | all (comma-separated)
--repo <name>       build only this repository and its prerequisites
--build-dir <path>  default <workspace>/.zpr-build/<name>
--keep              keep worktrees after a successful run
--allow-pin-drift   gate 1 disagreements warn instead of failing
--no-tarball        skip the dist tarball
--jobs <n>          concurrent netns scripts, on either route (default 4; §6)
--note <text>       append this sentence to the emitted manifest's notes
                    (§3); repeatable, recorded verbatim after the generated
                    notes (zipline#87)
--clean             remove the build directory and clear its worktree
                    registrations, then exit (zipline#71)
```

`--clean` is a mode, not a modifier: it conflicts with `--manifest`, `--tip`,
`--test`, `--repo`, `--keep`, `--gates-only`, `--allow-pin-drift`,
`--no-tarball` and `--note` (usage error, exit 2). `--build-dir`
stays allowed — it scopes the clean to that directory; without it the whole
`<workspace>/.zpr-build` tree goes. Cleaning is not per-set: there is no
manifest resolution and nothing to name a set with. After removing the
directory, `git worktree prune` runs in every workspace-manifest repository,
which is what catches registrations whose directories a person already
deleted. `--clean` resolves no refs, fetches nothing, and runs no gates — it
must work on a workspace too broken to resolve a build set. It never runs
`worktree remove --force` on a path the tool did not create; the registration
side is cleared only by prune, whose blast radius is limited to
already-missing directories by git's own definition. Cleaning an already-clean
workspace is a success (exit 0), not an error, and `--keep`'s contract is
untouched: deliberately retained worktrees remain until something —
`--force`, `--clean`, or a person — removes them.

### 7.1 Global options and exit codes

Global `--workspace`, `--context`, `--verbose`, `--quiet`, `--dry-run` and the
exit codes are inherited from spec-001 §5.1 and §6.4 unchanged: `0` success or
warnings, `1` gate/build/test failure, `2` command or configuration error.
`--manifest` together with `--tip` is a usage error (exit 2), enforced by the
argument parser. Flags whose stages have not landed (`--test`, `--repo`,
`--build-dir`, `--keep`, `--allow-pin-drift`, `--no-tarball` before B2–B5)
parse but are inert; `--dry-run` reports the stage as not yet implemented
rather than pretending.

### 7.2 `--dry-run`

Prints the resolved shas, the planned build order, the planned tier list with
prerequisite probe results, and the target `dist/` path — and creates
nothing. No worktree, no directory, no log file, and **no `git fetch`**:
`zpr-dev`'s existing rule that a fetch is already a mutation applies
(spec-001 §5.1). Probes that are themselves read-only (`unshare -nr true`,
`docker` on `PATH`) may run; anything else is reported as "would probe".
Final output is a one-screen summary in the existing `zpr-dev` style.

`--dry-run --clean` follows the same contract: it prints the clean report —
what would be removed and which repositories would be pruned — and removes
nothing; the build directory still exists afterwards (zipline#71).

## 8. Testing

- **Schema unit tests** (`src/build/mod.rs`): a valid manifest parses; an
  unknown top-level key is tolerated; `version: 2` is rejected; a repository
  name absent from `workspace.yaml` is rejected naming it; an
  `allow_pin_drift` entry without a `reason` is rejected; default-manifest
  selection picks the newest name and errors clearly on an absent or empty
  `build-sets/`.
- **Ref-resolution tests** against the `Fixture` of spec-001 §8.1, extended
  with tags and branches: a tag, a branch, a short sha and a full sha resolve
  to the same 40-character sha; an unknown ref errors naming repository and
  ref; `--tip` resolves `origin/<default_branch>` and ignores manifest
  values; a missing or non-Git repository directory errors.
- **`--dry-run` integration tests**: `build --tip --dry-run` prints a
  resolved sha per repository and creates nothing — the workspace tree is
  byte-identical before and after; an unresolvable ref exits 1 naming
  repository and ref; `--manifest` with `--tip` exits 2.
- **Round-trip test**: serialize a resolved set as the emitted manifest,
  re-read it as an input build set, and assert the resolution is identical
  and the `resolved:` block is ignored.
- Gate tests (B2) and worktree/recipe/tier tests (B3–B5) live beside the code
  in `src/build/`; gate 3's tests cite `libeval/src/pio.rs`'s `check_version`
  as their oracle so the two can be compared by eye.

## 9. Dependencies

B1 adds no crates. `toml` (reading pins out of `Cargo.toml`, gate 1) arrives
with B2, and `sha2` (binary digests) with B3 — each recorded in spec-001 §6.1
when it lands, so the manifest and the spec never disagree. No
`cargo-metadata`, no `git2`, no async runtime.

---

## 10. Design decisions

Rationale carried over from the completed master plans, which were retired
once shipped. Full plan text is in git history:

- `git show a35b224:docs/plans/2026-09-17-build-sets.md` — umbrella
  [zipline#57](https://github.com/org-zpr/zl-zipline/issues/57)
- `git show a35b224:docs/plans/2026-09-23-netns-docker-fallback.md` —
  umbrella [zipline#90](https://github.com/org-zpr/zl-zipline/issues/90)

### 10.1 Build sets

**Why build sets exist.** Nothing said "these binaries go together", and each
thing that makes a set compatible was invisible at build time: cargo pins
(not checkouts) decide what compiles, the visa service hardcodes a
near-exact compiler version, and the tests that would catch a bad set needed
foreign binaries staged by hand.
([zipline#57](https://github.com/org-zpr/zl-zipline/issues/57))

**A build set does not pin `zl-zpr-common`.** The first draft pinned it and
compared that ref to the consumers' tags — backwards, since nothing in the
set compiles the checkout. The meaningful invariant is that the consumers
*agree*, at whatever tag; generalizing that to every shared git dependency
(the `zpr-utils` crates, the Cap'n Proto fork) came free.
([zipline#57](https://github.com/org-zpr/zl-zipline/issues/57))

**Gate 3 is static *and* dynamic.** Parsing `POLICY_MIN_COMPILER_*` gives a
readable message before anything compiles; `pregen` with the set's own
`zplc` proves it for real and cannot be fooled by a parser bug. An
unparseable value is an error, because silently skipping the one check
nothing else performs would be worse than failing.
([zipline#59](https://github.com/org-zpr/zl-zipline/issues/59))

**No separate `vs-int` tier.** The visa service's `make test` already runs
its integration tests; a separate tier would reimplement that sequencing and
drift from it. ([zipline#61](https://github.com/org-zpr/zl-zipline/issues/61))

**Release for `dist/`, debug for `make test`, compiled twice.** Rather than
force one profile, `dist/` is release (what demos and tarballs use) and the
netns scripts are pointed at it via `PH_BIN`/`VS_BIN`; the double compile is
the price of not reimplementing `make release`.
([zipline#60](https://github.com/org-zpr/zl-zipline/issues/60))

**Reproducible inputs, not reproducible binaries.** The emitted manifest
guarantees the same sources and pins; `sha256` per binary is for comparison
only, since cargo output varies with toolchain and path. The manifest is
emitted whenever the gates pass, even on a later failure, because the most
interesting set to reproduce is the one that broke.
([zipline#60](https://github.com/org-zpr/zl-zipline/issues/60))

**Transitive pins are checked in `Cargo.lock`, not the manifests.** Gate 1
reads manifests only, so a pin made inside a tagged git dependency
(`zpr-utils-v0.2.2` pinning `zpr v0.8.1`) was invisible while both copies
shipped. The fix scans each repository's resolved `Cargo.lock` for a
ZPR-family crate at two versions (`gates::gate_lock_dual_versions`); a
finding names the lock, not the `Cargo.toml` line, because that file lives
inside a tagged artifact the workspace does not check out. An unreadable
lock is an error, not a skip.
([zipline#69](https://github.com/org-zpr/zl-zipline/issues/69))

**A known drift is fixed, not suppressed.** The first run's `rcu`
disagreement was a stale repo-wide tag in `zl-zpr-common` (same source,
identical `rcu/src/`); it was re-pinned in `zl-zpr-common` `v0.28.0` rather
than recorded in `allow_pin_drift`, whose reason would have cited a closed
issue and left an exception no reader could justify.
([zipline#18](https://github.com/org-zpr/zl-zipline/issues/18),
[zipline#63](https://github.com/org-zpr/zl-zipline/issues/63))

**The committed manifest is a set's identifier.** Sets live in
`zl-zpr-dev-context/build-sets/`, beside `workspace.yaml`; no per-repository
git tag is cut for a set, because a sha is a stronger reference than a tag
that can move. ([zipline#63](https://github.com/org-zpr/zl-zipline/issues/63))

### 10.2 netns container fallback

**Automatic fallback, no flag.** Host route runnable → host; otherwise a
reachable Docker daemon → container; otherwise skip or error as before. The
fallback covers any host-route miss (user namespaces, `valkey-server`,
`python3`), not only the privilege one.
([zipline#92](https://github.com/org-zpr/zl-zipline/issues/92))

**`zpr-dev` drives `zl-zpr-core`'s Makefile; it never owns `docker run`.**
One `make docker-test TEST=<script> WORKSPACE=<build-dir>` per script keeps
the manifest's per-script breakdown and `zpr-dev`'s explicit script list;
the Makefile's run-all target would collapse the per-script results into one
and use
its own glob. `WORKSPACE` is overridden from the `make` command line, which
beats the Makefile's `:=` — the first draft wrongly scheduled a `?=` change
in `zl-zpr-core` (zipline#91, withdrawn); `:=` is better anyway, since an
exported `WORKSPACE` in the operator's shell cannot leak into the mount.
([zipline#93](https://github.com/org-zpr/zl-zipline/issues/93))

**Container-route scripts run four at a time.** Each `make docker-test` is
its own `docker run`, with a private network namespace, `/run/netns`,
`/tmp` and valkey, and the scripts write their scratch files to `mktemp -d`
rather than the bind-mounted workspace, so container runs are independent.
Most of a script's time is spent in fixed `sleep`s, so the tier takes
roughly as long as its slowest script instead of the sum of all of them.
Prep builds (a2a's security-testing `ph`) all run first, serially, so a
cargo build does not compete with running tests. Four is a default, not a
limit: those fixed `sleep`s make heavy CPU contention a flake risk, so
`--jobs` (zipline#144) changes it per run. The host route was serial here,
because its scripts shared fixed `/run/netns` names; it is parallel too
since zipline#192 (§10.3).

**`VALKEY_SERVER_BIN` is removed from the child environment, not merely
omitted.** The Makefile forwards it into the container, where a host path
would not exist, and the child inherits the operator's environment; so the
plan removes it explicitly (`env_remove`).
([zipline#93](https://github.com/org-zpr/zl-zipline/issues/93))

**Provenance is `privilege: container`, and it quotes the real reason.**
The coverage note and the stdout header carry the host route's actual
failure text, because a fixed "user namespaces blocked" would record false
provenance when the miss was `valkey-server`. (The field was `sudo:` with
values `nopasswd | primed | container` until zipline#192 renamed it; build
sets committed before that still carry the old spelling.) ([zipline#93](https://github.com/org-zpr/zl-zipline/issues/93))

**A selected-but-unrunnable route is refused at the gate, never at run
time.** Skipped outcomes do not fail a build, so a run-time skip behind a
passing gate would let an explicitly requested tier exit 0 without running.
([zipline#92](https://github.com/org-zpr/zl-zipline/issues/92))

### 10.3 Unprivileged netns tier

Umbrella [zipline#189](https://github.com/org-zpr/zl-zipline/issues/189):
the integration scripts moved off `sudo ip netns` (#190, #191) and this
tool's gate followed (#192). No master plan was written; the issues carry
the full text.

**Why `unshare -nr`.** Each script re-execs itself in an unprivileged user +
network namespace where it is mapped root, so every `ip`/`tc`/`tuntap` call
works without sudo. Test namespaces are anonymous, each pinned by a `sleep
infinity` holder and entered with `nsenter`. Four things follow: agents run
the tier without credentials; a crashed run cannot leak namespaces into
`/run/netns`, because they die with their holders; namespace names are
private per run, so host runs go in parallel; and `common_funcs.sh` stays
close to upstream `main` (`ed071e4`, `bcfe2fa`), which took the same
approach, so syncs stay cheap.
([zipline#189](https://github.com/org-zpr/zl-zipline/issues/189))

**`ph` runs as the namespace's mapped root, not as the invoking user.**
Under `unshare -nr` there is no `SUDO_UID`, so ph's control-socket owner
resolution (`socket_owner.rs`, zipline#39) finds no owner and leaves the
socket as created, with a startup warning. The scripts pass
`--control-path` explicitly and `ph-cli -p` runs in the same namespace, so
that is harmless. The harness unsets `SUDO_*`/`PKEXEC_UID` before
re-exec: a script started via sudo would otherwise make ph chown the socket
to a uid unmapped in the namespace, and the bind would fail. Operator
decision on the umbrella, 2026-10-13.
([zipline#190](https://github.com/org-zpr/zl-zipline/issues/190),
[zipline#191](https://github.com/org-zpr/zl-zipline/issues/191))

**zipline keeps `/24` substrate addressing; upstream uses peer addressing.**
A `peer` address on a veth is a `/32`, the kernel does not answer ARP for
it on an ARP-capable device, so neighbour entries stay INCOMPLETE and no
adapter docks. TUN devices are NOARP, so `tun0` keeps peer addressing. This
is the one deliberate divergence of `create_network` from `main`.
([zipline#190](https://github.com/org-zpr/zl-zipline/issues/190))

**`pkill`/`pgrep` are scoped to the run's own user namespace.** Concurrent
runs share the invoking user's real uid and the host PID namespace, and use
the same relative socket names and patterns, so a bare `pkill -f` or
`ph_pid_for_socket`'s `pgrep` would hit another run's process.
`pkill_own`/`pgrep_own` add `--ns $$ --nslist user`.
([zipline#191](https://github.com/org-zpr/zl-zipline/issues/191))

**`remote-node-host-env.sh` stays root-based.** Its namespaces must be
reachable from a real remote Windows or macOS node, so their veths end in
the host's network namespace (the container runs `--network host`) and the
host NATs for them. An unprivileged user namespace has no `CAP_NET_ADMIN`
over the host's network namespace. It is a manual tool, not part of this
tier. ([zipline#191](https://github.com/org-zpr/zl-zipline/issues/191))

**The host route has a worktree floor, like the container route.** The
route is chosen from live host probes before any worktree exists, but a
build set can pin a `zl-zpr-core` from before zipline#191, whose scripts
still use `sudo` and fixed namespace names; run on the host, now four at
once, they fail or collide. So once the worktree exists the host route
requires `pkill_own` in `common_funcs.sh` — added by `3ad48ed`, the same
commit that converted every script — and an older worktree falls back to
the container, or skips, exactly as a probe miss would. Found in review of
zl-zpr-dev-context#65; the default build set at the time pinned such a core.
([zipline#192](https://github.com/org-zpr/zl-zipline/issues/192))

**`--prompt-for-sudo` is retired, and the gate probes `unshare -nr true`.**
Nothing in the tier uses sudo any more, so the prompt, the credential
refresher and the `nopasswd`/`primed` provenances (zipline#70) had nothing
left to serve; the flag is now a usage error. A blocked probe still falls
back to the container, and the skip reason names the AppArmor sysctl.
([zipline#192](https://github.com/org-zpr/zl-zipline/issues/192))

### 10.4 Deferred

Recorded, not scheduled; none has an open issue.

- **`--fetch`.** Opt-in fetch before ref resolution is cheap; making it the
  default would break `zpr-dev`'s rule that a fetch is a mutation.
- **`--link-common`.** An untagged `zl-zpr-common` cannot be tested by this
  tool: cut a tag. If that genuinely stalls work, a flag that rewrites only
  a throwaway worktree is the shape to add.
- **`--in-container` builds.** `dist/` is host-built, so a host glibc newer
  than the container image's can break the `docker` tier and the netns
  container route. Workaround: `make docker-image BASE_IMAGE=ubuntu:<host
  release>` in `zl-zpr-core/integration-test/` before the build. No
  `BASE_IMAGE` knob in `zpr-dev` until someone hits it.
- **`--netns-runner host|docker|auto`**, to force the container on a host
  whose user namespaces work.
- **Shared per-repository `CARGO_TARGET_DIR`**, the first thing to try if the
  roughly fifteen-minute cold build becomes the bottleneck.
- **Narrowing `--privileged`** to a capability set — belongs to
  `zl-zpr-core/integration-test/Makefile`.
- **Converting `containerized-demo`'s release flow** to `zpr-dev build`.
