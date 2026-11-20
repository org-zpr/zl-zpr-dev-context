# zl-zpr-dev-context

Agent context for ZPR development: the coding standards, architecture
documentation, and skills that every coding agent working in a `zl-zpr-*`
repository should see.

This is the **zipline** fork of the ZPR workspace. Every repository it names —
with one exception — is
a fork, living in the [`org-zpr`](https://github.com/org-zpr) organization
beside its upstream: `zl-zpr-core` forks `org-zpr/zpr-core`, and so on. The
forks were created under the personal `mkolehmainen` account and transferred to
`org-zpr` on 2026-10-29 (`org-zpr/zl-zipline#207`); old URLs redirect. See
[`docs/REPOSITORIES.md`](docs/REPOSITORIES.md).

Each fork has two long-lived branches: **`zipline`** is the working branch and
the repository default, and **`main`** is a read-only mirror of upstream `main`.
Feature branches start from `zipline` and pull requests target it. See
[`docs/REPOSITORIES.md`](docs/REPOSITORIES.md) ("Branch model") for how to sync
and merge without breaking that. The exception is
[`zl-zpr-coredns`](https://github.com/org-zpr/zl-zpr-coredns) (Go): it is
**not a fork**, so it has no upstream mirror to keep pristine — its working and
default branch is **`main`**, and PRs there target `main`.

`docs/` is the documentation agents read; the specs behind the tooling live in
`zpr-dev/docs/specs/` (`spec-001-zpr-dev.md` for the tool as built,
`spec-000-parent.md` for the original design record).

## For humans: what still needs doing by hand

The fork rename and the branch model are done. All eleven repositories exist as
`org-zpr/zl-zpr-*`; every fork has a `zipline` branch created from `main`
and `zipline` as its default branch (`zl-zpr-coredns` is not a fork and works
on `main`). The list below is what tooling
could not or should not do on its own.

**Once per clone** — `gh` points a fork's commands at its parent by default, so
without this a `gh pr create` offers to open your PR against `org-zpr`:

```bash
gh repo set-default org-zpr/zl-zpr-<name>
```

**Once per repository, if you want them** (each needs a decision, not just a
command):

| Action | Why it is not automated |
|---|---|
| Protect `main` against direct pushes: `gh api -X PUT repos/org-zpr/zl-zpr-<n>/branches/main/protection ...` | `main` is only a mirror by convention right now; nothing enforces it. Protecting it makes the convention real, but it is a policy call about your own workflow. |
| Widen the CI `push` branch filters to include `zipline` | The workflows use `push: branches: ["main"]`. `pull_request` has no branch filter, so **the PR gate already works on zipline PRs** — only post-merge push builds are silent. Widening it means a commit in each of the ten forks. |
| Confirm the `zipline` board can hold fork issues | Project 5 lives in `org-zpr` while the repositories are personal forks. It currently tracks an *upstream* issue. Adding an issue from a fork has not been tested — try it once before relying on it. |

**Routine, whenever you want upstream changes:**

```bash
# 1. fast-forward the fork's main from upstream (--branch main is required:
#    the repository default is now zipline)
gh repo sync org-zpr/zl-zpr-<name> --source org-zpr/zpr-<name> --branch main

# 2. merge it into zipline and push
git fetch origin && git checkout zipline && git merge origin/main
git push origin zipline
```

`zpr-dev update` does not do this — it only fast-forwards the current branch from
`origin`, never from upstream.

**Except `zl-zpr-core`, once zl-zipline#237 lands:** it takes upstream fixes
by cherry-pick instead of step 2 (`docs/REPOSITORIES.md`, "Taking upstream
changes into zipline").

## `zpr-dev`

`zpr-dev` makes the workspace reproducible. It clones the repository set listed
in `workspace.yaml` side by side, then renders this repository's `AGENTS.md`
(plus each repository's optional `AGENTS.repo.md`) into a generated
`<repo>/AGENTS.md` and `<repo>/CLAUDE.md`, with documentation references
rewritten to absolute paths so an agent in `zl-zpr-core` can actually open them.

| Command | What it does |
|---|---|
| `zpr-dev setup` | Clone the workspace, generate context files, validate |
| `zpr-dev update` | Fetch and fast-forward (context only; `--all` for every repository) |
| `zpr-dev status` | Report each checkout and whether generated context is current |
| `zpr-dev sync` | Regenerate the context files (no network access) |
| `zpr-dev validate` | Check workspace health; exit 1 on errors |
| `zpr-dev agent configure hermes` | Point Hermes at this repository's `skills/` directory |
| `zpr-dev agent status` | Report whether each agent is configured |

No command ever resets, rebases, stashes, pushes, switches branches, or touches
uncommitted work. `--dry-run` suppresses every mutation and prints what would
have happened.

The one file outside the workspace that `zpr-dev` will edit is
`~/.hermes/config.yaml`, only under `agent configure`, and only to add this
repository's `skills/` directory to `skills.external_dirs`. It backs the file up
first and verifies that the edit changed exactly that one key.

### Install and bootstrap

```bash
cargo install --path zl-zpr-dev-context/zpr-dev
```

There is an ordering wrinkle: the tool ships inside the repository it clones. If
you clone `zl-zpr-dev-context` to an arbitrary location and then run `zpr-dev
setup`, you end up with a second copy under `<workspace>/zl-zpr-dev-context`. Clone
into the workspace from the start instead:

```bash
mkdir -p ~/src/zl_zpr
git clone git@github.com:org-zpr/zl-zpr-dev-context.git ~/src/zl_zpr/zl-zpr-dev-context
cargo install --path ~/src/zl_zpr/zl-zpr-dev-context/zpr-dev
zpr-dev setup
```

`setup` then finds the existing context checkout and leaves it alone. If you
keep the checkout elsewhere, pass `--context <path>` (and `--workspace <path>`
if the workspace is not `~/src/zl_zpr`; `$ZPR_WORKSPACE` works too).

Claude and Codex need nothing further: they read the generated `AGENTS.md` and
`CLAUDE.md` from whichever repository they are started in. Hermes discovers
skills through its own configuration, so if you use it, run this once:

```bash
zpr-dev agent configure hermes
```

It is idempotent, and `zpr-dev agent status` reports the result.

### Generated files are untracked

The generated `AGENTS.md` and `CLAUDE.md` are deliberately not committed, and
`zpr-dev` does not manage `.gitignore` or `.git/info/exclude`. They will
therefore show up as untracked files in `git status` in every repository. That
is expected — the context is rendered per workspace, so committing it would
bake one developer's absolute paths into the repository.

A repository that already tracks its own `AGENTS.md` is the one case to handle
by hand. `zpr-dev` will not overwrite a file it did not generate — `sync` prints
`refusing to overwrite ...` and `validate` fails — because doing so silently
destroys conventions nothing else records. Rename that file to
`AGENTS.repo.md`, which generation appends under a "Repository-Specific
Context" heading instead of replacing, and stop tracking `AGENTS.md`/`CLAUDE.md`.

If you would rather the shared context just win, `--force` overwrites the file
instead of refusing: `zpr-dev --force sync` clobbers it and says so, and
`zpr-dev --force validate` reports a warning rather than an error. The clobber
is always announced — the flag suppresses the error, not the notice.
