//! Thin wrappers over the installed `git` binary (spec §6.3). Git operations are
//! never reimplemented here: every function shells out to `git` and interprets
//! its output. Nothing in this module resets, rebases, stashes, switches
//! branches, or pushes — the safety invariants of spec §11 hold by construction.

use std::path::Path;
use std::process::Command;

use anyhow::{Result, bail};

/// Runs `git` with `args` in `dir` and returns its trimmed stdout. A nonzero
/// exit becomes an error carrying git's stderr.
pub fn git(dir: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .map_err(|e| anyhow::anyhow!("cannot run git in {}: {e}", dir.display()))?;

    if !output.status.success() {
        bail!(
            "git {} failed in {}: {}",
            args.join(" "),
            dir.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// True when `dir` is itself the root of a Git working tree. The comparison
/// against the reported top level matters: without it, any plain directory
/// *inside* a repository would answer true.
pub fn is_repo(dir: &Path) -> bool {
    let Ok(toplevel) = git(dir, &["rev-parse", "--show-toplevel"]) else {
        return false;
    };
    match (std::fs::canonicalize(toplevel), std::fs::canonicalize(dir)) {
        (Ok(top), Ok(here)) => top == here,
        _ => false,
    }
}

/// The abbreviated commit hash of `HEAD`.
pub fn head_short(dir: &Path) -> Result<String> {
    git(dir, &["rev-parse", "--short", "HEAD"])
}

/// The current branch name, or `None` when `HEAD` is detached.
pub fn branch(dir: &Path) -> Result<Option<String>> {
    let name = git(dir, &["rev-parse", "--abbrev-ref", "HEAD"])?;
    // `--abbrev-ref` reports the literal string "HEAD" for a detached head.
    Ok(if name == "HEAD" { None } else { Some(name) })
}

/// True when the working tree has staged or unstaged changes to *tracked*
/// files.
///
/// Spec §6.3 words this as "`git status --porcelain` non-empty", which would
/// also count untracked files — but `zpr-dev` deliberately leaves the generated
/// `AGENTS.md` and `CLAUDE.md` untracked (§1.4.3, no ignore-file management), so
/// every synced repository would read as dirty forever: `status` would always
/// say `modified` and `update --all` would always skip. `--untracked-files=no`
/// costs nothing in safety — an untracked file that a fast-forward would
/// overwrite still makes `git merge --ff-only` refuse, which [`ff_merge`]
/// reports as `cannot fast-forward`.
pub fn is_dirty(dir: &Path) -> Result<bool> {
    Ok(!git(dir, &["status", "--porcelain", "--untracked-files=no"])?.is_empty())
}

/// Commits ahead of, and behind, the upstream of the current branch. `None`
/// when there is no upstream to compare against (which includes a detached
/// `HEAD`), so callers can report "no upstream" rather than an error.
pub fn ahead_behind(dir: &Path) -> Result<Option<(usize, usize)>> {
    let Ok(counts) = git(dir, &["rev-list", "--left-right", "--count", "HEAD...@{u}"]) else {
        return Ok(None);
    };
    // Output is "<ahead>\t<behind>": left of the three-dot range is HEAD.
    let mut fields = counts.split_whitespace();
    match (fields.next(), fields.next()) {
        (Some(ahead), Some(behind)) => Ok(Some((ahead.parse()?, behind.parse()?))),
        _ => bail!("unexpected git rev-list output: {counts:?}"),
    }
}

/// Clones `url` into `dest`, optionally at a specific branch. The parent
/// directory is created if needed, since `setup` may be cloning into a
/// workspace that does not exist yet.
pub fn clone(url: &str, dest: &Path, branch: Option<&str>) -> Result<()> {
    let parent = dest.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent)?;

    let dest = dest.to_string_lossy();
    let mut args = vec!["clone"];
    if let Some(branch) = branch {
        args.push("--branch");
        args.push(branch);
    }
    args.push(url);
    args.push(&dest);

    git(parent, &args)?;
    Ok(())
}

/// Fetches from the default remote. Does not modify the working tree.
pub fn fetch(dir: &Path) -> Result<()> {
    git(dir, &["fetch"])?;
    Ok(())
}

/// Fast-forwards the current branch onto its upstream. Returns `false` when a
/// fast-forward is not possible — an expected outcome for a diverged branch,
/// not a failure — and leaves the repository untouched in that case.
pub fn ff_merge(dir: &Path) -> Result<bool> {
    Ok(git(dir, &["merge", "--ff-only", "@{u}"]).is_ok())
}

/// Resolves `rev` — a tag, branch, short or full sha — to the full 40-character
/// sha of the commit it names, without fetching. `^{commit}` peels annotated
/// tags to the commit they point at. An unknown rev is an error naming it,
/// because "unknown ref" with no ref is undiagnosable from a build report.
pub fn rev_parse(dir: &Path, rev: &str) -> Result<String> {
    git(
        dir,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{rev}^{{commit}}"),
        ],
    )
    .map_err(|_| {
        anyhow::anyhow!(
            "unknown ref {rev} in {} (refs are resolved locally; zpr-dev never fetches)",
            dir.display()
        )
    })
}

/// The repository's tags, one per line from `git tag --list`. Empty when there
/// are none. Gate 2 (spec-003 §4.2) orders these by semantic version itself;
/// this returns git's plain listing.
pub fn tag_list(dir: &Path) -> Result<Vec<String>> {
    let output = git(dir, &["tag", "--list"])?;
    Ok(output
        .lines()
        .map(str::to_string)
        .filter(|line| !line.is_empty())
        .collect())
}

/// Adds a detached worktree of `repo` at `dest`, checked out at `sha`
/// (spec-003 §5). Detached means the source checkout's branch, `HEAD` and
/// working tree are untouched — the worktree shares the object store and
/// nothing else. A `dest` that already exists and is not empty is refused
/// naming the path: half-overwriting a previous run's directory is how stale
/// binaries get shipped (task B3 step 1).
pub fn worktree_add(repo: &Path, dest: &Path, sha: &str) -> Result<()> {
    // Prune first, mirroring the prune-after in `worktree_remove` below: a
    // build directory deleted by hand leaves every registration behind, and
    // git then refuses the very same path with "missing but already
    // registered worktree" (zipline#71). Prune is scoped by git's own
    // definition — it drops only registrations whose directory is already
    // gone — so a live worktree, anyone's, is untouchable by construction.
    git(repo, &["worktree", "prune"])?;
    // `git worktree add` itself refuses a non-empty directory, but its message
    // names neither our context nor the remedy; check first so the error is
    // diagnosable from a build report.
    if dest.exists() && std::fs::read_dir(dest)?.next().is_some() {
        bail!(
            "worktree destination {} already exists and is not empty; \
             remove it or build with --force",
            dest.display()
        );
    }
    git(
        repo,
        &["worktree", "add", "--detach", &dest.to_string_lossy(), sha],
    )?;
    Ok(())
}

/// Removes the worktree of `repo` at `dest` and prunes stale administrative
/// entries (spec-003 §5). `--force` because build worktrees are throwaways:
/// logs and build artifacts in them must not block removal.
pub fn worktree_remove(repo: &Path, dest: &Path) -> Result<()> {
    git(
        repo,
        &["worktree", "remove", "--force", &dest.to_string_lossy()],
    )?;
    git(repo, &["worktree", "prune"])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Pins identity and signing per repository so results never depend on the
    /// developer's global Git configuration.
    fn configure(dir: &Path) {
        git(dir, &["config", "user.name", "zpr-dev tests"]).unwrap();
        git(dir, &["config", "user.email", "tests@example.invalid"]).unwrap();
        git(dir, &["config", "commit.gpgsign", "false"]).unwrap();
    }

    /// Creates a working repository at `dir` with one commit.
    fn init_repo(dir: &Path) {
        std::fs::create_dir_all(dir).unwrap();
        git(dir, &["init", "-b", "main"]).unwrap();
        configure(dir);
        commit_file(dir, "README.md");
    }

    /// Writes `name` and commits it.
    fn commit_file(dir: &Path, name: &str) {
        std::fs::write(dir.join(name), format!("{name} contents\n")).unwrap();
        git(dir, &["add", "-A"]).unwrap();
        git(dir, &["commit", "-m", &format!("add {name}")]).unwrap();
    }

    /// Creates a bare origin under `root`, seeded with one commit.
    fn seeded_origin(root: &Path) -> PathBuf {
        let origin = root.join("origin.git");
        std::fs::create_dir_all(&origin).unwrap();
        git(&origin, &["init", "--bare", "-b", "main"]).unwrap();
        advance_origin(root, &origin, "seed.md");
        origin
    }

    /// Moves the bare origin forward by one commit, via a throwaway clone —
    /// the only way to add a commit to a bare repository without reimplementing
    /// git. (The push lives in the fixture; no command ever pushes.)
    fn advance_origin(root: &Path, origin: &Path, file: &str) {
        let scratch = root.join(format!("scratch-{file}"));
        clone(&origin.to_string_lossy(), &scratch, None).unwrap();
        configure(&scratch);
        commit_file(&scratch, file);
        git(&scratch, &["push", "origin", "HEAD:main"]).unwrap();
        std::fs::remove_dir_all(&scratch).unwrap();
    }

    /// A bare origin with one commit plus a fresh clone of it at `root/work`.
    fn origin_and_clone(root: &Path) -> (PathBuf, PathBuf) {
        let origin = seeded_origin(root);
        let work = root.join("work");
        clone(&origin.to_string_lossy(), &work, None).unwrap();
        configure(&work);
        (origin, work)
    }

    #[test]
    fn is_repo_false_for_plain_directory() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(!is_repo(tmp.path()));
    }

    #[test]
    fn is_repo_true_after_init() {
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path());
        assert!(is_repo(tmp.path()));
        // A subdirectory of a repository is not itself a repository.
        let nested = tmp.path().join("nested");
        std::fs::create_dir(&nested).unwrap();
        assert!(!is_repo(&nested));
    }

    #[test]
    fn head_short_returns_seven_or_more_hex_chars() {
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path());
        let sha = head_short(tmp.path()).unwrap();
        assert!(sha.len() >= 7, "unexpectedly short sha: {sha:?}");
        assert!(
            sha.chars().all(|c| c.is_ascii_hexdigit()),
            "not hex: {sha:?}"
        );
    }

    #[test]
    fn branch_returns_name_on_branch() {
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path());
        assert_eq!(branch(tmp.path()).unwrap().as_deref(), Some("main"));
    }

    #[test]
    fn branch_returns_none_when_detached() {
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path());
        let first = git(tmp.path(), &["rev-parse", "HEAD"]).unwrap();
        commit_file(tmp.path(), "second.md");
        git(tmp.path(), &["checkout", &first]).unwrap();
        assert_eq!(branch(tmp.path()).unwrap(), None);
    }

    #[test]
    fn is_dirty_false_when_clean_true_after_edit() {
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path());
        assert!(!is_dirty(tmp.path()).unwrap());

        // An untracked file alone is not dirtiness: that is what the generated
        // context files look like in every synced repository.
        std::fs::write(tmp.path().join("AGENTS.md"), "generated\n").unwrap();
        assert!(!is_dirty(tmp.path()).unwrap());

        std::fs::write(tmp.path().join("README.md"), "edited\n").unwrap();
        assert!(is_dirty(tmp.path()).unwrap());
    }

    #[test]
    fn ahead_behind_none_without_upstream() {
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path());
        assert_eq!(ahead_behind(tmp.path()).unwrap(), None);
    }

    #[test]
    fn ahead_behind_counts_after_origin_advances() {
        let tmp = tempfile::tempdir().unwrap();
        let (origin, work) = origin_and_clone(tmp.path());
        advance_origin(tmp.path(), &origin, "later.md");
        fetch(&work).unwrap();
        assert_eq!(ahead_behind(&work).unwrap(), Some((0, 1)));
    }

    #[test]
    fn ff_merge_true_when_behind() {
        let tmp = tempfile::tempdir().unwrap();
        let (origin, work) = origin_and_clone(tmp.path());
        let before = head_short(&work).unwrap();
        advance_origin(tmp.path(), &origin, "later.md");
        fetch(&work).unwrap();

        assert!(ff_merge(&work).unwrap());
        assert_ne!(head_short(&work).unwrap(), before);
        assert!(work.join("later.md").exists());
    }

    #[test]
    fn ff_merge_false_on_divergence() {
        let tmp = tempfile::tempdir().unwrap();
        let (origin, work) = origin_and_clone(tmp.path());
        commit_file(&work, "local.md");
        let before = head_short(&work).unwrap();
        advance_origin(tmp.path(), &origin, "remote.md");
        fetch(&work).unwrap();

        assert!(!ff_merge(&work).unwrap());
        assert_eq!(head_short(&work).unwrap(), before);
        assert!(!is_dirty(&work).unwrap());
    }

    /// A tag (annotated, so `^{commit}` peeling matters), a branch, a short sha
    /// and the full sha all name the same commit and must resolve identically.
    #[test]
    fn rev_parse_resolves_tag_branch_short_and_full_sha_identically() {
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path());
        let full = git(tmp.path(), &["rev-parse", "HEAD"]).unwrap();
        assert_eq!(full.len(), 40);
        git(tmp.path(), &["tag", "-a", "v1.0.0", "-m", "release"]).unwrap();
        git(tmp.path(), &["branch", "release-line"]).unwrap();

        for rev in ["v1.0.0", "release-line", &full[..7], full.as_str()] {
            assert_eq!(rev_parse(tmp.path(), rev).unwrap(), full, "rev {rev}");
        }
    }

    /// The error must name the rev: a bare "unknown ref" in a build report is
    /// undiagnosable.
    #[test]
    fn rev_parse_unknown_ref_errors_naming_it() {
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path());
        let error = rev_parse(tmp.path(), "no-such-tag")
            .unwrap_err()
            .to_string();
        assert!(error.contains("no-such-tag"), "{error}");
    }

    #[test]
    fn tag_list_returns_tags_or_empty() {
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path());
        assert!(tag_list(tmp.path()).unwrap().is_empty());

        git(tmp.path(), &["tag", "v0.1.0"]).unwrap();
        git(tmp.path(), &["tag", "v0.2.0"]).unwrap();
        let mut tags = tag_list(tmp.path()).unwrap();
        tags.sort();
        assert_eq!(tags, vec!["v0.1.0", "v0.2.0"]);
    }

    // -- worktree lifecycle (spec-003 §5, task B3 step 1) ---------------------

    /// A detached worktree appears at the requested sha, and the source
    /// checkout — branch, `HEAD`, and a dirty file — is byte-identical
    /// afterwards. This is the §5 guarantee that a build never touches the
    /// live checkouts.
    #[test]
    fn worktree_add_is_detached_at_sha_and_leaves_source_untouched() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        init_repo(&repo);
        let first = git(&repo, &["rev-parse", "HEAD"]).unwrap();
        commit_file(&repo, "second.md");
        let head_before = git(&repo, &["rev-parse", "HEAD"]).unwrap();

        // Dirty the source checkout: the worktree must not disturb it.
        std::fs::write(repo.join("README.md"), "dirty edit\n").unwrap();

        let dest = tmp.path().join("wt");
        worktree_add(&repo, &dest, &first).unwrap();

        // The worktree is at the requested (older) sha, detached.
        assert_eq!(git(&dest, &["rev-parse", "HEAD"]).unwrap(), first);
        assert_eq!(branch(&dest).unwrap(), None, "worktree must be detached");
        // The requested sha's tree, not the source's: second.md predates it.
        assert!(!dest.join("second.md").exists());

        // The source checkout is untouched: same branch, same HEAD, still
        // dirty with the same content.
        assert_eq!(branch(&repo).unwrap().as_deref(), Some("main"));
        assert_eq!(git(&repo, &["rev-parse", "HEAD"]).unwrap(), head_before);
        assert_eq!(
            std::fs::read_to_string(repo.join("README.md")).unwrap(),
            "dirty edit\n"
        );
    }

    /// Removal leaves no entry in `git worktree list` and deletes the
    /// directory.
    #[test]
    fn worktree_remove_leaves_no_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        init_repo(&repo);
        let sha = git(&repo, &["rev-parse", "HEAD"]).unwrap();
        let dest = tmp.path().join("wt");
        worktree_add(&repo, &dest, &sha).unwrap();

        worktree_remove(&repo, &dest).unwrap();

        assert!(!dest.exists());
        // Count registrations rather than searching the text: the random
        // tempdir name can itself contain "wt" (zipline#280 —
        // `/tmp/.tmpYtuywt/repo` failed a substring check). Porcelain output
        // has one `worktree <path>` line per registration. (`-z` would also
        // survive a newline in the path, but needs Git 2.36.) Only the main
        // worktree may remain.
        let listing = git(&repo, &["worktree", "list", "--porcelain"]).unwrap();
        let registered = listing
            .lines()
            .filter(|line| line.starts_with("worktree "))
            .count();
        assert_eq!(
            registered, 1,
            "stale worktree entry after removal: {listing}"
        );
    }

    /// Adding over a non-empty destination fails with a message naming the
    /// path, rather than half-overwriting whatever is there.
    #[test]
    fn worktree_add_refuses_nonempty_destination_naming_it() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        init_repo(&repo);
        let sha = git(&repo, &["rev-parse", "HEAD"]).unwrap();

        let dest = tmp.path().join("occupied");
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::write(dest.join("leftover.txt"), "previous run\n").unwrap();

        let error = worktree_add(&repo, &dest, &sha).unwrap_err().to_string();
        assert!(
            error.contains("occupied"),
            "path not named in error: {error}"
        );
        // The leftover file survives: nothing was half-overwritten.
        assert!(dest.join("leftover.txt").exists());
    }

    /// `worktree_remove` on a worktree with local modifications still removes
    /// it: build worktrees are throwaways, so removal is `--force`.
    #[test]
    fn worktree_remove_forces_out_a_dirty_worktree() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        init_repo(&repo);
        let sha = git(&repo, &["rev-parse", "HEAD"]).unwrap();
        let dest = tmp.path().join("wt");
        worktree_add(&repo, &dest, &sha).unwrap();
        std::fs::write(dest.join("build-artifact.o"), "junk\n").unwrap();

        worktree_remove(&repo, &dest).unwrap();
        assert!(!dest.exists());
    }

    /// The prune-first in `worktree_add` (zipline#71) never touches a *live*
    /// worktree: one whose directory exists — a developer's own, unrelated to
    /// any build — survives an unrelated `worktree_add` on the same
    /// repository, registration and content both. This pins the prune's blast
    /// radius at zero so it can never be widened by accident: prune drops
    /// only registrations whose directory is already gone.
    #[test]
    fn worktree_add_prune_leaves_live_worktrees_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        init_repo(&repo);
        let sha = git(&repo, &["rev-parse", "HEAD"]).unwrap();

        // The developer's own live worktree, with an uncommitted file in it.
        let live = tmp.path().join("live-wt");
        worktree_add(&repo, &live, &sha).unwrap();
        std::fs::write(live.join("scratch.txt"), "work in progress\n").unwrap();

        // An unrelated add — the prune inside it must not disturb `live`.
        let other = tmp.path().join("other-wt");
        worktree_add(&repo, &other, &sha).unwrap();

        // Still registered, directory intact, uncommitted content untouched.
        let listing = git(&repo, &["worktree", "list"]).unwrap();
        assert!(
            listing.contains("live-wt"),
            "live worktree pruned: {listing}"
        );
        assert_eq!(
            std::fs::read_to_string(live.join("scratch.txt")).unwrap(),
            "work in progress\n"
        );
    }
}
