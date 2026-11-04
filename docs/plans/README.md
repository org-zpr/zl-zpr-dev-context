# Master plans

A master plan sequences a multi-issue feature: ordering, cross-repository
interface contracts, per-issue scope and acceptance criteria. It is named
`YYYY-MM-DD-<topic>.md`, opens with `**Status:** IN FLIGHT`, and gets a row in
the `AGENTS.md` required-reading table while it is in flight.

**Only features that span forks get a file here.** Work is planned in `pd`
(see "Work source: pd first" in `skills/zpr/SKILL.md`), one pd plan per fork. A
single-fork plan lives in its pd plan body and needs no file. A feature that
needs several pd plans keeps its cross-fork interface contracts and ordering in a
master plan here, and each pd plan links to it.

**Only in-flight plans live here.** When the umbrella closes, its final
documentation-closure child retires the plan: the lasting decisions, findings and
still-open deferred items move into the spec's `## Design decisions` section, the
plan file is deleted, and every reference to it (including its `AGENTS.md` row)
is repointed at the spec. Git history keeps the full text. The procedure is
"Plan retirement" in `skills/zpr/SKILL.md`.

Retired plans, and where their rationale now lives:

| Plan (`git show a35b224:docs/plans/<file>`) | Umbrella | Rationale now in |
|---|---|---|
| `2026-09-02-oidc-implementation-plan.md` | [zipline#1](https://github.com/org-zpr/zl-zipline/issues/1) | `docs/OIDC.md` |
| `2026-09-14-trusted-service-interplay.md` | [zipline#22](https://github.com/org-zpr/zl-zipline/issues/22) | `docs/VISA_SERVICE.md` |
| `2026-09-15-dns-integration.md` | [zipline#34](https://github.com/org-zpr/zl-zipline/issues/34) | `docs/DNS.md` |
| `2026-09-16-silent-oidc-reauth.md` | [zipline#40](https://github.com/org-zpr/zl-zipline/issues/40) | `docs/OIDC.md` |
| `2026-09-17-build-sets.md` | [zipline#57](https://github.com/org-zpr/zl-zipline/issues/57) | `zpr-dev/docs/specs/spec-003-build.md` §10 |
| `2026-09-17-machine-hostname-dns.md` | [zipline#49](https://github.com/org-zpr/zl-zipline/issues/49) | `docs/DNS.md` |
| `2026-09-22-attr-query.md` | [zipline#72](https://github.com/org-zpr/zl-zipline/issues/72) | `docs/ATTRIBUTE_SERVICE.md` |
| `2026-09-23-netns-docker-fallback.md` | [zipline#90](https://github.com/org-zpr/zl-zipline/issues/90) | `zpr-dev/docs/specs/spec-003-build.md` §10 |
| `2026-09-24-static-address-grants.md` | [zipline#95](https://github.com/org-zpr/zl-zipline/issues/95) | `docs/VISA_SERVICE.md` |
| `2026-09-25-retire-authored-address-pins.md` (`git show b817c70:docs/plans/<file>`) | [zipline#106](https://github.com/org-zpr/zl-zipline/issues/106) | `docs/VISA_SERVICE.md` |
| `2026-09-28-reauth-on-policy-install-plan.md` (`git show 43d56bd:docs/plans/<file>`) | [zipline#118](https://github.com/org-zpr/zl-zipline/issues/118) | `docs/VISA_SERVICE.md` |
| `2026-09-28-windows.md` (`git show 44e37d0:docs/plans/<file>`) | [zipline#126](https://github.com/org-zpr/zl-zipline/issues/126) | `docs/SYSTEM_OVERVIEW.md`, `docs/BUILD.md` |
| `2026-09-29-capnp-fd-passing-unix.md` (`git show c7f15ed:docs/plans/<file>`) | [zipline#140](https://github.com/org-zpr/zl-zipline/issues/140) | `docs/BUILD.md` |
| `2026-10-01-win-node.md` (`git show cc02abd:docs/plans/<file>`) | [zipline#149](https://github.com/org-zpr/zl-zipline/issues/149) | `zl-zpr-core/docs/SETUP.md` "Windows", `docs/SYSTEM_OVERVIEW.md` "Design decisions" |
| `2026-10-01-mac-node.md` (`git show 7323267:docs/plans/<file>`) | [zipline#158](https://github.com/org-zpr/zl-zipline/issues/158) | `zl-zpr-core/docs/SETUP.md` "macOS", `docs/SYSTEM_OVERVIEW.md` "Design decisions" |
