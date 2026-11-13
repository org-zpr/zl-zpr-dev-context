# ZPR

The ZPR system is a REFERENCE IMPLEMENTATION.  That means:
- Code should favor readability over clever succinctness.
- Code must to be appropriately commented.


The ZPR system is a secure networking system.  That means:
- Code must be auditable.
- Follow established best practices for building secure software.


Additional coding guidelines:
- Use the DRY principle, favor code reuse and refactor aggressively to achieve this.
- Unit test everything. When a bug is found, before fixing it write a test that fails.
- Unless a function is exceedingly trivial, every function should have a comment explaining what it does.


## INDEX

- `docs/` -> technical knowledge loaded when relevant.
- `docs/plans/` -> master plans from the GitHub-issue flow that are **in flight**: ordering, cross-repository interface contracts, per-issue scope and acceptance criteria. Work planned in `pd` has no file here; its plan, including cross-fork contracts, is the pd plan body. A plan is retired when its umbrella closes -- decisions moved into the spec, file deleted -- so there is usually little or nothing here; `docs/plans/README.md` has the lifecycle and where each retired plan's rationale went.
- `skills/` -> specialized, repeatable agent workflows.
- `zpr-dev/` -> binary for configuring the ZPR development environment.


## Before you start

**This file and everything it points at are a checkout, so they go stale.** Run
`zpr-dev update` first: with no arguments it fetches and fast-forwards the
`zl-zpr-dev-context` checkout only, then regenerates these context files. It never
resets, rebases, stashes or switches branches. Read its output — it skips a dirty or
detached checkout and says so — and if `HEAD` moved, re-read the documents below and
`skills/zpr/SKILL.md` before acting on them.

## Required reading by task

Read these before making the change, not after. Paths are relative to the
shared context checkout; a generated `AGENTS.md` rewrites them to absolute
paths, so they can be opened directly.

| When you are | Read |
|---|---|
| Taking a task through GitHub: issue, plan, branch, PR, review | `skills/zpr/SKILL.md` |
| Picking up work from `pd`, or turning a GitHub issue into a pd plan | `skills/zpr/SKILL.md` ("Work source: pd first"), `pd skill` |
| New to ZPR, or unsure how the pieces fit | `docs/SYSTEM_OVERVIEW.md`, `docs/TERMINOLOGY.md` |
| Unsure which repository owns something | `docs/REPOSITORIES.md` |
| Building, testing, or changing a cross-repository dependency | `docs/BUILD.md` |
| Building a compatible set of binaries, or working a build-set issue | `docs/BUILD.md` ("Compatible build sets"), `zpr-dev/docs/specs/spec-003-build.md` |
| Changing how `zpr-dev build` gates or runs the netns test tier, or its Docker fallback | `zpr-dev/docs/specs/spec-003-build.md` ("Test tiers"), `docs/BUILD.md` |
| Changing how the visa service assigns, pins or checks a ZPR address | `docs/VISA_SERVICE.md`, `docs/SECURITY_MODEL.md` |
| Working on `ph adapter` Windows support (Wintun, `WaitSet`, named-pipe control channel) | `docs/SYSTEM_OVERVIEW.md` ("Design decisions"), `docs/BUILD.md` ("Design decisions") |
| Working an issue under the capnp FD-passing umbrella (the `mkolehmainen/capnproto-rust` fork, unix-only `capnp-ancillary`, retiring `capture.sock`), or changing core's capnp `[patch]` | docs/BUILD.md (incl. ## Design decisions), the fork's ZIPLINE.md |
| Working an issue under the `ph node` on macOS umbrella (macOS `new_mq` address rule, `macos-node-test.md`) | `docs/SYSTEM_OVERVIEW.md` ("Design decisions") |
| Changing ZPL syntax or semantics, or the compiler | `docs/ZPL.md` |
| Changing visa issuance, revocation, or the evaluator | `docs/VISA_SERVICE.md`, `docs/SECURITY_MODEL.md` |
| Changing authentication, identity, attributes, or trusted services | `docs/SECURITY_MODEL.md`, `docs/VISA_SERVICE.md` |
| Changing packet formats, links, docking sessions, forwarding, or compression | `docs/ZDP.md` |
| Changing Zode links: who dials, acceptance, link lifecycle, link-state reports, forwarding (traffic over Zode links) | `docs/ZODE_LINKS.md`, `docs/ZDP.md`, `docs/ROUTING.md` |
| Changing routing, topology, or address assignment | `docs/ROUTING.md`, `docs/SYSTEM_OVERVIEW.md`, `docs/ZDP.md` |
| Changing DNS resolution, the CoreDNS `zpr` plugin, machine hostnames, or a demo's `Corefile` | `docs/DNS.md` |
| Changing anything cryptographic, or touching the enforcement path | `docs/SECURITY_MODEL.md` |
| Writing or reviewing a policy file | `docs/ZPL.md` |
| Changing the topology schema, `Router`/`TopologyMgr`, or how a visa's next hop is chosen | `docs/ROUTING.md` |
| Changing OIDC token validation, JWKS handling, silent re-authentication, or the `api = "oidc"` trusted service | `docs/OIDC.md`, `docs/SECURITY_MODEL.md` |
| Changing an attribute store, the `api = "zpr-attr/1"` attribute-service API, or implementing that API | `docs/ATTRIBUTE_SERVICE.md`, `docs/VISA_SERVICE.md` |
| Writing or reviewing Rust code | `skills/rust-coding-guidelines/SKILL.md` |

Three rules that apply to every task above:

- **A plan wins over the spec it implements while it is in flight, and is
  retired when it completes.** Every plan in `docs/plans/` opens with a
  `**Status:** IN FLIGHT` line. While in flight it fixes ordering, interface
  contracts and acceptance criteria against the code as it actually is, and wins
  wherever it and the spec disagree: read the spec for intent and the plan for
  what to do. When its umbrella closes, the plan's lasting decisions, findings
  and still-open deferred items move into the spec's `## Design decisions`
  section and the plan file is deleted -- git history keeps the full text. So
  there is no such thing as a completed plan to read: for *why* something was
  decided, read the spec's `## Design decisions`. A task-table row in this file
  that points at a plan goes when the plan does.
- **These documents record design intent, not what runs.** The RFCs describe the
  system as designed; each document in `docs/` has an `## Implementation status`
  section recording where the code diverges, and flags divergence inline where
  it matters. **The code wins.** Check the status section before assuming a
  feature exists, and verify against the source before relying on a detail.
- **A change to what policy can express usually spans three repositories** --
  the grammar and compiler in `zl-zpr-compiler`, the schema in `zl-zpr-policy`, and
  the evaluator in `zl-zpr-visaservice`. See `docs/REPOSITORIES.md`.
