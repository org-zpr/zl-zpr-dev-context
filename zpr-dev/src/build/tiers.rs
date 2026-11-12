//! The test tiers of spec-003 §6 (task B4): parsing the `--test` selection,
//! and the `unit` tier — each built repository's own `make test` in build
//! order, preceded in `zl-zpr-visaservice` by `make pregen ZPLC=<dist>/zplc`
//! so the visa service's policy fixtures are compiled by the set's own
//! compiler (the dynamic form of gate 3).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

use super::recipes;

/// The tiers this tool implements, in run order. Approved decisions on
/// zipline#61 (Q1) and zipline#62 (Q1): `default` — and no `--test` flag at
/// all — means every implemented tier, and since B5 the `netns` and `docker`
/// end-to-end tiers are implemented, joining the default selection with
/// probe-gated skip-with-reason.
const IMPLEMENTED: &[&str] = &["unit", "netns", "docker"];

/// Every tier spec-003 §6 names, in run order. Now identical to
/// `IMPLEMENTED` — B5 landed the last two — but kept separate so the
/// unknown-name error and any future tier land in the right place.
const KNOWN: &[&str] = &["unit", "netns", "docker"];

/// Which tiers a run executes, parsed from `--test` (spec-003 §7). Empty
/// means `--test none`: build only, no tier runs and none is reported.
///
/// Each selected tier also records whether it was *explicit* — named in a
/// `--test` list or covered by `--test all` — because the two end-to-end
/// tiers are probe-gated: a default-selected tier whose prerequisites are
/// missing is skipped with the reason, while an explicitly requested one is
/// an error (exit 1), never a silent skip (spec-003 §6; approved Q1 on
/// zipline#62).
#[derive(Debug, PartialEq)]
pub struct Selection {
    /// Tier names in run order, deduplicated, each with its explicitness.
    tiers: Vec<(&'static str, bool)>,
    /// The literal `--test` text, `default` for the absent flag: what the
    /// emitted manifest reports as `tests_requested`, so a reader can see
    /// a set was gated on `unit` alone (zipline#87).
    requested: String,
}

impl Selection {
    /// Parses the `--test` flag. `None` (flag absent) and `default` select
    /// every implemented tier, non-explicitly; `none` selects nothing;
    /// `all` selects every known tier, each explicitly; otherwise the value
    /// is a comma-separated list of tier names, each explicit. An unknown
    /// name and the special words mixed into a list are usage errors — they
    /// surface as exit 2 through `run`'s `Err` path.
    pub fn parse(flag: Option<&str>) -> Result<Selection> {
        // Absent and `default` are the same selection by definition
        // (approved Q1 on zipline#61): every implemented tier.
        let text = flag.unwrap_or("default");
        match text {
            "default" => {
                return Ok(Selection {
                    tiers: IMPLEMENTED.iter().map(|tier| (*tier, false)).collect(),
                    requested: text.to_string(),
                });
            }
            "none" => {
                return Ok(Selection {
                    tiers: Vec::new(),
                    requested: text.to_string(),
                });
            }
            // `all` asks for every known tier by name, so each is explicit:
            // an asked-for tier that cannot run is an error, never a silent
            // skip (spec-003 §6).
            "all" => {
                return Ok(Selection {
                    tiers: KNOWN.iter().map(|tier| (*tier, true)).collect(),
                    requested: text.to_string(),
                });
            }
            _ => {}
        }

        // A comma-separated list of tier names, each explicit. The special
        // whole-selection words are rejected inside a list: `unit,none` has
        // no coherent meaning.
        let mut tiers: Vec<(&'static str, bool)> = Vec::new();
        for name in text.split(',') {
            let name = name.trim();
            match KNOWN.iter().find(|known| **known == name) {
                Some(known) => {
                    if !tiers.iter().any(|(tier, _)| tier == known) {
                        tiers.push((known, true));
                    }
                }
                None => bail!(
                    "--test {name:?} is not a tier; valid values: none, default, all, \
                     or a comma-separated list of {}",
                    KNOWN.join(", ")
                ),
            }
        }
        // Run order is KNOWN's order, not the list's: `docker,unit` and
        // `unit,docker` are the same request, and unit failures should
        // surface before the slower end-to-end tiers run.
        tiers.sort_by_key(|(tier, _)| KNOWN.iter().position(|known| known == tier));
        Ok(Selection {
            tiers,
            requested: text.to_string(),
        })
    }

    /// The `--test` text as given (`default` when the flag was absent), for
    /// the emitted manifest's `tests_requested` (zipline#87).
    pub fn requested(&self) -> &str {
        &self.requested
    }

    /// True when no tier was selected (`--test none`).
    pub fn is_empty(&self) -> bool {
        self.tiers.is_empty()
    }

    /// True when `tier` was selected.
    pub fn contains(&self, tier: &str) -> bool {
        self.tiers.iter().any(|(name, _)| *name == tier)
    }

    /// True when `tier` was selected *explicitly* — named in a `--test`
    /// list or covered by `--test all` — which turns a failing prerequisite
    /// probe from a skip-with-reason into an error (spec-003 §6; approved
    /// Q1 on zipline#62). False for a tier that was not selected at all.
    pub fn is_explicit(&self, tier: &str) -> bool {
        self.tiers
            .iter()
            .any(|(name, explicit)| *name == tier && *explicit)
    }
}

/// Every tier spec-003 §6 names, in run order — the set the emitted
/// manifest's coverage summary must account for (zipline#87).
pub fn known() -> &'static [&'static str] {
    KNOWN
}

/// A tier's human label for the manifest's `notes`: the reader should see
/// "integration tests did NOT run", not a bare tier name (zipline#87).
pub fn label(tier: &str) -> &'static str {
    match tier {
        "unit" => "unit tests",
        "netns" => "netns integration tests",
        "docker" => "docker end-to-end tests",
        // KNOWN is the only source of tier names; a new tier must add its
        // label here, and the test `every_known_tier_has_a_label` says so.
        _ => "tests",
    }
}

// ---------------------------------------------------------------------------
// Prerequisite probes for the end-to-end tiers (issue62 step 2)
// ---------------------------------------------------------------------------

/// What the host offers the end-to-end tiers, gathered once per run by
/// [`Probes::gather`] and consumed by the pure gate functions below —
/// separated so the gating logic is testable with injected results.
#[derive(Debug)]
pub struct Probes {
    /// The target OS is Linux (the netns scripts create network namespaces).
    pub linux: bool,
    /// `unshare -nr true` succeeded: this user may create an unprivileged
    /// user + network namespace, which is all the netns scripts need since
    /// they re-exec themselves under `unshare -nr` (zipline#190/#191/#192).
    /// On Ubuntu 24.04+ AppArmor blocks this unless
    /// `kernel.apparmor_restrict_unprivileged_userns=0`. The probe is
    /// read-only: the namespaces exist only for the lifetime of `true`.
    pub unprivileged_userns: bool,
    /// Where `valkey-server` is: `$VALKEY_SERVER_BIN` when set (the same
    /// override the scripts honour), otherwise the first hit on `PATH`.
    pub valkey_server: Option<PathBuf>,
    /// `python3` is on `PATH`.
    pub python3: bool,
    /// `docker` is on `PATH`.
    pub docker: bool,
    /// `docker compose version` succeeded (the compose v2 plugin exists).
    pub docker_compose: bool,
    /// `docker info` succeeded: the daemon is reachable, not merely the
    /// client installed (zipline#92). Gathered only when `docker` is on
    /// PATH, like `docker_compose` — a client-less host cannot have a
    /// reachable daemon worth probing.
    pub docker_daemon: bool,
}

impl Probes {
    /// Probes the live host. Every check is read-only, so gathering is legal
    /// under `--dry-run` (spec-003 §7.2).
    pub fn gather() -> Probes {
        let valkey_server = std::env::var_os("VALKEY_SERVER_BIN")
            .map(PathBuf::from)
            .filter(|path| path.is_file())
            .or_else(|| find_on_path("valkey-server"));
        let docker = find_on_path("docker").is_some();
        Probes {
            linux: cfg!(target_os = "linux"),
            // The same invocation the scripts re-exec under, so a pass here
            // means the scripts' own `unshare -nr` will pass too.
            unprivileged_userns: command_succeeds("unshare", &["-nr", "true"]),
            valkey_server,
            python3: find_on_path("python3").is_some(),
            docker,
            // Only worth asking when docker itself exists.
            docker_compose: docker && command_succeeds("docker", &["compose", "version"]),
            // Same guard: `docker info` answers only when the daemon is up,
            // and asking without a client is a spawn failure, not a probe.
            // Read-only, so legal under --dry-run (spec-003 §7.2).
            docker_daemon: docker && command_succeeds("docker", &["info"]),
        }
    }
}

/// True when running `program args` exits 0, treating a spawn failure as a
/// failed probe rather than an error — a missing binary is exactly what the
/// probe exists to detect.
fn command_succeeds(program: &str, args: &[&str]) -> bool {
    std::process::Command::new(program)
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// The first `PATH` entry holding an executable file named `program`.
fn find_on_path(program: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    find_in_path_value(&path, program)
}

/// [`find_on_path`] against an explicit `PATH` value — separated so tests can
/// probe a constructed PATH without mutating the process environment. A
/// candidate must be a regular file AND executable: a non-executable file of
/// the right name (a stray download, a sources checkout) must not satisfy a
/// tier gate, or the tier runs and fails mid-script instead of skipping with
/// the reason — the search continues to later PATH entries instead.
fn find_in_path_value(path: &std::ffi::OsStr, program: &str) -> Option<PathBuf> {
    std::env::split_paths(path)
        .map(|dir| dir.join(program))
        .find(|candidate| is_executable_file(candidate))
}

/// True when `path` is a regular file with any execute bit set — what "on
/// PATH" means to a shell about to run it.
fn is_executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::metadata(path)
        .map(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

/// A tier's gate: run, or skip with the reason. What a skip *means* depends
/// on explicitness — see [`check_gate`].
#[derive(Debug)]
pub enum TierGate {
    /// Every prerequisite is present.
    Run,
    /// A prerequisite is missing; the text names each missing one.
    Skip(String),
}

/// Gates the netns tier: a thin match over [`select_netns_runner`], so the
/// selection logic has one source of truth (zipline#92). `Host` and
/// `Container` both run — the container route executes through `make
/// docker-test` since zipline#93; its remaining prerequisite, the worktree's
/// Makefile floor, is checked against the actual worktree at execution time
/// (`docker_runner_floor`), because no worktree exists at gate time. No
/// route at all skips with the selector's two-route reason. Pure — probes
/// are injected.
pub fn netns_gate(probes: &Probes) -> TierGate {
    match select_netns_runner(probes) {
        Ok(_) => TierGate::Run,
        Err(reason) => TierGate::Skip(reason),
    }
}

// ---------------------------------------------------------------------------
// The netns runner selection (zipline#92 step 2)
// ---------------------------------------------------------------------------

/// Which route the netns tier takes (zipline#92): the host
/// when its own prerequisites hold, the privileged container of zipline#84's
/// `make docker-test` when they do not but a Docker daemon answers. The
/// no-route case is [`select_netns_runner`]'s `Err`, naming both gaps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetnsRunner {
    /// The host route: the scripts run directly, each in its own
    /// unprivileged user + network namespace (manifest `privilege: userns`).
    ///
    /// `docker_fallback` records whether the container route was usable
    /// when the host was probed: `Ok(())` when a Docker daemon answered,
    /// otherwise `Err` with the docker gap (`docker not found` / `docker
    /// daemon not reachable`). The host route is chosen from live probes
    /// before any worktree exists, so [`apply_runner_floors`] can still
    /// find that the worktree's scripts predate the unprivileged harness;
    /// this is what it falls back to then (review of
    /// zl-zpr-dev-context#65).
    Host { docker_fallback: Result<(), String> },
    /// The container fallback. `host_reason` is the host route's own
    /// missing-prerequisite text — the same words the host skip would have
    /// used — because the container can be selected for unusable user
    /// namespaces *or* a missing `valkey-server` / `python3`, and
    /// zipline#93's tier header and coverage note must quote the real cause,
    /// never a fixed one (operator amendment on zipline#92).
    Container { host_reason: String },
}

/// The host route's missing user-namespace prerequisite, worded so the
/// remedy is discoverable from the failure: on Ubuntu 24.04+ it is the
/// AppArmor sysctl that blocks unprivileged user namespaces.
pub const MISSING_USERNS: &str = "unprivileged user namespaces (unshare -nr failed; \
     on Ubuntu 24.04+ set kernel.apparmor_restrict_unprivileged_userns=0)";

/// The host route's own missing-prerequisite text: `missing: <gaps>`, or
/// `None` when the host route can run. Linux is deliberately not listed
/// here: the selector refuses a non-Linux host before routing, because
/// neither route creates network namespaces elsewhere.
fn host_route_reason(probes: &Probes) -> Option<String> {
    let mut missing: Vec<&str> = Vec::new();
    if !probes.unprivileged_userns {
        missing.push(MISSING_USERNS);
    }
    if probes.valkey_server.is_none() {
        missing.push("valkey-server");
    }
    if !probes.python3 {
        missing.push("python3");
    }
    if missing.is_empty() {
        return None;
    }
    Some(format!("missing: {}", missing.join(", ")))
}

/// Decides which route the netns tier takes (zipline#92; the rationale is
/// spec-003 §10.2): the host route wins whenever it can run;
/// otherwise the container is selected when a Docker daemon is reachable —
/// a `docker` client on PATH with no daemon is not a usable fallback; and
/// when neither route works the `Err` names both routes' gaps, e.g.
/// `missing: valkey-server; docker fallback unavailable: docker daemon not
/// reachable`. Pure — probes are injected — so every selection rule is
/// testable without a live docker or `unshare`.
pub fn select_netns_runner(probes: &Probes) -> Result<NetnsRunner, String> {
    if !probes.linux {
        // The netns scripts create network namespaces; neither the host
        // route nor the docker fallback offers those off Linux.
        return Err(
            "missing: linux (neither the host route nor the docker fallback \
             runs the netns scripts elsewhere)"
                .to_string(),
        );
    }
    // Why the container route is unusable, if it is. The wording matches
    // the docker tier's own gate.
    let docker_gap = if !probes.docker {
        Some("docker not found")
    } else if !probes.docker_daemon {
        Some("docker daemon not reachable")
    } else {
        None
    };
    let Some(host_reason) = host_route_reason(probes) else {
        // The host route can run, so it wins outright — the fallback is a
        // fallback, never a preference (approved decision: automatic, no
        // flag). Whether the fallback is usable is still recorded, for the
        // execution-time host floor.
        return Ok(NetnsRunner::Host {
            docker_fallback: docker_gap.map_or(Ok(()), |gap| Err(gap.to_string())),
        });
    };
    match docker_gap {
        None => Ok(NetnsRunner::Container { host_reason }),
        // Neither route: the reason names both gaps so the operator can fix
        // either one.
        Some(gap) => Err(no_route_reason(&host_reason, gap)),
    }
}

/// The two-route skip reason: the host route's gap and the docker
/// route's, so the operator can fix either one. Shared by the probe-time
/// selector and the execution-time floors so the two never word it
/// differently.
fn no_route_reason(host_reason: &str, docker_gap: &str) -> String {
    format!("{host_reason}; docker fallback unavailable: {docker_gap}")
}

/// How the netns tier was privileged, recorded in the emitted manifest
/// (zipline#192) so the audit record says which route actually ran the
/// scripts. Serializes lowercase: `userns` / `container`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Privilege {
    /// The host route: each script ran as the invoking user inside its own
    /// unprivileged user + network namespace (`unshare -nr`).
    Userns,
    /// The docker fallback (zipline#93): the scripts ran as root inside the
    /// privileged container of `make docker-test`.
    Container,
}

impl NetnsRunner {
    /// The manifest's [`Privilege`] for a tier that ran on this route.
    pub fn privilege(&self) -> Privilege {
        match self {
            NetnsRunner::Host { .. } => Privilege::Userns,
            NetnsRunner::Container { .. } => Privilege::Container,
        }
    }
}

/// The dry-run report's planned-tier annotation for netns (zipline#92
/// step 5): the selection, stated without overstating — `prerequisites
/// present` for the host route, `would run in docker` with the host route's
/// real gap for the fallback, or the no-route reason. Pure, like the gates,
/// so it is testable with injected probes.
pub fn netns_dry_run_text(probes: &Probes) -> String {
    match select_netns_runner(probes) {
        Ok(NetnsRunner::Host { .. }) => "prerequisites present".to_string(),
        Ok(NetnsRunner::Container { host_reason }) => {
            format!("would run in docker (host route unavailable: {host_reason})")
        }
        Err(reason) => reason,
    }
}

/// The netns tier's stdout header (zipline#93 step 5): the host route keeps
/// the plain `netns tier:`; the container route names the docker fallback
/// and quotes the host route's real failure, so an operator watching the
/// run sees the cause without opening the manifest.
pub fn netns_tier_header(runner: &NetnsRunner) -> String {
    match runner {
        NetnsRunner::Host { .. } => "netns tier:".to_string(),
        NetnsRunner::Container { host_reason } => {
            format!("netns tier (docker fallback; host route unavailable: {host_reason}):")
        }
    }
}

/// The container route's floor check (zipline#93, contract 1): the
/// `zl-zpr-core` worktree must carry `integration-test/Makefile` defining
/// `FORWARD_ENV` — added by `c163628`, the commit that forwards the caller's
/// `*_BIN` overrides into the container. An older worktree would run the
/// scripts against the image's own defaults and silently test the wrong
/// binaries, so the route is refused with a reason naming the worktree's sha
/// and the floor commit. Pure over the filesystem: no git, no docker.
pub fn docker_runner_floor(core_worktree: &Path, head_sha: &str) -> Result<(), String> {
    let makefile = core_worktree.join("integration-test").join("Makefile");
    let defines_forward_env = std::fs::read_to_string(&makefile)
        .map(|text| text.contains("FORWARD_ENV"))
        .unwrap_or(false);
    if defines_forward_env {
        return Ok(());
    }
    Err(format!(
        "zl-zpr-core @ {head_sha} predates the Docker runner \
         (integration-test/Makefile with FORWARD_ENV, zl-zpr-core c163628)"
    ))
}

/// The first `zl-zpr-core` commit whose integration scripts can run on the
/// host route: every script re-execs itself under `unshare -nr`, needs no
/// sudo, and scopes its `pkill`/`pgrep` to its own user namespace, so they
/// run unprivileged and in parallel (zipline#191, merged as #83).
pub const HOST_RUNNER_FLOOR_COMMIT: &str = "3ad48ed";

/// The host route's floor check, the counterpart of
/// [`docker_runner_floor`] (review of zl-zpr-dev-context#65): the
/// `zl-zpr-core` worktree's `integration-test/lib/common_funcs.sh` must
/// define `pkill_own`. That function landed in
/// [`HOST_RUNNER_FLOOR_COMMIT`], the same commit that converted every
/// script and removed the transitional sudo path, so its presence is the
/// marker that the whole harness is unprivileged. An older worktree — a
/// build set pinning core from before zipline#191 — still calls `sudo` and
/// shares fixed `/run/netns` names, so it must not run on the host, let
/// alone four at a time. Pure over the filesystem: no git.
pub fn host_runner_floor(core_worktree: &Path, head_sha: &str) -> Result<(), String> {
    let common_funcs = core_worktree
        .join("integration-test")
        .join("lib")
        .join("common_funcs.sh");
    let unprivileged_harness = std::fs::read_to_string(&common_funcs)
        .map(|text| text.contains("function pkill_own"))
        .unwrap_or(false);
    if unprivileged_harness {
        return Ok(());
    }
    Err(format!(
        "zl-zpr-core @ {head_sha} predates the unprivileged netns harness \
         (integration-test/lib/common_funcs.sh with pkill_own, zl-zpr-core \
         {HOST_RUNNER_FLOOR_COMMIT})"
    ))
}

/// Applies the execution-time floors to the probe-time route selection,
/// once the `zl-zpr-core` worktree exists, and returns the route that
/// actually runs:
///
/// - `Host` on a worktree that passes [`host_runner_floor`] stays `Host`.
/// - `Host` on an older worktree falls back to `Container`, with the host
///   floor's failure as `host_reason`, when a Docker daemon answered at
///   probe time and the worktree also passes [`docker_runner_floor`];
///   otherwise it is the two-route `Err`.
/// - `Container` must pass [`docker_runner_floor`]; its failure is returned
///   as is.
///
/// The caller turns an `Err` into a skip or, for an explicit request, an
/// error, as for any gate (spec-003 §6). Pure over the filesystem.
pub fn apply_runner_floors(
    runner: &NetnsRunner,
    core_worktree: &Path,
    head_sha: &str,
) -> Result<NetnsRunner, String> {
    match runner {
        NetnsRunner::Host { docker_fallback } => {
            let Err(host_reason) = host_runner_floor(core_worktree, head_sha) else {
                return Ok(runner.clone());
            };
            if let Err(docker_gap) = docker_fallback {
                return Err(no_route_reason(&host_reason, docker_gap));
            }
            docker_runner_floor(core_worktree, head_sha)
                .map_err(|docker_gap| no_route_reason(&host_reason, &docker_gap))?;
            Ok(NetnsRunner::Container { host_reason })
        }
        NetnsRunner::Container { .. } => {
            docker_runner_floor(core_worktree, head_sha)?;
            Ok(runner.clone())
        }
    }
}

/// Gates the docker tier: `docker`, the compose v2 plugin, and a reachable
/// daemon (zipline#92 — a client whose daemon is down failed the tier
/// mid-deploy before; the probe was free once the netns fallback needed it).
/// The missing-docker wording matches the issue's acceptance text
/// (`docker not found`).
pub fn docker_gate(probes: &Probes) -> TierGate {
    if !probes.docker {
        return TierGate::Skip("docker not found".to_string());
    }
    if !probes.docker_compose {
        return TierGate::Skip("docker compose not found".to_string());
    }
    if !probes.docker_daemon {
        return TierGate::Skip("docker daemon not reachable".to_string());
    }
    TierGate::Run
}

/// Applies a gate under the approved Q1 policy (zipline#62): a failing probe
/// on a default-selected tier is a skip carrying its reason (`Ok(Some(..))`),
/// the same failure on an explicitly requested tier is an error carrying the
/// same text (exit 1 through `run`'s gate-style failure path), and a passing
/// gate runs (`Ok(None)`) either way. Never a silent skip.
pub fn check_gate(tier: &str, gate: TierGate, explicit: bool) -> Result<Option<String>> {
    match gate {
        TierGate::Run => Ok(None),
        TierGate::Skip(reason) if explicit => {
            bail!("--test {tier} was requested but cannot run: {reason}")
        }
        TierGate::Skip(reason) => Ok(Some(reason)),
    }
}

// ---------------------------------------------------------------------------
// The unit tier (task B4 steps 1-2)
// ---------------------------------------------------------------------------

/// One command the unit tier runs, with the repository's worktree as the
/// working directory. `name` labels the log file and attributes a failure —
/// a failing `pregen` must be named `pregen`, because a `zplc` that cannot
/// compile the visa service's own fixtures is exactly the incompatibility
/// this tier exists to catch.
#[derive(Debug)]
pub struct TierStep {
    pub name: &'static str,
    pub program: &'static str,
    /// Owned, unlike `recipes::Step`: the `ZPLC=<dist>/zplc` argument is
    /// computed per run.
    pub args: Vec<String>,
}

/// One repository's part of the unit-tier plan: commands to run, or a skip
/// with its reason. A repository with nothing to test is reported `skipped`,
/// never silently omitted (issue constraint; approved Q2 on zipline#61).
#[derive(Debug)]
pub enum RepoPlan {
    /// Run `steps` in order in `worktree`; the first failure fails the
    /// repository and the sweep moves on to the next one.
    Run {
        repo: String,
        worktree: PathBuf,
        steps: Vec<TierStep>,
    },
    /// Nothing to run here, and the reason says why.
    Skip { repo: String, reason: String },
}

/// The unit tier's outcome: overall pass/fail and the per-repository
/// breakdown that lands in the emitted manifest's `tiers.unit.repos`
/// (approved Q3 on zipline#61).
#[derive(Debug)]
pub struct TierOutcome {
    /// False when any repository failed. Skips do not fail the tier.
    pub passed: bool,
    /// Repository → `passed`, `failed: ...` (naming the failing step and its
    /// log), or `skipped: ...` (with the reason).
    pub repos: BTreeMap<String, String>,
}

/// Builds the unit tier's plan from the worktrees a build produced: `make
/// test` in each built repository **in build order** (the recipe table's
/// order, whatever order the worktrees arrived in), preceded in
/// `zl-zpr-visaservice` by `make pregen ZPLC=<dist>/zplc` so the policy
/// fixtures its tests load are compiled by the set's own compiler. A
/// repository whose recipe builds nothing has no unit suite to run and is
/// planned as a skip (approved Q2: `zl-zpr-demo`; the docker tier covers it).
/// Pure planning: nothing here executes a command.
pub fn unit_plan(
    recipes: &[recipes::Recipe],
    worktrees: &[(&recipes::Recipe, PathBuf)],
    dist: &Path,
) -> Vec<RepoPlan> {
    let mut plans: Vec<RepoPlan> = Vec::new();
    // Iterating the recipe table, not the input, is what makes the plan
    // build-ordered: the table's order is BUILD_ORDER, asserted by test.
    for recipe in recipes {
        let Some((_, worktree)) = worktrees.iter().find(|(wt, _)| wt.repo == recipe.repo) else {
            continue; // not in this set: nothing was built, nothing to test
        };

        // A recipe with no build steps builds nothing, so there is no unit
        // suite to run against the set (approved Q2 on zipline#61).
        if recipe.steps.is_empty() {
            plans.push(RepoPlan::Skip {
                repo: recipe.repo.to_string(),
                reason: "no unit tests (docker tier covers it)".to_string(),
            });
            continue;
        }

        let mut steps: Vec<TierStep> = Vec::new();
        if recipe.repo == "zl-zpr-visaservice" {
            // `pregen` recompiles the visa service's policy fixtures with
            // the set's own zplc before `make test` loads them — the dynamic
            // form of gate 3, and the reason this tier exists (spec-003 §6).
            steps.push(TierStep {
                name: "pregen",
                program: "make",
                args: vec![
                    "pregen".to_string(),
                    format!("ZPLC={}", dist.join("zplc").display()),
                ],
            });
        }
        steps.push(TierStep {
            name: "test",
            program: "make",
            args: vec!["test".to_string()],
        });
        plans.push(RepoPlan::Run {
            repo: recipe.repo.to_string(),
            worktree: worktree.clone(),
            steps,
        });
    }
    plans
}

/// Runs a unit-tier plan: each repository's steps in order, logging through
/// the same shape as the build steps (`logs/<repo>-<step>.log`, last 40
/// lines echoed on failure unless `quiet`). A failing step fails its
/// repository naming the step, and the sweep **keeps going** so one run
/// reports every broken repository rather than only the first (issue
/// step 2). Prints one line per repository unless `quiet`.
pub fn run_unit(plans: &[RepoPlan], logs: &Path, quiet: bool) -> TierOutcome {
    let mut outcome = TierOutcome {
        passed: true,
        repos: BTreeMap::new(),
    };
    for plan in plans {
        match plan {
            RepoPlan::Skip { repo, reason } => {
                // Visible, never omitted: a tier entry that did not run
                // always says why (issue constraint; approved Q2).
                if !quiet {
                    println!("{repo}: skipped ({reason})");
                }
                outcome
                    .repos
                    .insert(repo.clone(), format!("skipped: {reason}"));
            }
            RepoPlan::Run {
                repo,
                worktree,
                steps,
            } => {
                // The first failing step fails the repository — a failed
                // `pregen` must not let `make test` run against stale
                // fixtures — but the sweep continues to the next repository
                // so one run reports every broken one (issue step 2).
                let failure = steps.iter().find_map(|step| {
                    recipes::run_command(
                        repo,
                        step.name,
                        step.program,
                        &step.args,
                        worktree,
                        logs,
                        quiet,
                    )
                    .err()
                    .map(|error| (step.name, error))
                });
                match failure {
                    None => {
                        if !quiet {
                            println!("{repo}: passed");
                        }
                        outcome.repos.insert(repo.clone(), "passed".to_string());
                    }
                    Some((step, error)) => {
                        // The entry names the failing step and carries the
                        // run_command error, which names the log path.
                        if !quiet {
                            println!("{repo}: FAILED at step `{step}`");
                        }
                        outcome.passed = false;
                        outcome
                            .repos
                            .insert(repo.clone(), format!("failed at `{step}`: {error}"));
                    }
                }
            }
        }
    }
    outcome
}

// ---------------------------------------------------------------------------
// The netns tier (issue62 step 3)
// ---------------------------------------------------------------------------

/// The twelve integration scripts the netns tier runs, in order. An explicit
/// list, never a glob: `integration-test/unused_or_outdated/` stays out, and
/// adding a script to the set's gate is a reviewed change (zipline#62).
/// The list is guarded against drift by [`netns_script_drift`] (zipline#103):
/// a top-level `*-test.sh` in the worktree that is neither here nor in
/// [`NETNS_EXCLUDED`] fails planning, so a new script can never be silently
/// not-run.
const NETNS_SCRIPTS: &[&str] = &[
    "one-node-test.sh",
    "one-node-v6-test.sh",
    "one-node-oidc-test.sh",
    "capture-test.sh",
    "oidc-file-interplay-test.sh",
    "fake-idp-smoke-test.sh",
    "a2a-pubkey-test.sh",
    "attr-query-test.sh",
    "one-node-oidc-renewal-test.sh",
    "one-node-policy-reauth-test.sh",
    "node-restart-test.sh",
    // The Zode link lifecycle: a relay, a Zode with an initial link to it,
    // and a Zode outside the relay's topology (zl-zipline#250).
    "zode-link-test.sh",
];

/// Scripts in [`NETNS_SCRIPTS`] that only some `zl-zpr-core` revisions carry:
/// a feature branch added them, and a build set may pin a core from before
/// that (the default build set pins `zipline`). Planned only when the
/// worktree has them; otherwise recorded as skipped with a reason, never
/// silently dropped. Every other listed script must exist.
const NETNS_SCRIPTS_IF_PRESENT: &[&str] = &[
    // feature/zodes only, until it merges into zipline (zl-zipline#250).
    "zode-link-test.sh",
];

/// Top-level `integration-test/*-test.sh` scripts the netns tier
/// deliberately does NOT run, each with the reason (zipline#103). A script
/// belongs here only after a reviewed decision — the drift guard
/// ([`netns_script_drift`]) treats anything in neither list as an error, so
/// "not yet gated" is never a silent state.
const NETNS_EXCLUDED: &[(&str, &str)] = &[];

/// The drift guard (zipline#103): every top-level `*-test.sh` in the
/// worktree's `integration-test/` must be either gated ([`NETNS_SCRIPTS`])
/// or deliberately excluded with a reason ([`NETNS_EXCLUDED`]). Top level
/// only (approved Q2 on zipline#103): `lib/` and `unused_or_outdated/` are
/// invisible, matching the Makefile's own `$(wildcard *-test.sh)` glob. A
/// missing `integration-test/` passes — there is nothing to judge, and the
/// tier run itself reports a broken worktree.
///
/// The guard fails closed (review findings on zl-zpr-dev-context#35): a
/// directory entry that cannot be inspected or a script name that is not
/// valid UTF-8 is an error, never silently absent — the Makefile's byte-wise
/// glob would still run such a script, so dropping it here would report
/// complete coverage falsely. And every exclusion must carry a nonempty
/// reason: the whole table is validated up front, so an unauditable entry
/// like `("x-test.sh", "")` fails planning even before any script matches.
fn netns_script_drift(integration: &Path, gated: &[&str], excluded: &[(&str, &str)]) -> Result<()> {
    // The exclusion table must be auditable before it is consulted: an
    // entry without a reason could exempt a script with no recorded
    // justification, which is exactly what the guard exists to prevent.
    for (name, reason) in excluded {
        if reason.trim().is_empty() {
            bail!(
                "NETNS_EXCLUDED entry {name:?} has no reason: every exclusion \
                 must record why the script deliberately does not run \
                 (zipline#103)"
            );
        }
    }
    if !integration.is_dir() {
        return Ok(());
    }
    // Every top-level `*-test.sh`, sorted so the error is deterministic.
    // Per-entry errors and non-UTF-8 names fail the scan rather than being
    // skipped: a name the guard cannot judge is not a name it may ignore.
    let mut strays: Vec<String> = Vec::new();
    let entries = std::fs::read_dir(integration)
        .map_err(|e| anyhow::anyhow!("cannot read {}: {e}", integration.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| {
            anyhow::anyhow!("cannot read an entry in {}: {e}", integration.display())
        })?;
        if !entry.path().is_file() {
            continue;
        }
        let name = entry.file_name().into_string().map_err(|raw| {
            anyhow::anyhow!(
                "non-UTF-8 file name {raw:?} in {}: the netns drift guard \
                 cannot compare it against NETNS_SCRIPTS/NETNS_EXCLUDED — \
                 rename it (zipline#103)",
                integration.display()
            )
        })?;
        if !name.ends_with("-test.sh") {
            continue;
        }
        if gated.contains(&name.as_str()) || excluded.iter().any(|(excl, _)| *excl == name) {
            continue;
        }
        strays.push(name);
    }
    strays.sort();
    if strays.is_empty() {
        return Ok(());
    }
    bail!(
        "netns script list has drifted: {} in {} is neither gated nor \
         deliberately excluded — add it to NETNS_SCRIPTS to run it, or to \
         NETNS_EXCLUDED with the reason it must not run (zipline#103)",
        strays.join(", "),
        integration.display()
    );
}

/// A build that must succeed before its script runs — the worktree-local
/// `enable-security-testing` build of `ph` for `a2a-pubkey-test.sh`. Kept
/// separate from the script so its failure fails that one script while the
/// rest of the tier still runs (zipline#62).
#[derive(Debug)]
pub struct PrepStep {
    /// Labels the log file (`logs/netns-<name>.log`) and the failure entry.
    pub name: &'static str,
    pub program: &'static str,
    pub args: Vec<String>,
    /// Working directory — the `zl-zpr-core` worktree, not `integration-test/`.
    pub dir: PathBuf,
}

/// One planned script: its name, the command that runs it, the environment
/// overrides it runs under, the inherited variables to strip from the
/// child, and an optional prep build. On the host route the command is the
/// script itself; on the container route it is `make docker-test` in the
/// worktree's `integration-test/` (zipline#93).
#[derive(Debug)]
pub struct NetnsScript {
    pub script: &'static str,
    /// What to execute: the script's own path (host) or `make` (container).
    pub program: String,
    /// Arguments to `program`: empty on the host route; the
    /// `-C .. docker-test TEST=.. WORKSPACE=..` invocation on the container
    /// route (contract 1).
    pub args: Vec<String>,
    /// `KEY=value` pairs set on the child only — never the tool's own env.
    pub env: Vec<(String, String)>,
    /// Inherited variables removed from the child's environment before it
    /// runs (zipline#93): `run_env_command` inherits this process's
    /// environment and only adds `env`, so an operator's exported
    /// `VALKEY_SERVER_BIN` would otherwise reach the container through the
    /// Makefile's `FORWARD_ENV`. Empty on the host route — its behaviour is
    /// byte-identical to before the field existed.
    pub env_remove: Vec<&'static str>,
    pub prep: Option<PrepStep>,
}

/// The netns tier's plan: where the scripts live and what to run. Pure
/// planning — nothing here executes a command.
#[derive(Debug)]
pub struct NetnsPlan {
    /// `<zl-zpr-core worktree>/integration-test`.
    pub dir: PathBuf,
    /// How many scripts may run at once, on either route: [`NETNS_JOBS`]
    /// unless `--jobs` overrides it.
    pub jobs: usize,
    pub scripts: Vec<NetnsScript>,
    /// Listed scripts this worktree does not carry
    /// ([`NETNS_SCRIPTS_IF_PRESENT`]), with the reason; recorded in the
    /// tier's outcome as skipped.
    pub skipped: Vec<(&'static str, String)>,
}

/// Concurrent netns scripts, on either route. On the host route each script
/// re-execs itself under `unshare -nr` and pins anonymous network
/// namespaces, so its namespaces, links and `pkill`/`pgrep` scope are
/// private to its own user namespace (zipline#190/#191). On the container
/// route each `make docker-test` is its own `docker run`, private in the
/// same way. Either way the scripts cannot collide.
// The default width. The scripts wait on fixed `sleep`s, so too many at
// once on a small machine risks timing flakes; `--jobs` overrides it per
// run where 4 proves wrong for a host (zipline#144).
pub const NETNS_JOBS: usize = 4;

/// Builds the netns plan against the `zl-zpr-core` worktree and `dist/`:
/// the twelve blessed scripts in order, each with the `*_BIN` overrides the
/// scripts already honour pointed at `dist/` — nothing is copied into
/// `integration-test/`. `a2a-pubkey-test.sh` alone runs the worktree-local
/// `enable-security-testing` `ph` (a debug build that must never reach
/// `dist/` — see [`dist_ph_is_clean`]); its prep build runs on the host on
/// both routes, and the debug binary is under the container's mount either
/// way (Finding 3). `verbose` exports `ZPR_TEST_VERBOSE=1`; `DEBUG_TARGETS`
/// is left at the scripts' default.
///
/// The `runner` decides the route (zipline#93, contract 1):
/// - `Host`: each script runs directly from `integration-test/` with
///   `VALKEY_SERVER_BIN` pointing at the host's `valkey` — byte-identical to
///   the pre-#93 plan.
/// - `Container`: each script becomes `make -C <core>/integration-test
///   docker-test TEST=<script> WORKSPACE=<build_dir>` in the worktree, and
///   `VALKEY_SERVER_BIN` is neither set nor inherited (`env_remove`): the
///   image ships its own valkey, a host path would not resolve inside the
///   container, and the Makefile's `FORWARD_ENV` would forward an operator's
///   exported value.
///
/// `jobs` is the operator's `--jobs` override for the tier's width on
/// either route (zipline#144, zipline#192); `None` keeps [`NETNS_JOBS`].
pub fn netns_plan(
    core_worktree: &Path,
    dist: &Path,
    valkey: &Path,
    verbose: bool,
    runner: &NetnsRunner,
    build_dir: &Path,
    jobs: Option<std::num::NonZeroUsize>,
) -> Result<NetnsPlan> {
    let display = |path: PathBuf| path.display().to_string();
    let container = matches!(runner, NetnsRunner::Container { .. });
    let integration = core_worktree.join("integration-test");
    // The drift guard (zipline#103): judge the worktree under test before
    // planning anything — an unlisted top-level script is a plan error,
    // never a silently smaller tier.
    netns_script_drift(&integration, NETNS_SCRIPTS, NETNS_EXCLUDED)?;
    // A feature-branch script the worktree lacks is skipped, not run. A
    // missing integration-test/ judges nothing, as in the drift guard.
    let lacks = |script: &str| {
        NETNS_SCRIPTS_IF_PRESENT.contains(&script)
            && integration.is_dir()
            && !integration.join(script).exists()
    };
    let skipped: Vec<(&'static str, String)> = NETNS_SCRIPTS
        .iter()
        .copied()
        .filter(|script| lacks(script))
        .map(|script| {
            (
                script,
                "not in this zl-zpr-core revision (a feature-branch script)".to_string(),
            )
        })
        .collect();
    let scripts = NETNS_SCRIPTS
        .iter()
        .filter(|script| !lacks(script))
        .map(|script| {
            let a2a = *script == "a2a-pubkey-test.sh";
            // The security-testing ph is a debug-profile build in the
            // worktree's own target/, pointed at for this one script only.
            let ph_bin = if a2a {
                display(core_worktree.join("target/debug/ph"))
            } else {
                display(dist.join("ph"))
            };
            let mut env: Vec<(String, String)> = vec![
                ("PH_BIN".to_string(), ph_bin),
                ("PH_DEBUG_BIN".to_string(), display(dist.join("ph-cli"))),
                ("VS_BIN".to_string(), display(dist.join("vs"))),
                ("VS_ADMIN_BIN".to_string(), display(dist.join("vs-admin"))),
                // attr-query-test.sh needs the reference attribute server
                // and (optionally, for its pre-check) zpdump; a build
                // worktree carries none of the symlinks the scripts default
                // to, so point both at the staged dist/ (zipline#103). The
                // other scripts ignore them.
                (
                    "ZPR_ATTR_SERVER_BIN".to_string(),
                    display(dist.join("zpr-attr-server")),
                ),
                ("ZPDUMP_BIN".to_string(), display(dist.join("zpdump"))),
            ];
            if !container {
                // Host route only: the container's image ships its own
                // valkey and a host path would not exist inside it.
                env.push((
                    "VALKEY_SERVER_BIN".to_string(),
                    valkey.display().to_string(),
                ));
            }
            if verbose {
                env.push(("ZPR_TEST_VERBOSE".to_string(), "1".to_string()));
            }
            let (program, args, env_remove) = if container {
                (
                    "make".to_string(),
                    vec![
                        "-C".to_string(),
                        integration.display().to_string(),
                        "docker-test".to_string(),
                        format!("TEST={script}"),
                        format!("WORKSPACE={}", build_dir.display()),
                    ],
                    vec!["VALKEY_SERVER_BIN"],
                )
            } else {
                (display(integration.join(script)), vec![], vec![])
            };
            NetnsScript {
                script,
                program,
                args,
                env,
                env_remove,
                prep: a2a.then(|| PrepStep {
                    name: "security-ph",
                    program: "cargo",
                    args: ["build", "-p", "ph", "--features", "enable-security-testing"]
                        .iter()
                        .map(|arg| arg.to_string())
                        .collect(),
                    dir: core_worktree.to_path_buf(),
                }),
            }
        })
        .collect();
    Ok(NetnsPlan {
        // The container invocation runs `make` from the worktree (`-C`
        // names the Makefile's directory); the host route keeps running
        // the scripts from integration-test/ as before.
        dir: if container {
            core_worktree.to_path_buf()
        } else {
            integration
        },
        jobs: jobs.map(std::num::NonZeroUsize::get).unwrap_or(NETNS_JOBS),
        scripts,
        skipped,
    })
}

/// Runs a netns plan, logging each script as `logs/netns-<script>.log` in
/// `run_unit`'s shape. Prep builds (a2a's security-testing `ph`) run first,
/// one at a time, on the host; then the scripts whose prep passed run on
/// up to `plan.jobs` worker threads, each taking the next script in plan
/// order. A failing script — or a failing prep build — fails the tier and
/// the sweep **keeps going**, so one run reports every broken script
/// (zipline#62: continue after a failing script; record each).
pub fn run_netns(plan: &NetnsPlan, logs: &Path, quiet: bool) -> TierOutcome {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let mut outcome = TierOutcome {
        passed: true,
        repos: BTreeMap::new(),
    };
    for (script, reason) in &plan.skipped {
        if !quiet {
            println!("{script}: skipped ({reason})");
        }
        outcome
            .repos
            .insert(script.to_string(), format!("skipped: {reason}"));
    }

    // Phase 1, serial: the prep builds. Their failure fails that script
    // alone; the rest of the tier still runs. Running them before any
    // script keeps a cargo build from competing with timing-sensitive tests.
    let mut runnable: Vec<&NetnsScript> = Vec::new();
    for script in &plan.scripts {
        if let Some(prep) = &script.prep {
            if let Err(error) = run_env_command(
                "netns",
                prep.name,
                prep.program,
                &prep.args,
                &[],
                &[],
                &prep.dir,
                logs,
                quiet,
            ) {
                if !quiet {
                    println!("{}: FAILED at prep `{}`", script.script, prep.name);
                }
                outcome.passed = false;
                outcome.repos.insert(
                    script.script.to_string(),
                    format!("failed at prep `{}`: {error}", prep.name),
                );
                continue;
            }
        }
        runnable.push(script);
    }

    // Phase 2, up to `plan.jobs` wide: the scripts. Each worker claims the
    // next unclaimed index until none remain; results are collected by
    // script name, so the outcome does not depend on finishing order.
    let next = AtomicUsize::new(0);
    let results: Mutex<Vec<(&str, Result<()>)>> = Mutex::new(Vec::new());
    let workers = plan.jobs.clamp(1, runnable.len().max(1));
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                while let Some(script) = runnable.get(next.fetch_add(1, Ordering::Relaxed)) {
                    let result = run_env_command(
                        "netns",
                        script.script,
                        &script.program,
                        &script.args,
                        &script.env,
                        &script.env_remove,
                        &plan.dir,
                        logs,
                        quiet,
                    );
                    if !quiet {
                        let verdict = if result.is_ok() { "passed" } else { "FAILED" };
                        println!("{}: {verdict}", script.script);
                    }
                    results
                        .lock()
                        .expect("a netns worker panicked while recording a result")
                        .push((script.script, result));
                }
            });
        }
    });

    let results = results
        .into_inner()
        .expect("a netns worker panicked while recording a result");
    for (script, result) in results {
        let entry = match result {
            Ok(()) => "passed".to_string(),
            Err(error) => {
                outcome.passed = false;
                format!("failed: {error}")
            }
        };
        outcome.repos.insert(script.to_string(), entry);
    }
    outcome
}

/// The runtime half of the dist/ guard (zipline#62): the staged `ph`
/// must not be an `enable-security-testing` build. A security-testing `ph`
/// advertises `--security-testing-mangle-forwarded-pings` in `node --help`
/// (the same detection `a2a-pubkey-test.sh` uses); a clean one does not.
/// A missing `dist/ph` passes — there is nothing to guard, and the staging
/// verification reports the absence separately.
pub fn dist_ph_is_clean(dist: &Path) -> Result<()> {
    let ph = dist.join("ph");
    if !ph.is_file() {
        return Ok(());
    }
    let output = std::process::Command::new(&ph)
        .args(["node", "--help"])
        .output()
        .map_err(|e| anyhow::anyhow!("cannot run {} node --help: {e}", ph.display()))?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    if text.contains("--security-testing-mangle-forwarded-pings") {
        bail!(
            "{} is an enable-security-testing build; that ph is for \
             a2a-pubkey-test.sh only and must never be staged into dist/",
            ph.display()
        );
    }
    Ok(())
}

/// [`recipes::run_command`] with per-child environment overrides: same
/// working-directory, log shape (`logs/<label>-<name>.log`), failure echo
/// and error wording. The env touches the child only, never this process.
/// `env_remove` strips inherited variables from the child before `env` is
/// applied (zipline#93): the child otherwise inherits this process's whole
/// environment, and an operator's exported value must not leak through.
#[allow(clippy::too_many_arguments)]
fn run_env_command(
    label: &str,
    name: &str,
    program: &str,
    args: &[impl AsRef<std::ffi::OsStr>],
    env: &[(String, String)],
    env_remove: &[&str],
    dir: &Path,
    logs: &Path,
    quiet: bool,
) -> Result<()> {
    use std::io::Write as _;

    let log_path = logs.join(format!("{label}-{name}.log"));
    let mut command = std::process::Command::new(program);
    command.args(args).current_dir(dir);
    for key in env_remove {
        command.env_remove(key);
    }
    for (key, value) in env {
        command.env(key, value);
    }
    let output = command.output().map_err(|e| {
        anyhow::anyhow!("cannot run {program} for {label} in {}: {e}", dir.display())
    })?;

    // One log per step, stdout then stderr — the same record run_command
    // writes, so netns logs read like build logs.
    let mut log = std::fs::File::create(&log_path)
        .map_err(|e| anyhow::anyhow!("cannot create {}: {e}", log_path.display()))?;
    log.write_all(&output.stdout)?;
    log.write_all(&output.stderr)?;

    if output.status.success() {
        return Ok(());
    }
    if !quiet {
        // Hold stderr for the whole dump: netns scripts can run
        // concurrently, and two interleaved tails would be unreadable.
        // The lock is re-entrant, so `eprintln!` below still works.
        let _stderr = std::io::stderr().lock();
        let text = std::fs::read_to_string(&log_path).unwrap_or_default();
        let lines: Vec<&str> = text.lines().collect();
        let start = lines.len().saturating_sub(40);
        eprintln!(
            "--- last {} lines of {} ---",
            lines.len() - start,
            log_path.display()
        );
        for line in &lines[start..] {
            eprintln!("{line}");
        }
    }
    bail!(
        "{label}: step `{name}` failed ({}); full output in {}",
        output.status,
        log_path.display()
    );
}

// ---------------------------------------------------------------------------
// The docker tier (issue62 step 4)
// ---------------------------------------------------------------------------

/// One docker-tier command: deploy, test, or teardown.
#[derive(Debug)]
pub struct DockerStep {
    /// Labels the log file (`logs/docker-<name>.log`) and the outcome entry.
    pub name: &'static str,
    pub program: String,
    pub args: Vec<String>,
    pub dir: PathBuf,
}

/// The docker tier's plan: stage `dist/` into the demo's `dns-demo/bin/`,
/// deploy, test, tear down. Pure planning — nothing here executes.
#[derive(Debug)]
pub struct DockerPlan {
    /// Where the built set's binaries are.
    pub dist: PathBuf,
    /// `<zl-zpr-demo worktree>/dns-demo/bin` — the copy target.
    pub bin: PathBuf,
    pub deploy: DockerStep,
    pub test: DockerStep,
    pub teardown: DockerStep,
}

/// Builds the docker plan against the `zl-zpr-demo` worktree and `dist/`:
/// stage `dist/`'s binaries into `dns-demo/bin/` — skipping the demo's own
/// `make` entirely, so the DNS test exercises the set's binaries rather
/// than a fresh build — then `local-compute/deploy-docker.sh`,
/// `local-compute/test-dns.sh`, and `docker compose down -v`
/// unconditionally (zipline#62).
pub fn docker_plan(demo_worktree: &Path, dist: &Path) -> DockerPlan {
    let demo = demo_worktree.join("dns-demo");
    let compose_file = demo.join("docker-compose.yml").display().to_string();
    DockerPlan {
        dist: dist.to_path_buf(),
        bin: demo.join("bin"),
        deploy: DockerStep {
            name: "deploy",
            program: demo
                .join("local-compute/deploy-docker.sh")
                .display()
                .to_string(),
            args: vec![],
            dir: demo.clone(),
        },
        test: DockerStep {
            name: "test",
            program: demo.join("local-compute/test-dns.sh").display().to_string(),
            args: vec![],
            dir: demo.clone(),
        },
        teardown: DockerStep {
            name: "teardown",
            program: "docker".to_string(),
            args: ["compose", "-f", &compose_file, "down", "-v"]
                .iter()
                .map(|arg| arg.to_string())
                .collect(),
            dir: demo,
        },
    }
}

/// Copies every staged name the recipe table produces from `dist/` into the
/// demo's `bin/`, executably — the two sets of names are identical, which is
/// what makes the DNS test exercise the set (zipline#62). A missing
/// binary is an error naming it: the demo must not run against a half-staged
/// `bin/`.
pub fn stage_dist_into_demo(dist: &Path, bin: &Path) -> Result<()> {
    std::fs::create_dir_all(bin)
        .map_err(|e| anyhow::anyhow!("cannot create {}: {e}", bin.display()))?;
    for recipe in recipes::RECIPES {
        for staged in recipe.staged {
            let source = dist.join(staged.name);
            if !source.is_file() {
                bail!(
                    "dist/ is missing {} (expected at {}); cannot stage the demo",
                    staged.name,
                    source.display()
                );
            }
            // fs::copy preserves the mode, so an executable stays executable.
            std::fs::copy(&source, bin.join(staged.name)).map_err(|e| {
                anyhow::anyhow!("cannot stage {} into {}: {e}", staged.name, bin.display())
            })?;
        }
    }
    Ok(())
}

/// Runs a docker plan: stage, deploy, test — each failure skipping the rest
/// — then teardown **unconditionally**, recorded separately from a test
/// failure (zipline#62). Compose down is safe when nothing is up, so
/// even a stage failure tears down: a previous run's leftovers must not
/// survive. Steps log as `logs/docker-<step>.log` in `run_unit`'s shape.
pub fn run_docker(plan: &DockerPlan, logs: &Path, quiet: bool) -> TierOutcome {
    let mut outcome = TierOutcome {
        passed: true,
        repos: BTreeMap::new(),
    };
    let record = |outcome: &mut TierOutcome, name: &str, result: Result<()>| -> bool {
        match result {
            Ok(()) => {
                if !quiet {
                    println!("{name}: passed");
                }
                outcome.repos.insert(name.to_string(), "passed".to_string());
                true
            }
            Err(error) => {
                if !quiet {
                    println!("{name}: FAILED");
                }
                outcome.passed = false;
                outcome
                    .repos
                    .insert(name.to_string(), format!("failed: {error}"));
                false
            }
        }
    };
    let run_step = |step: &DockerStep, quiet: bool| -> Result<()> {
        run_env_command(
            "docker",
            step.name,
            &step.program,
            &step.args,
            &[],
            &[],
            &step.dir,
            logs,
            quiet,
        )
    };

    // stage -> deploy -> test, each failure skipping what follows: the demo
    // must not deploy half-staged, and a failed deploy leaves nothing to
    // test. A skip is recorded with its reason, never silently absent.
    let staged = record(
        &mut outcome,
        "stage",
        stage_dist_into_demo(&plan.dist, &plan.bin),
    );
    let deployed = if staged {
        record(&mut outcome, "deploy", run_step(&plan.deploy, quiet))
    } else {
        outcome
            .repos
            .insert("deploy".to_string(), "skipped: staging failed".to_string());
        false
    };
    if deployed {
        record(&mut outcome, "test", run_step(&plan.test, quiet));
    } else {
        let reason = if staged {
            "skipped: deploy failed"
        } else {
            "skipped: staging failed"
        };
        outcome.repos.insert("test".to_string(), reason.to_string());
    }

    // Teardown, in every path — pass, fail, or half-start — and recorded
    // separately: a stuck volume is a different problem than a red test.
    record(&mut outcome, "teardown", run_step(&plan.teardown, quiet));
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The flag absent entirely selects every implemented tier — `unit`,
    /// `netns` and `docker` since B5 (approved Q1 on zipline#62: the two
    /// end-to-end tiers join the default selection with probe-gated skips).
    /// A default-selected tier is not *explicit*: its probe failing skips
    /// it with a reason rather than failing the run.
    #[test]
    fn absent_flag_selects_the_implemented_tiers() {
        let selection = Selection::parse(None).unwrap();
        assert!(!selection.is_empty());
        for tier in ["unit", "netns", "docker"] {
            assert!(selection.contains(tier), "{tier} missing from default");
            assert!(
                !selection.is_explicit(tier),
                "{tier} must not be explicit under the default selection"
            );
        }
    }

    /// A tier named in `--test <list>` is explicit: the user asked for it,
    /// so a failing prerequisite probe is an error, never a silent skip
    /// (spec-003 §6; approved Q1 on zipline#62).
    #[test]
    fn a_listed_tier_is_explicit() {
        let selection = Selection::parse(Some("docker")).unwrap();
        assert!(selection.contains("docker"));
        assert!(selection.is_explicit("docker"));
        assert!(!selection.contains("unit"));
        assert!(!selection.contains("netns"));
        // Not selected at all, so not explicit either.
        assert!(!selection.is_explicit("unit"));
    }

    /// `all` selects every known tier, each explicitly — an asked-for tier
    /// that cannot run must never silently degrade (spec-003 §6).
    #[test]
    fn all_selects_every_tier_explicitly() {
        let selection = Selection::parse(Some("all")).unwrap();
        for tier in ["unit", "netns", "docker"] {
            assert!(selection.contains(tier), "{tier} missing from all");
            assert!(selection.is_explicit(tier), "{tier} must be explicit");
        }
    }

    /// The netns and docker tiers are selectable by name, alone and in a
    /// list, now that B5 has landed.
    #[test]
    fn netns_and_docker_are_selectable_by_name() {
        for flag in ["netns", "docker", "unit,netns,docker"] {
            let selection = Selection::parse(Some(flag)).unwrap();
            assert!(!selection.is_empty(), "{flag}");
        }
        let selection = Selection::parse(Some("unit,docker")).unwrap();
        assert!(selection.contains("unit"));
        assert!(selection.contains("docker"));
        assert!(!selection.contains("netns"));
    }

    /// `default` is the spelled-out form of the absent flag.
    #[test]
    fn default_matches_the_absent_flag() {
        assert_eq!(
            Selection::parse(Some("default")).unwrap(),
            Selection::parse(None).unwrap()
        );
    }

    /// The literal `--test` text is kept for the emitted manifest's
    /// `tests_requested` (zipline#87): a reader must be able to see that a
    /// set was gated on `unit` alone. The absent flag reads `default`.
    #[test]
    fn selection_records_the_requested_text() {
        assert_eq!(Selection::parse(None).unwrap().requested(), "default");
        assert_eq!(Selection::parse(Some("unit")).unwrap().requested(), "unit");
        assert_eq!(Selection::parse(Some("all")).unwrap().requested(), "all");
        assert_eq!(Selection::parse(Some("none")).unwrap().requested(), "none");
        assert_eq!(
            Selection::parse(Some("unit,docker")).unwrap().requested(),
            "unit,docker"
        );
    }

    /// Every known tier has a human label for the manifest's `notes`
    /// (zipline#87), so a reader sees "integration tests did NOT run"
    /// rather than a bare tier name.
    #[test]
    fn every_known_tier_has_a_label() {
        assert_eq!(known(), &["unit", "netns", "docker"]);
        assert_eq!(label("unit"), "unit tests");
        assert_eq!(label("netns"), "netns integration tests");
        assert_eq!(label("docker"), "docker end-to-end tests");
    }

    /// `none` selects nothing: build only, byte-identical to B3 behaviour.
    #[test]
    fn none_selects_nothing() {
        let selection = Selection::parse(Some("none")).unwrap();
        assert!(selection.is_empty());
        assert!(!selection.contains("unit"));
    }

    /// A single implemented tier by name.
    #[test]
    fn unit_selects_exactly_unit() {
        let selection = Selection::parse(Some("unit")).unwrap();
        assert!(selection.contains("unit"));
        assert!(!selection.is_empty());
    }

    /// A comma-separated list is accepted, tolerating spaces and duplicates,
    /// as long as every name is implemented.
    #[test]
    fn comma_list_of_implemented_tiers_is_accepted() {
        let selection = Selection::parse(Some("unit, unit")).unwrap();
        assert!(selection.contains("unit"));
    }

    /// A known tier whose stage has not landed used to be rejected naming
    /// the stage; both stages have landed, so this behaviour is gone —
    /// covered by `netns_and_docker_are_selectable_by_name` and
    /// `all_selects_every_tier_explicitly` above. (The two B5-era tests were
    /// replaced in this change; their RED failure was the proof the
    /// behaviour changed.)
    /// An unknown name is rejected listing the valid values, so the fix is
    /// obvious from the message (exit 2 through `run`'s `Err` path).
    #[test]
    fn unknown_tier_is_rejected_listing_the_valid_names() {
        for flag in ["bogus", "unit,bogus", ""] {
            let error = Selection::parse(Some(flag)).unwrap_err().to_string();
            assert!(error.contains("unit"), "{flag}: {error}");
            assert!(error.contains("none"), "{flag}: {error}");
        }
    }

    /// `none`, `default` and `all` describe whole selections, not tiers:
    /// mixing them into a list is a usage error.
    #[test]
    fn special_words_in_a_list_are_rejected() {
        for flag in ["unit,none", "none,unit", "unit,default", "unit,all"] {
            assert!(Selection::parse(Some(flag)).is_err(), "{flag}");
        }
    }

    /// `all` used to be an error while tiers were unimplemented; every tier
    /// has landed, so `all` now selects all three explicitly — see
    /// `all_selects_every_tier_explicitly` above.

    // -- the prerequisite probes (issue62 step 2) -------------------------------

    /// A probe result with everything present.
    fn all_present() -> Probes {
        Probes {
            linux: true,
            unprivileged_userns: true,
            valkey_server: Some(PathBuf::from("/usr/bin/valkey-server")),
            python3: true,
            docker: true,
            docker_compose: true,
            docker_daemon: true,
        }
    }

    /// The probe set carries whether the docker daemon answered `docker
    /// info` (zipline#92 step 1) — gathered only when `docker` itself is on
    /// PATH, mirroring the `docker_compose` guard in `Probes::gather`. The
    /// injected constructors carry the field explicitly, so every selection
    /// rule over it is testable without a live docker.
    #[test]
    fn probes_carry_docker_daemon_reachability() {
        let probes = all_present();
        assert!(probes.docker_daemon);
        let probes = Probes {
            docker_daemon: false,
            ..all_present()
        };
        assert!(!probes.docker_daemon);
    }

    /// With every prerequisite present both tiers gate to `Run`.
    #[test]
    fn gates_run_when_every_prerequisite_is_present() {
        let probes = all_present();
        assert!(matches!(netns_gate(&probes), TierGate::Run));
        assert!(matches!(docker_gate(&probes), TierGate::Run));
    }

    /// Each missing netns prerequisite lands in the skip reason by name, so
    /// the output and the manifest say exactly what to install (spec-003 §6:
    /// never a silent skip). The daemon is out too — with a fallback
    /// available this scenario now selects the container instead (see
    /// `netns_gate_refuses_a_container_selection_until_93_lands`) — so the
    /// reason is the selector's two-route text, host gaps first.
    #[test]
    fn netns_gate_names_each_missing_prerequisite() {
        let mut probes = all_present();
        probes.unprivileged_userns = false;
        probes.valkey_server = None;
        probes.docker_daemon = false;
        let TierGate::Skip(reason) = netns_gate(&probes) else {
            panic!("netns must skip without user namespaces");
        };
        assert!(
            reason.contains(MISSING_USERNS),
            "the skip reason must make the fix discoverable: {reason}"
        );
        assert!(reason.contains("valkey-server"), "{reason}");
        // Present prerequisites are not named as missing.
        assert!(!reason.contains("python3"), "{reason}");
    }

    /// A non-Linux host skips netns naming the platform.
    #[test]
    fn netns_gate_requires_linux() {
        let mut probes = all_present();
        probes.linux = false;
        let TierGate::Skip(reason) = netns_gate(&probes) else {
            panic!("netns must skip off linux");
        };
        assert!(reason.to_lowercase().contains("linux"), "{reason}");
    }

    /// The dry-run planned-tier line for netns (zipline#92 step 5;
    /// zipline#93 step 4): the host route reads `prerequisites present`; a
    /// container selection says what the real run would do — `would run in
    /// docker` — quoting the host route's real gap, whichever it was; no
    /// route at all reports the selector's two-route reason.
    #[test]
    fn netns_dry_run_text_reports_the_selection_without_overstating() {
        assert_eq!(netns_dry_run_text(&all_present()), "prerequisites present");

        // User namespaces are the host gap: the text names the sysctl fix.
        let probes = Probes {
            unprivileged_userns: false,
            ..all_present()
        };
        assert_eq!(
            netns_dry_run_text(&probes),
            format!("would run in docker (host route unavailable: missing: {MISSING_USERNS})")
        );

        // valkey is the host gap: the text names it, not a fixed cause.
        let probes = Probes {
            valkey_server: None,
            ..all_present()
        };
        assert_eq!(
            netns_dry_run_text(&probes),
            "would run in docker (host route unavailable: missing: valkey-server)"
        );

        // No route: the selector's reason verbatim.
        let probes = Probes {
            unprivileged_userns: false,
            docker_daemon: false,
            ..all_present()
        };
        assert_eq!(
            netns_dry_run_text(&probes),
            select_netns_runner(&probes).unwrap_err()
        );
    }

    /// The docker gate's reason matches the acceptance wording: a missing
    /// docker binary reads `docker not found`.
    #[test]
    fn docker_gate_names_the_missing_prerequisite() {
        let mut probes = all_present();
        probes.docker = false;
        let TierGate::Skip(reason) = docker_gate(&probes) else {
            panic!("docker must skip without docker");
        };
        assert!(reason.contains("docker not found"), "{reason}");

        let mut probes = all_present();
        probes.docker_compose = false;
        let TierGate::Skip(reason) = docker_gate(&probes) else {
            panic!("docker must skip without compose");
        };
        assert!(reason.contains("docker compose"), "{reason}");
    }

    /// A `docker` client whose daemon does not answer fails the docker tier
    /// mid-deploy today; with the daemon probe (zipline#92) the gate catches
    /// it up front, with a reason naming the daemon rather than the client.
    #[test]
    fn docker_gate_requires_a_reachable_daemon() {
        let mut probes = all_present();
        probes.docker_daemon = false;
        let TierGate::Skip(reason) = docker_gate(&probes) else {
            panic!("docker must skip when the daemon is unreachable");
        };
        assert_eq!(reason, "docker daemon not reachable");
    }

    /// The PATH probe requires the candidate to be executable, not merely a
    /// regular file: a non-executable `valkey-server` shadowing the name on
    /// an earlier PATH entry is passed over in favour of a later executable
    /// one, and a PATH holding only the non-executable file finds nothing.
    /// Otherwise the netns gate runs against a binary that cannot start —
    /// and for valkey even exports the unusable path (Codex review, PR #10).
    #[test]
    fn path_probe_skips_non_executable_candidates() {
        use std::os::unix::fs::PermissionsExt as _;
        let tmp = tempfile::tempdir().unwrap();
        let decoy_dir = tmp.path().join("decoy");
        let real_dir = tmp.path().join("real");
        std::fs::create_dir_all(&decoy_dir).unwrap();
        std::fs::create_dir_all(&real_dir).unwrap();

        // A regular but non-executable file with the program's name.
        let decoy = decoy_dir.join("valkey-server");
        std::fs::write(&decoy, "not a binary").unwrap();
        std::fs::set_permissions(&decoy, std::fs::Permissions::from_mode(0o644)).unwrap();

        // The real, executable program on a later PATH entry.
        let real = real_dir.join("valkey-server");
        std::fs::write(&real, "#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o755)).unwrap();

        // Only the decoy on PATH: the probe finds nothing.
        let decoy_only = std::env::join_paths([&decoy_dir]).unwrap();
        assert_eq!(find_in_path_value(&decoy_only, "valkey-server"), None);

        // Decoy first, real second: the search continues past the decoy.
        let both = std::env::join_paths([&decoy_dir, &real_dir]).unwrap();
        assert_eq!(
            find_in_path_value(&both, "valkey-server"),
            Some(real.clone())
        );

        // A directory of the program's name is not a hit either.
        std::fs::remove_file(&decoy).unwrap();
        std::fs::create_dir(&decoy).unwrap();
        assert_eq!(find_in_path_value(&both, "valkey-server"), Some(real));
    }

    // -- the netns runner selection (zipline#92 step 2) --------------------------

    /// The host route wins whenever it can run: with every prerequisite
    /// present the selection is `Host`, never the container — the fallback
    /// is a fallback, not a preference (zipline#92; spec-003 §10.2).
    #[test]
    fn selector_picks_the_host_when_both_routes_work() {
        let runner = select_netns_runner(&all_present()).unwrap();
        assert_eq!(
            runner,
            NetnsRunner::Host {
                docker_fallback: Ok(())
            }
        );
    }

    /// Unusable user namespaces alone (`unshare -nr` blocked, e.g. by
    /// Ubuntu 24.04+'s AppArmor default) fall to the container when the
    /// daemon answers, and `host_reason` is the host route's own
    /// missing-prerequisite text — naming the sysctl that fixes the host —
    /// so zipline#93's header and coverage note quote the real cause.
    #[test]
    fn selector_falls_to_the_container_when_userns_is_blocked() {
        let probes = Probes {
            unprivileged_userns: false,
            ..all_present()
        };
        let NetnsRunner::Container { host_reason } = select_netns_runner(&probes).unwrap() else {
            panic!("blocked userns with a reachable daemon must select the container");
        };
        assert_eq!(
            host_reason,
            "missing: unprivileged user namespaces (unshare -nr failed; \
             on Ubuntu 24.04+ set kernel.apparmor_restrict_unprivileged_userns=0)"
        );
    }

    /// The fallback covers any host-route miss, not only user namespaces: a
    /// host with working `unshare -nr` but no `valkey-server` also falls to
    /// the container, and the reason names valkey, not a fixed cause
    /// (operator amendment — a fixed text would record false provenance).
    #[test]
    fn selector_falls_to_the_container_when_valkey_is_missing() {
        let probes = Probes {
            valkey_server: None,
            ..all_present()
        };
        let NetnsRunner::Container { host_reason } = select_netns_runner(&probes).unwrap() else {
            panic!("missing valkey with a reachable daemon must select the container");
        };
        assert_eq!(host_reason, "missing: valkey-server");
    }

    /// `docker` on PATH with no reachable daemon is not a usable fallback:
    /// the selection errors naming BOTH routes' gaps, so the operator sees
    /// what to fix on either route (zipline#92).
    #[test]
    fn selector_requires_a_reachable_daemon_not_merely_a_client() {
        let probes = Probes {
            unprivileged_userns: false,
            docker_daemon: false,
            ..all_present()
        };
        let reason = select_netns_runner(&probes).unwrap_err();
        assert_eq!(
            reason,
            format!(
                "missing: {MISSING_USERNS}; \
                 docker fallback unavailable: docker daemon not reachable"
            )
        );
    }

    /// No docker client at all is its own gap wording, matching the docker
    /// tier's `docker not found`.
    #[test]
    fn selector_names_a_missing_docker_client() {
        let probes = Probes {
            unprivileged_userns: false,
            docker: false,
            docker_compose: false,
            docker_daemon: false,
            ..all_present()
        };
        let reason = select_netns_runner(&probes).unwrap_err();
        assert!(reason.contains(MISSING_USERNS), "{reason}");
        assert!(
            reason.contains("docker fallback unavailable: docker not found"),
            "{reason}"
        );
    }

    /// A non-Linux host errors: the netns scripts create network namespaces,
    /// which neither route offers off Linux (zipline#92).
    #[test]
    fn selector_errors_off_linux() {
        let probes = Probes {
            linux: false,
            ..all_present()
        };
        let reason = select_netns_runner(&probes).unwrap_err();
        assert!(reason.to_lowercase().contains("linux"), "{reason}");
    }

    /// Since zipline#93 a `Container` selection runs through the gate: the
    /// docker fallback is implemented, so `netns_gate` maps it to `Run` —
    /// the run-time floor check (`docker_runner_floor`) is the only
    /// remaining refusal, and it goes through `check_gate` in
    /// `execute_build` so an explicit request still errors. This replaces
    /// zipline#92's placeholder gate-skip test; its RED failure was the
    /// proof the behaviour changed.
    #[test]
    fn netns_gate_runs_a_container_selection() {
        let probes = Probes {
            unprivileged_userns: false,
            ..all_present()
        };
        assert!(matches!(
            select_netns_runner(&probes),
            Ok(NetnsRunner::Container { .. })
        ));
        assert!(matches!(netns_gate(&probes), TierGate::Run));
    }

    /// A failed probe on a default-selected (non-explicit) tier is a skip
    /// carrying the reason; the same failure on an explicitly requested tier
    /// is an error with the same text (approved Q1 on zipline#62).
    #[test]
    fn gate_check_skips_by_default_and_errors_when_explicit() {
        let gate = TierGate::Skip("docker not found".to_string());
        // Non-explicit: skip with the reason.
        let skipped = check_gate("docker", gate, false).unwrap();
        assert_eq!(skipped, Some("docker not found".to_string()));
        // Explicit: the same text as an error (exit 1 through run()).
        let gate = TierGate::Skip("docker not found".to_string());
        let error = check_gate("docker", gate, true).unwrap_err().to_string();
        assert!(error.contains("docker not found"), "{error}");
        assert!(error.contains("docker"), "{error}");
        // A passing gate runs either way.
        assert_eq!(check_gate("docker", TierGate::Run, true).unwrap(), None);
        assert_eq!(check_gate("docker", TierGate::Run, false).unwrap(), None);
    }

    /// The stdout tier header (zipline#93 step 5): the host route keeps the
    /// plain `netns tier:`, and the container route names the fallback and
    /// quotes the host route's real failure, so an operator watching the run
    /// sees the cause without opening the manifest.
    #[test]
    fn netns_tier_header_names_the_docker_fallback_with_the_host_reason() {
        assert_eq!(netns_tier_header(&host_runner()), "netns tier:");
        assert_eq!(
            netns_tier_header(&NetnsRunner::Container {
                host_reason: "missing: valkey-server".to_string()
            }),
            "netns tier (docker fallback; host route unavailable: missing: valkey-server):"
        );
    }

    // -- the Makefile floor for the container route (zipline#93 step 2) ---------

    /// A worktree whose `integration-test/Makefile` defines `FORWARD_ENV`
    /// meets the container route's floor (`zl-zpr-core` @ `c163628`, the
    /// commit that forwards the environment into the container).
    #[test]
    fn docker_runner_floor_passes_a_makefile_with_forward_env() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("integration-test");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("Makefile"),
            "FORWARD_ENV := PH_BIN VS_BIN\ndocker-test:\n\ttrue\n",
        )
        .unwrap();
        assert_eq!(docker_runner_floor(tmp.path(), "c0ffee1"), Ok(()));
    }

    /// A worktree without the Makefile — or with one that predates
    /// `FORWARD_ENV` — fails the floor with a reason naming the sha and the
    /// commit that introduced the runner, so the operator knows what to
    /// update (zipline#93 step 2).
    #[test]
    fn docker_runner_floor_names_the_sha_and_the_floor_commit() {
        let expected = "zl-zpr-core @ c0ffee1 predates the Docker runner \
                        (integration-test/Makefile with FORWARD_ENV, zl-zpr-core c163628)";

        // No integration-test/Makefile at all.
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(
            docker_runner_floor(tmp.path(), "c0ffee1"),
            Err(expected.to_string())
        );

        // A Makefile from before c163628: no FORWARD_ENV.
        let dir = tmp.path().join("integration-test");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Makefile"), "docker-test:\n\ttrue\n").unwrap();
        assert_eq!(
            docker_runner_floor(tmp.path(), "c0ffee1"),
            Err(expected.to_string())
        );
    }

    /// A `Host` selection whose Docker fallback was usable at probe time.
    fn host_runner() -> NetnsRunner {
        NetnsRunner::Host {
            docker_fallback: Ok(()),
        }
    }

    /// Writes `integration-test/lib/common_funcs.sh` with `body` into
    /// `core`, the shape [`host_runner_floor`] reads.
    fn write_common_funcs(core: &Path, body: &str) {
        let lib = core.join("integration-test").join("lib");
        std::fs::create_dir_all(&lib).unwrap();
        std::fs::write(lib.join("common_funcs.sh"), body).unwrap();
    }

    /// Writes an `integration-test/Makefile` that passes
    /// [`docker_runner_floor`] into `core`.
    fn write_docker_runner_makefile(core: &Path) {
        let dir = core.join("integration-test");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Makefile"), "FORWARD_ENV := PH_BIN\n").unwrap();
    }

    /// The selector records whether the Docker fallback was usable when it
    /// picks the host, so the execution-time host floor can fall back
    /// (review of zl-zpr-dev-context#65).
    #[test]
    fn selector_records_the_docker_fallback_on_a_host_selection() {
        let mut probes = all_present();
        probes.docker_daemon = false;
        assert_eq!(
            select_netns_runner(&probes).unwrap(),
            NetnsRunner::Host {
                docker_fallback: Err("docker daemon not reachable".to_string())
            }
        );
        probes.docker = false;
        assert_eq!(
            select_netns_runner(&probes).unwrap(),
            NetnsRunner::Host {
                docker_fallback: Err("docker not found".to_string())
            }
        );
    }

    /// The host floor passes a worktree whose harness defines `pkill_own`
    /// (zipline#191 and later), and fails an older one with a reason naming
    /// the sha and the floor commit: a missing `common_funcs.sh`, the
    /// pre-#190 sudo harness, and the #190 transitional harness, which has
    /// `enter_test_namespace` but still runs unconverted scripts under sudo.
    #[test]
    fn host_runner_floor_requires_the_unprivileged_harness() {
        let expected = "zl-zpr-core @ c0ffee1 predates the unprivileged netns harness \
                        (integration-test/lib/common_funcs.sh with pkill_own, zl-zpr-core 3ad48ed)";
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(
            host_runner_floor(tmp.path(), "c0ffee1"),
            Err(expected.to_string())
        );
        write_common_funcs(
            tmp.path(),
            "function create_network() {\n  sudo ip netns add zpr-node\n}\n",
        );
        assert_eq!(
            host_runner_floor(tmp.path(), "c0ffee1"),
            Err(expected.to_string())
        );
        write_common_funcs(tmp.path(), "function enter_test_namespace() {\n  :\n}\n");
        assert_eq!(
            host_runner_floor(tmp.path(), "c0ffee1"),
            Err(expected.to_string())
        );
        write_common_funcs(tmp.path(), "function pkill_own() {\n  :\n}\n");
        assert_eq!(host_runner_floor(tmp.path(), "c0ffee1"), Ok(()));
    }

    /// A host selection on a worktree that passes the host floor runs on
    /// the host, whatever the fallback.
    #[test]
    fn floors_keep_the_host_route_on_a_converted_worktree() {
        let tmp = tempfile::tempdir().unwrap();
        write_common_funcs(tmp.path(), "function pkill_own() {\n  :\n}\n");
        let runner = NetnsRunner::Host {
            docker_fallback: Err("docker not found".to_string()),
        };
        assert_eq!(
            apply_runner_floors(&runner, tmp.path(), "c0ffee1"),
            Ok(runner)
        );
    }

    /// A host selection on a pre-zipline#191 worktree must not run its
    /// sudo-based, shared-name scripts on the host (review of
    /// zl-zpr-dev-context#65): it falls back to the container, quoting the
    /// host floor's failure, when Docker answered and the worktree passes
    /// the Docker floor.
    #[test]
    fn floors_send_a_pre_conversion_worktree_to_the_container() {
        let tmp = tempfile::tempdir().unwrap();
        write_docker_runner_makefile(tmp.path());
        let floored = apply_runner_floors(&host_runner(), tmp.path(), "c0ffee1").unwrap();
        let NetnsRunner::Container { host_reason } = floored else {
            panic!("expected the container route, got {floored:?}");
        };
        assert!(
            host_reason.contains("predates the unprivileged netns harness"),
            "{host_reason}"
        );
        assert_eq!(floored_privilege(tmp.path()), Privilege::Container);
    }

    /// The privilege a host selection ends up with on `core`.
    fn floored_privilege(core: &Path) -> Privilege {
        apply_runner_floors(&host_runner(), core, "c0ffee1")
            .unwrap()
            .privilege()
    }

    /// With no usable fallback a pre-conversion worktree is the two-route
    /// `Err`, naming the host floor and the docker gap — and so is one
    /// whose Makefile also predates the Docker runner.
    #[test]
    fn floors_refuse_a_pre_conversion_worktree_without_a_usable_fallback() {
        let tmp = tempfile::tempdir().unwrap();
        let no_docker = NetnsRunner::Host {
            docker_fallback: Err("docker not found".to_string()),
        };
        let reason = apply_runner_floors(&no_docker, tmp.path(), "c0ffee1").unwrap_err();
        assert!(
            reason.contains("predates the unprivileged netns harness"),
            "{reason}"
        );
        assert!(
            reason.ends_with("; docker fallback unavailable: docker not found"),
            "{reason}"
        );

        let reason = apply_runner_floors(&host_runner(), tmp.path(), "c0ffee1").unwrap_err();
        assert!(
            reason.contains("predates the unprivileged netns harness"),
            "{reason}"
        );
        assert!(
            reason.contains(
                "docker fallback unavailable: zl-zpr-core @ c0ffee1 predates the Docker runner"
            ),
            "{reason}"
        );
    }

    /// A container selection keeps its own floor and its own reason.
    #[test]
    fn floors_check_a_container_selection_against_the_docker_floor() {
        let tmp = tempfile::tempdir().unwrap();
        let container = NetnsRunner::Container {
            host_reason: "missing: python3".to_string(),
        };
        assert_eq!(
            apply_runner_floors(&container, tmp.path(), "c0ffee1"),
            docker_runner_floor(tmp.path(), "c0ffee1").map(|()| container.clone())
        );
        write_docker_runner_makefile(tmp.path());
        assert_eq!(
            apply_runner_floors(&container, tmp.path(), "c0ffee1"),
            Ok(container)
        );
    }

    // -- the netns tier's plan (issue62 step 3) ---------------------------------

    /// One env lookup in a planned script.
    fn env_of<'a>(script: &'a NetnsScript, key: &str) -> Option<&'a str> {
        script
            .env
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.as_str())
    }

    /// The container route's plan (zipline#93 step 3, contract 1): every
    /// script becomes one `make -C <core>/integration-test docker-test
    /// TEST=<script> WORKSPACE=<build-dir>` invocation in the worktree, the
    /// env keeps the `*_BIN` overrides but drops `VALKEY_SERVER_BIN` — the
    /// image ships its own valkey, and a host path would not exist inside
    /// the container — and `VALKEY_SERVER_BIN` is also in `env_remove` so an
    /// operator's exported value cannot leak through the Makefile's
    /// `FORWARD_ENV`. The a2a prep build is unchanged: it runs on the host
    /// and its debug `ph` is under the mount (Finding 3).
    #[test]
    fn netns_plan_container_route_runs_make_docker_test_per_script() {
        let runner = NetnsRunner::Container {
            host_reason: format!("missing: {MISSING_USERNS}"),
        };
        let plan = netns_plan(
            Path::new("/wt/zl-zpr-core"),
            Path::new("/b/dist"),
            Path::new("/usr/bin/valkey-server"),
            false,
            &runner,
            Path::new("/b"),
            None,
        )
        .expect("a fixture-free plan is clean");
        // The workdir is the worktree; `-C` names the Makefile's directory.
        assert_eq!(plan.dir, Path::new("/wt/zl-zpr-core"));
        // Each container is its own network namespace, so scripts run
        // concurrently.
        assert_eq!(plan.jobs, NETNS_JOBS);
        for script in &plan.scripts {
            assert_eq!(script.program, "make", "{}", script.script);
            assert_eq!(
                script.args,
                [
                    "-C",
                    "/wt/zl-zpr-core/integration-test",
                    "docker-test",
                    &format!("TEST={}", script.script),
                    "WORKSPACE=/b",
                ],
                "{}",
                script.script
            );
            // The container carries its own valkey: never set, and stripped
            // from the inherited environment too.
            assert_eq!(
                env_of(script, "VALKEY_SERVER_BIN"),
                None,
                "{}",
                script.script
            );
            assert!(
                script.env_remove.contains(&"VALKEY_SERVER_BIN"),
                "{} must remove VALKEY_SERVER_BIN",
                script.script
            );
            // The *_BIN overrides still point at the build's dist/.
            assert_eq!(env_of(script, "PH_DEBUG_BIN"), Some("/b/dist/ph-cli"));
            assert_eq!(env_of(script, "VS_BIN"), Some("/b/dist/vs"));
            assert_eq!(env_of(script, "VS_ADMIN_BIN"), Some("/b/dist/vs-admin"));
            if script.script == "a2a-pubkey-test.sh" {
                let prep = script.prep.as_ref().expect("a2a needs a prep build");
                assert_eq!(prep.program, "cargo");
                assert_eq!(
                    env_of(script, "PH_BIN"),
                    Some("/wt/zl-zpr-core/target/debug/ph")
                );
            } else {
                assert!(script.prep.is_none(), "{} must not prep", script.script);
                assert_eq!(env_of(script, "PH_BIN"), Some("/b/dist/ph"));
            }
        }
    }

    /// `--jobs` overrides the tier's width on both routes (zipline#144,
    /// zipline#192): a non-default value reaches `NetnsPlan.jobs` verbatim
    /// and absence keeps [`NETNS_JOBS`]. The host route is no longer forced
    /// serial — each script runs in its own private user + network
    /// namespace, so the scripts cannot collide.
    #[test]
    fn netns_plan_jobs_override_applies_to_both_routes() {
        let container = NetnsRunner::Container {
            host_reason: "missing: valkey-server".to_string(),
        };
        let six = std::num::NonZeroUsize::new(6);
        let plan = |runner: &NetnsRunner, jobs| {
            netns_plan(
                Path::new("/wt/zl-zpr-core"),
                Path::new("/b/dist"),
                Path::new("/usr/bin/valkey-server"),
                false,
                runner,
                Path::new("/b"),
                jobs,
            )
            .expect("a fixture-free plan is clean")
        };
        for runner in [&container, &host_runner()] {
            assert_eq!(
                plan(runner, six).jobs,
                6,
                "{runner:?}: --jobs 6 must reach the plan"
            );
            assert_eq!(
                plan(runner, None).jobs,
                NETNS_JOBS,
                "{runner:?}: no --jobs keeps the default width"
            );
        }
    }

    /// The host route's plan is unchanged by the runner parameter
    /// (zipline#93 step 3): direct script invocation from the
    /// integration-test directory, `VALKEY_SERVER_BIN` set, nothing removed
    /// — the exact shape the pre-#93 plan had.
    #[test]
    fn netns_plan_host_route_shape_is_unchanged() {
        let plan = netns_plan(
            Path::new("/wt/zl-zpr-core"),
            Path::new("/b/dist"),
            Path::new("/usr/bin/valkey-server"),
            false,
            &host_runner(),
            Path::new("/b"),
            None,
        )
        .expect("a fixture-free plan is clean");
        assert_eq!(plan.dir, Path::new("/wt/zl-zpr-core/integration-test"));
        for script in &plan.scripts {
            assert_eq!(
                script.program,
                format!("/wt/zl-zpr-core/integration-test/{}", script.script)
            );
            assert!(script.args.is_empty(), "{}", script.script);
            assert!(script.env_remove.is_empty(), "{}", script.script);
        }
        assert_eq!(
            env_of(&plan.scripts[0], "VALKEY_SERVER_BIN"),
            Some("/usr/bin/valkey-server")
        );
    }

    /// `--verbose` on the container route still exports `ZPR_TEST_VERBOSE=1`
    /// (zipline#93 step 6): the Makefile's `FORWARD_ENV` forwards it into
    /// the container, so the child env must carry it exactly as on the host.
    #[test]
    fn netns_plan_container_route_keeps_verbose_export() {
        let runner = NetnsRunner::Container {
            host_reason: "missing: valkey-server".to_string(),
        };
        let plan = netns_plan(
            Path::new("/wt/zl-zpr-core"),
            Path::new("/b/dist"),
            Path::new("/usr/bin/valkey-server"),
            true,
            &runner,
            Path::new("/b"),
            None,
        )
        .expect("a fixture-free plan is clean");
        for script in &plan.scripts {
            assert_eq!(
                env_of(script, "ZPR_TEST_VERBOSE"),
                Some("1"),
                "{} missing ZPR_TEST_VERBOSE under --verbose on the container route",
                script.script
            );
        }
    }

    /// The plan runs exactly the twelve blessed scripts, in order — an
    /// explicit list, not a glob: `unused_or_outdated/` and any new script
    /// stay out until reviewed in (zipline#62, zipline#103). The order is
    /// the documented one: `attr-query-test.sh` after `a2a-pubkey-test.sh`,
    /// `one-node-oidc-renewal-test.sh` after that (approved Q1 on
    /// zipline#103), `one-node-policy-reauth-test.sh` — the
    /// policy-install re-authentication e2e (zipline#124) — after that
    /// (zipline#125), and `node-restart-test.sh` — the node-restart
    /// regression e2e (zipline#167/#170/#171) — after that (zipline#178),
    /// and `zode-link-test.sh` — the Zode link lifecycle e2e — last, newest
    /// last (zl-zipline#250).
    #[test]
    fn netns_plan_lists_the_twelve_scripts_in_order() {
        let plan = netns_plan(
            Path::new("/wt/zl-zpr-core"),
            Path::new("/b/dist"),
            Path::new("/usr/bin/valkey-server"),
            false,
            &host_runner(),
            Path::new("/b"),
            None,
        )
        .expect("a worktree with no integration-test/ plans cleanly");
        // Each script runs in its own private namespaces, so the host route
        // takes the same default width as the container (zipline#192).
        assert_eq!(plan.jobs, NETNS_JOBS);
        let names: Vec<&str> = plan.scripts.iter().map(|script| script.script).collect();
        assert_eq!(
            names,
            [
                "one-node-test.sh",
                "one-node-v6-test.sh",
                "one-node-oidc-test.sh",
                "capture-test.sh",
                "oidc-file-interplay-test.sh",
                "fake-idp-smoke-test.sh",
                "a2a-pubkey-test.sh",
                "attr-query-test.sh",
                "one-node-oidc-renewal-test.sh",
                "one-node-policy-reauth-test.sh",
                "node-restart-test.sh",
                "zode-link-test.sh",
            ]
        );
        assert_eq!(plan.dir, Path::new("/wt/zl-zpr-core/integration-test"));
    }

    /// Writes a fixture `integration-test/` directory carrying the named
    /// top-level scripts (plus `lib/` and `unused_or_outdated/` decoys that
    /// the guard must never see — approved Q2 on zipline#103), and returns
    /// the worktree root.
    fn drift_fixture(scripts: &[&str]) -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("integration-test");
        std::fs::create_dir_all(dir.join("lib")).unwrap();
        std::fs::create_dir_all(dir.join("unused_or_outdated")).unwrap();
        for script in scripts {
            std::fs::write(dir.join(script), "#!/bin/sh\n").unwrap();
        }
        // Below top level: invisible to the guard, like the Makefile's
        // `$(wildcard *-test.sh)`.
        std::fs::write(dir.join("lib/helper-test.sh"), "#!/bin/sh\n").unwrap();
        std::fs::write(dir.join("unused_or_outdated/old-test.sh"), "#!/bin/sh\n").unwrap();
        // Non-matching top-level names are not scripts to the guard.
        std::fs::write(dir.join("Makefile"), "docker-test:\n\ttrue\n").unwrap();
        tmp
    }

    /// A feature-branch script (zl-zipline#250) runs only where the core
    /// worktree carries it: a core revision without `zode-link-test.sh`
    /// plans the other eleven and records the twelfth as skipped with a
    /// reason, so the tier neither fails on a missing file nor overstates
    /// what ran. With the script present it runs, last.
    #[test]
    fn netns_plan_skips_a_feature_script_the_worktree_lacks() {
        let older: Vec<&str> = NETNS_SCRIPTS
            .iter()
            .copied()
            .filter(|script| *script != "zode-link-test.sh")
            .collect();
        for (present, expect_run) in [(older.clone(), false), (NETNS_SCRIPTS.to_vec(), true)] {
            let tmp = drift_fixture(&present);
            let plan = netns_plan(
                tmp.path(),
                Path::new("/b/dist"),
                Path::new("/usr/bin/valkey-server"),
                false,
                &host_runner(),
                Path::new("/b"),
                None,
            )
            .expect("both worktrees plan cleanly");
            let names: Vec<&str> = plan.scripts.iter().map(|script| script.script).collect();
            if expect_run {
                assert_eq!(names, NETNS_SCRIPTS);
                assert!(plan.skipped.is_empty());
            } else {
                assert_eq!(names, older);
                assert_eq!(plan.skipped.len(), 1);
                assert_eq!(plan.skipped[0].0, "zode-link-test.sh");
                assert!(
                    plan.skipped[0]
                        .1
                        .contains("not in this zl-zpr-core revision")
                );
            }
        }
    }

    /// The drift guard (zipline#103): a top-level `*-test.sh` in the
    /// worktree that is neither gated nor excluded fails planning with an
    /// error naming the script and both lists, so a new script can never be
    /// silently not-run.
    #[test]
    fn netns_plan_fails_on_an_unlisted_top_level_script() {
        let mut scripts: Vec<&str> = NETNS_SCRIPTS.to_vec();
        scripts.push("new-test.sh");
        let tmp = drift_fixture(&scripts);
        let error = netns_plan(
            tmp.path(),
            Path::new("/b/dist"),
            Path::new("/usr/bin/valkey-server"),
            false,
            &host_runner(),
            Path::new("/b"),
            None,
        )
        .expect_err("an unlisted script must fail planning")
        .to_string();
        assert!(
            error.contains("new-test.sh"),
            "must name the script: {error}"
        );
        assert!(
            error.contains("NETNS_SCRIPTS") && error.contains("NETNS_EXCLUDED"),
            "must point at the two lists to fix: {error}"
        );
    }

    /// The same fixture passes once the new script is deliberately
    /// excluded with a reason: exclusion is the reviewed way to keep a
    /// top-level script out of the tier (zipline#103).
    #[test]
    fn drift_guard_accepts_a_deliberately_excluded_script() {
        let mut scripts: Vec<&str> = NETNS_SCRIPTS.to_vec();
        scripts.push("new-test.sh");
        let tmp = drift_fixture(&scripts);
        netns_script_drift(
            &tmp.path().join("integration-test"),
            NETNS_SCRIPTS,
            &[("new-test.sh", "deliberately unrun: fixture reason")],
        )
        .expect("an excluded script must plan cleanly");
        // And the gated set alone — no strays — passes against the real
        // constants, empty exclusion list included.
        let tmp = drift_fixture(NETNS_SCRIPTS);
        netns_script_drift(
            &tmp.path().join("integration-test"),
            NETNS_SCRIPTS,
            NETNS_EXCLUDED,
        )
        .expect("the gated set alone must plan cleanly");
    }

    /// Review finding on zl-zpr-dev-context#35 (zipline#103): a top-level
    /// script whose name is not valid UTF-8 cannot be compared against
    /// [`NETNS_SCRIPTS`] / [`NETNS_EXCLUDED`], yet the Makefile's byte-wise
    /// `$(wildcard *-test.sh)` WOULD run it — so the guard must fail loudly
    /// on such a name rather than silently treating the script as absent.
    #[cfg(unix)]
    #[test]
    fn drift_guard_fails_closed_on_a_non_utf8_script_name() {
        use std::os::unix::ffi::OsStrExt;
        let tmp = drift_fixture(NETNS_SCRIPTS);
        let dir = tmp.path().join("integration-test");
        let bad = std::ffi::OsStr::from_bytes(b"bad-\xff-test.sh");
        std::fs::write(dir.join(bad), "#!/bin/sh\n").unwrap();
        let error = netns_script_drift(&dir, NETNS_SCRIPTS, NETNS_EXCLUDED)
            .expect_err("a non-UTF-8 script name must fail the guard, not vanish")
            .to_string();
        assert!(
            error.contains("non-UTF-8"),
            "must say why the name is unusable: {error}"
        );
    }

    /// Review finding on zl-zpr-dev-context#35 (zipline#103): an exclusion
    /// is only auditable with a reason. An entry like `("new-test.sh", "")`
    /// must be a hard error — the whole table is validated, so an
    /// empty-reason entry fails planning even before any script matches it.
    #[test]
    fn drift_guard_rejects_an_exclusion_without_a_reason() {
        let mut scripts: Vec<&str> = NETNS_SCRIPTS.to_vec();
        scripts.push("new-test.sh");
        let tmp = drift_fixture(&scripts);
        let error = netns_script_drift(
            &tmp.path().join("integration-test"),
            NETNS_SCRIPTS,
            &[("new-test.sh", "")],
        )
        .expect_err("an exclusion with an empty reason must be a hard error")
        .to_string();
        assert!(
            error.contains("new-test.sh") && error.contains("reason"),
            "must name the entry and the missing reason: {error}"
        );
        // Whitespace is not a reason either.
        let error = netns_script_drift(
            &tmp.path().join("integration-test"),
            NETNS_SCRIPTS,
            &[("new-test.sh", "  ")],
        )
        .expect_err("a whitespace-only reason must be a hard error")
        .to_string();
        assert!(
            error.contains("new-test.sh"),
            "must name the offending entry: {error}"
        );
    }

    /// zipline#178: `node-restart-test.sh` (added to `zl-zpr-core` by
    /// zl-zpr-core#65, reworked by zl-zpr-core#69) is the regression test
    /// for zipline#167/#170/#171, so it must be gated, not excluded. The
    /// drift guard locks the worktree side — any ungated script fails
    /// planning — and this locks the list side: the gate cannot regress
    /// without this test naming it.
    #[test]
    fn netns_scripts_gate_node_restart_test() {
        assert!(
            NETNS_SCRIPTS.contains(&"node-restart-test.sh"),
            "node-restart-test.sh must be in NETNS_SCRIPTS (zipline#178)"
        );
    }

    /// Every script gets the `*_BIN` overrides pointing at `dist/` and
    /// the system valkey — nothing is ever copied into `integration-test/`
    /// (zipline#62). `ZPR_ATTR_SERVER_BIN` and `ZPDUMP_BIN` joined in
    /// zipline#103: `attr-query-test.sh` requires the reference attribute
    /// server, and a build worktree has no symlinks next to the scripts, so
    /// the plan must point at the staged binaries (the Makefile's
    /// `FORWARD_ENV` already forwards both).
    #[test]
    fn netns_plan_points_the_bin_overrides_at_dist() {
        let plan = netns_plan(
            Path::new("/wt/zl-zpr-core"),
            Path::new("/b/dist"),
            Path::new("/usr/bin/valkey-server"),
            false,
            &host_runner(),
            Path::new("/b"),
            None,
        )
        .expect("a fixture-free plan is clean");
        // Every script except a2a runs the dist/ ph.
        let one_node = &plan.scripts[0];
        assert_eq!(env_of(one_node, "PH_BIN"), Some("/b/dist/ph"));
        assert_eq!(env_of(one_node, "PH_DEBUG_BIN"), Some("/b/dist/ph-cli"));
        assert_eq!(env_of(one_node, "VS_BIN"), Some("/b/dist/vs"));
        assert_eq!(env_of(one_node, "VS_ADMIN_BIN"), Some("/b/dist/vs-admin"));
        assert_eq!(
            env_of(one_node, "ZPR_ATTR_SERVER_BIN"),
            Some("/b/dist/zpr-attr-server")
        );
        assert_eq!(env_of(one_node, "ZPDUMP_BIN"), Some("/b/dist/zpdump"));
        assert_eq!(
            env_of(one_node, "VALKEY_SERVER_BIN"),
            Some("/usr/bin/valkey-server")
        );
        // Not verbose: ZPR_TEST_VERBOSE is not set at all.
        assert_eq!(env_of(one_node, "ZPR_TEST_VERBOSE"), None);
    }

    /// The host route never removes anything from the child's environment:
    /// `env_remove` exists for the container route (zipline#93), and an empty
    /// list keeps the host route byte-identical to today's behaviour.
    #[test]
    fn netns_plan_host_route_removes_nothing_from_the_environment() {
        let plan = netns_plan(
            Path::new("/wt/zl-zpr-core"),
            Path::new("/b/dist"),
            Path::new("/usr/bin/valkey-server"),
            false,
            &host_runner(),
            Path::new("/b"),
            None,
        )
        .expect("a fixture-free plan is clean");
        for script in &plan.scripts {
            assert!(
                script.env_remove.is_empty(),
                "{} must not remove environment variables on the host route",
                script.script
            );
        }
    }

    /// `run_env_command` must be able to *remove* a variable from the child's
    /// inherited environment (zipline#93, contract 1): it inherits this
    /// process's environment and only adds the listed pairs, so an operator's
    /// exported `VALKEY_SERVER_BIN` would otherwise be forwarded into the
    /// container by the Makefile's `FORWARD_ENV` (Codex review of PR #23).
    /// The child is executed, not planned: the assertion is the variable's
    /// absence in the running child's environment.
    ///
    /// The exported variable must be present in the environment of the
    /// process that calls `run_env_command`, but mutating THIS process's
    /// environment with `set_var` is unsound under the multithreaded test
    /// harness (Codex review of PR #25): other tests spawn commands and may
    /// read the environment concurrently, and the value would leak into
    /// later tests. So the call happens one process down: this test re-runs
    /// the test binary filtered to the `#[ignore]`d helper below, injecting
    /// `VALKEY_SERVER_BIN` through `Command::env` — the helper process is
    /// born with the variable, and no environment is ever mutated.
    #[test]
    fn run_env_command_removes_named_variables_from_the_executed_child() {
        let exe = std::env::current_exe().expect("the test binary's own path");
        let output = std::process::Command::new(exe)
            .args([
                "--exact",
                "build::tiers::tests::run_env_command_removal_helper",
                "--include-ignored",
            ])
            .env("VALKEY_SERVER_BIN", "/host/valkey-server")
            .output()
            .expect("re-running the test binary");
        assert!(
            output.status.success(),
            "removal helper failed:\n{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        // The filter must have matched exactly one test: a renamed or
        // deleted helper would otherwise make this test pass vacuously.
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("1 passed"),
            "the removal helper did not run:\n{stdout}"
        );
    }

    /// The executing half of
    /// [`run_env_command_removes_named_variables_from_the_executed_child`]:
    /// ignored so the normal suite never runs it, driven by that test in a
    /// child process whose environment carries `VALKEY_SERVER_BIN` from
    /// birth (`Command::env`, no `set_var`).
    #[test]
    #[ignore = "helper: driven by run_env_command_removes_named_variables_from_the_executed_child"]
    fn run_env_command_removal_helper() {
        assert!(
            std::env::var_os("VALKEY_SERVER_BIN").is_some(),
            "this helper asserts nothing without VALKEY_SERVER_BIN in its \
             environment; it is driven by \
             run_env_command_removes_named_variables_from_the_executed_child, \
             which injects the variable via Command::env"
        );
        let tmp = tempfile::tempdir().unwrap();
        let logs = tmp.path().join("logs");
        std::fs::create_dir_all(&logs).unwrap();

        // Without the removal the child inherits it: the fixture is real.
        run_env_command(
            "netns",
            "inherited",
            "sh",
            &["-c", "test -n \"$VALKEY_SERVER_BIN\""],
            &[],
            &[],
            tmp.path(),
            &logs,
            true,
        )
        .expect("the child must inherit the exported variable");

        // With the removal it is gone from the executed child's environment.
        run_env_command(
            "netns",
            "removed",
            "sh",
            &["-c", "test -z \"$VALKEY_SERVER_BIN\""],
            &[],
            &["VALKEY_SERVER_BIN"],
            tmp.path(),
            &logs,
            true,
        )
        .expect("env_remove must reach the executed child");
    }

    /// `--verbose` exports `ZPR_TEST_VERBOSE=1` to every script; the default
    /// leaves it unset (the scripts' own default is quiet).
    #[test]
    fn netns_plan_sets_test_verbose_only_under_verbose() {
        let plan = netns_plan(
            Path::new("/wt/zl-zpr-core"),
            Path::new("/b/dist"),
            Path::new("/usr/bin/valkey-server"),
            true,
            &host_runner(),
            Path::new("/b"),
            None,
        )
        .expect("a fixture-free plan is clean");
        for script in &plan.scripts {
            assert_eq!(
                env_of(script, "ZPR_TEST_VERBOSE"),
                Some("1"),
                "{} missing ZPR_TEST_VERBOSE under --verbose",
                script.script
            );
        }
    }

    /// `a2a-pubkey-test.sh` alone gets a prep step — the worktree-local
    /// `enable-security-testing` build of `ph` — and its `PH_BIN` points at
    /// that build's debug binary, not at `dist/` (zipline#62: that
    /// binary must never reach `dist/`).
    #[test]
    fn netns_plan_gives_a2a_its_own_security_testing_ph() {
        let plan = netns_plan(
            Path::new("/wt/zl-zpr-core"),
            Path::new("/b/dist"),
            Path::new("/usr/bin/valkey-server"),
            false,
            &host_runner(),
            Path::new("/b"),
            None,
        )
        .expect("a fixture-free plan is clean");
        for script in &plan.scripts {
            if script.script == "a2a-pubkey-test.sh" {
                let prep = script.prep.as_ref().expect("a2a needs a prep build");
                assert_eq!(prep.program, "cargo");
                assert_eq!(
                    prep.args,
                    ["build", "-p", "ph", "--features", "enable-security-testing"]
                );
                assert_eq!(prep.dir, Path::new("/wt/zl-zpr-core"));
                assert_eq!(
                    env_of(script, "PH_BIN"),
                    Some("/wt/zl-zpr-core/target/debug/ph")
                );
            } else {
                assert!(script.prep.is_none(), "{} must not prep", script.script);
                assert_eq!(env_of(script, "PH_BIN"), Some("/b/dist/ph"));
            }
        }
    }

    /// The staging table can never source the security-testing `ph`: it is
    /// a debug-profile build, and every staged source is a release path.
    /// This is the static half of the dist/ guard; the runtime half is
    /// `dist_ph_is_clean` below.
    #[test]
    fn no_staged_source_is_a_debug_build() {
        for recipe in recipes::RECIPES {
            for staged in recipe.staged {
                assert!(
                    !staged.source.contains("debug"),
                    "{}: staged source {} is a debug path — the security-testing \
                     ph build must never be stageable",
                    recipe.repo,
                    staged.source
                );
            }
        }
    }

    /// The runtime guard: a `dist/ph` that advertises the security-testing
    /// flag fails the check; one that does not passes; a missing `ph` passes
    /// (nothing to guard).
    #[test]
    fn dist_ph_clean_check_rejects_a_security_testing_build() {
        let tmp = tempfile::tempdir().unwrap();
        let dist = tmp.path();

        // No ph staged: nothing to guard.
        assert!(dist_ph_is_clean(dist).is_ok());

        // A clean ph: `node --help` does not mention the mangle flag.
        write_fake_ph(dist, "usage: ph node [--config PATH]\n");
        assert!(dist_ph_is_clean(dist).is_ok());

        // A security-testing ph: the check names the problem.
        write_fake_ph(
            dist,
            "usage: ph node [--security-testing-mangle-forwarded-pings]\n",
        );
        let error = dist_ph_is_clean(dist).unwrap_err().to_string();
        assert!(error.contains("enable-security-testing"), "{error}");
    }

    /// A fake `dist/ph` that prints `help_text` for any invocation.
    fn write_fake_ph(dist: &Path, help_text: &str) {
        use std::os::unix::fs::PermissionsExt as _;
        let path = dist.join("ph");
        std::fs::write(&path, format!("#!/bin/sh\nprintf '%s' '{help_text}'\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// The netns runner: continues after a failing script, records each
    /// script by name, and a prep-build failure fails that one script while
    /// the rest of the tier still runs.
    #[test]
    fn run_netns_continues_after_failure_and_prep_failure_hits_one_script() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("integration-test");
        std::fs::create_dir_all(&dir).unwrap();
        let logs = tmp.path().join("logs");
        std::fs::create_dir_all(&logs).unwrap();

        // Three fixture scripts: pass, fail, pass-with-failing-prep.
        for (name, body) in [
            ("ok.sh", "#!/bin/sh\necho fine\n"),
            ("bad.sh", "#!/bin/sh\necho broken; exit 3\n"),
            ("prepped.sh", "#!/bin/sh\necho never runs\n"),
        ] {
            use std::os::unix::fs::PermissionsExt as _;
            let path = dir.join(name);
            std::fs::write(&path, body).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let plan = NetnsPlan {
            skipped: Vec::new(),
            dir: dir.clone(),
            jobs: 1,
            scripts: vec![
                NetnsScript {
                    script: "ok.sh",
                    program: dir.join("ok.sh").display().to_string(),
                    args: vec![],
                    env: vec![],
                    env_remove: vec![],
                    prep: None,
                },
                NetnsScript {
                    script: "bad.sh",
                    program: dir.join("bad.sh").display().to_string(),
                    args: vec![],
                    env: vec![],
                    env_remove: vec![],
                    prep: None,
                },
                NetnsScript {
                    script: "prepped.sh",
                    program: dir.join("prepped.sh").display().to_string(),
                    args: vec![],
                    env: vec![],
                    env_remove: vec![],
                    prep: Some(PrepStep {
                        name: "security-ph",
                        program: "sh",
                        args: vec!["-c".to_string(), "exit 1".to_string()],
                        dir: tmp.path().to_path_buf(),
                    }),
                },
            ],
        };
        let outcome = run_netns(&plan, &logs, true);
        assert!(!outcome.passed);
        assert_eq!(outcome.repos["ok.sh"], "passed");
        let bad = &outcome.repos["bad.sh"];
        assert!(bad.starts_with("failed"), "{bad}");
        assert!(bad.contains("netns-bad.sh.log"), "log not named: {bad}");
        // The prep failure fails prepped.sh without running it...
        let prepped = &outcome.repos["prepped.sh"];
        assert!(prepped.starts_with("failed"), "{prepped}");
        assert!(prepped.contains("security-ph"), "{prepped}");
        // ...and the earlier pass proves the sweep visited every script.
        let ok_log = std::fs::read_to_string(logs.join("netns-ok.sh.log")).unwrap();
        assert!(ok_log.contains("fine"), "{ok_log}");
    }

    /// With `jobs` workers the netns runner really overlaps scripts: three
    /// fixture scripts each mark their arrival and then wait for the other
    /// two. Run one at a time, the first would wait alone and time out; run
    /// three wide, all meet and pass. Results still land under each
    /// script's name, and a script whose prep failed never runs.
    #[test]
    fn run_netns_runs_scripts_concurrently_up_to_jobs() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("integration-test");
        std::fs::create_dir_all(&dir).unwrap();
        let logs = tmp.path().join("logs");
        std::fs::create_dir_all(&logs).unwrap();
        let meet = tmp.path().join("meet");
        std::fs::create_dir_all(&meet).unwrap();

        // Touch our own marker, then poll up to ~10s for all three.
        let rendezvous = format!(
            "#!/bin/sh\ntouch {meet}/$1\n\
             for i in $(seq 100); do\n\
             [ $(ls {meet} | wc -l) -ge 3 ] && exit 0\n\
             sleep 0.1\ndone\nexit 1\n",
            meet = meet.display()
        );
        let path = dir.join("meet.sh");
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::write(&path, rendezvous).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let script = |name: &'static str| NetnsScript {
            script: name,
            program: path.display().to_string(),
            args: vec![name.to_string()],
            env: vec![],
            env_remove: vec![],
            prep: None,
        };
        let mut never_runs = script("never.sh");
        never_runs.prep = Some(PrepStep {
            name: "security-ph",
            program: "sh",
            args: vec!["-c".to_string(), "exit 1".to_string()],
            dir: tmp.path().to_path_buf(),
        });
        let plan = NetnsPlan {
            skipped: Vec::new(),
            dir: dir.clone(),
            jobs: 3,
            scripts: vec![script("a.sh"), never_runs, script("b.sh"), script("c.sh")],
        };
        let outcome = run_netns(&plan, &logs, true);
        for name in ["a.sh", "b.sh", "c.sh"] {
            assert_eq!(outcome.repos[name], "passed", "{name}");
        }
        assert!(outcome.repos["never.sh"].contains("security-ph"));
        assert!(
            !meet.join("never.sh").exists(),
            "a failed prep must skip its script"
        );
        assert!(!outcome.passed);
    }

    // -- the docker tier's plan (issue62 step 4) --------------------------------

    /// The plan stages `dist/` into the demo worktree's `dns-demo/bin/`,
    /// then deploys, tests, and tears down with `docker compose down -v` —
    /// never the demo's own `make`, so the DNS test exercises the set's
    /// binaries rather than a fresh build (zipline#62).
    #[test]
    fn docker_plan_stages_deploys_tests_and_tears_down() {
        let plan = docker_plan(Path::new("/wt/zl-zpr-demo"), Path::new("/b/dist"));
        assert_eq!(plan.dist, Path::new("/b/dist"));
        assert_eq!(plan.bin, Path::new("/wt/zl-zpr-demo/dns-demo/bin"));

        assert_eq!(plan.deploy.name, "deploy");
        assert!(
            plan.deploy
                .program
                .ends_with("local-compute/deploy-docker.sh"),
            "{}",
            plan.deploy.program
        );
        assert_eq!(plan.test.name, "test");
        assert!(
            plan.test.program.ends_with("local-compute/test-dns.sh"),
            "{}",
            plan.test.program
        );
        // Teardown is compose down -v against the demo's compose file.
        assert_eq!(plan.teardown.name, "teardown");
        assert_eq!(plan.teardown.program, "docker");
        assert_eq!(
            plan.teardown.args,
            [
                "compose",
                "-f",
                "/wt/zl-zpr-demo/dns-demo/docker-compose.yml",
                "down",
                "-v"
            ]
        );
    }

    /// Staging copies exactly the recipe table's staged names — the eleven
    /// binaries of contract 4 — into `bin/`, executably, and a missing one
    /// is an error naming it (the set must not half-stage).
    #[test]
    fn stage_dist_into_demo_copies_the_staged_names() {
        use std::os::unix::fs::PermissionsExt as _;
        let tmp = tempfile::tempdir().unwrap();
        let dist = tmp.path().join("dist");
        std::fs::create_dir_all(&dist).unwrap();
        let names: Vec<&str> = recipes::RECIPES
            .iter()
            .flat_map(|recipe| recipe.staged.iter().map(|staged| staged.name))
            .collect();
        for name in &names {
            let path = dist.join(name);
            std::fs::write(&path, "#!/bin/sh\n").unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let bin = tmp.path().join("demo/dns-demo/bin");

        stage_dist_into_demo(&dist, &bin).unwrap();
        for name in &names {
            let staged = bin.join(name);
            assert!(staged.is_file(), "{name} not staged");
            let mode = std::fs::metadata(&staged).unwrap().permissions().mode();
            assert!(mode & 0o111 != 0, "{name} staged non-executable");
        }

        // A missing binary is an error naming it.
        std::fs::remove_file(dist.join("coredns")).unwrap();
        let error = stage_dist_into_demo(&dist, &bin).unwrap_err().to_string();
        assert!(error.contains("coredns"), "{error}");
    }

    /// A fixture docker plan over shell scripts, with a marker file the
    /// teardown step touches so its execution is observable.
    fn fixture_docker_plan(tmp: &Path, test_body: &str, teardown_body: &str) -> DockerPlan {
        use std::os::unix::fs::PermissionsExt as _;
        let dist = tmp.join("dist");
        std::fs::create_dir_all(&dist).unwrap();
        for recipe in recipes::RECIPES {
            for staged in recipe.staged {
                let path = dist.join(staged.name);
                std::fs::write(&path, "#!/bin/sh\n").unwrap();
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
        }
        let write_script = |name: &str, body: &str| -> String {
            let path = tmp.join(name);
            std::fs::write(&path, body).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            path.display().to_string()
        };
        DockerPlan {
            dist,
            bin: tmp.join("demo/dns-demo/bin"),
            deploy: DockerStep {
                name: "deploy",
                program: write_script("deploy.sh", "#!/bin/sh\necho deployed\n"),
                args: vec![],
                dir: tmp.to_path_buf(),
            },
            test: DockerStep {
                name: "test",
                program: write_script("test.sh", test_body),
                args: vec![],
                dir: tmp.to_path_buf(),
            },
            teardown: DockerStep {
                name: "teardown",
                program: write_script("teardown.sh", teardown_body),
                args: vec![],
                dir: tmp.to_path_buf(),
            },
        }
    }

    /// Teardown runs even when the test step fails, and the two are recorded
    /// separately: the tier fails on the test, the teardown entry still says
    /// `passed` (zipline#62: report a teardown failure separately).
    #[test]
    fn run_docker_tears_down_unconditionally_after_a_test_failure() {
        let tmp = tempfile::tempdir().unwrap();
        let logs = tmp.path().join("logs");
        std::fs::create_dir_all(&logs).unwrap();
        let marker = tmp.path().join("torn-down");
        let plan = fixture_docker_plan(
            tmp.path(),
            "#!/bin/sh\necho SECTION 3 broke; exit 1\n",
            &format!("#!/bin/sh\ntouch {}\n", marker.display()),
        );

        let outcome = run_docker(&plan, &logs, true);
        assert!(!outcome.passed);
        assert_eq!(outcome.repos["stage"], "passed");
        assert_eq!(outcome.repos["deploy"], "passed");
        assert!(
            outcome.repos["test"].starts_with("failed"),
            "{:?}",
            outcome.repos
        );
        assert_eq!(outcome.repos["teardown"], "passed");
        assert!(
            marker.exists(),
            "teardown did not run after the test failed"
        );
    }

    /// A teardown failure is its own recorded failure — a passing test with
    /// a failing teardown still fails the tier, attributed to teardown.
    #[test]
    fn run_docker_records_a_teardown_failure_separately() {
        let tmp = tempfile::tempdir().unwrap();
        let logs = tmp.path().join("logs");
        std::fs::create_dir_all(&logs).unwrap();
        let plan = fixture_docker_plan(
            tmp.path(),
            "#!/bin/sh\necho all good\n",
            "#!/bin/sh\necho stuck volume; exit 1\n",
        );

        let outcome = run_docker(&plan, &logs, true);
        assert!(!outcome.passed, "a teardown failure must fail the tier");
        assert_eq!(outcome.repos["test"], "passed");
        assert!(
            outcome.repos["teardown"].starts_with("failed"),
            "{:?}",
            outcome.repos
        );
    }

    /// A staging failure skips deploy and test — nothing to run against —
    /// but the teardown still runs (compose down is safe when nothing is
    /// up, and a previous run's leftovers must not survive).
    #[test]
    fn run_docker_skips_deploy_and_test_after_a_stage_failure() {
        let tmp = tempfile::tempdir().unwrap();
        let logs = tmp.path().join("logs");
        std::fs::create_dir_all(&logs).unwrap();
        let marker = tmp.path().join("torn-down");
        let mut plan = fixture_docker_plan(
            tmp.path(),
            "#!/bin/sh\necho unreachable\n",
            &format!("#!/bin/sh\ntouch {}\n", marker.display()),
        );
        // Break staging: remove one staged binary from dist/.
        std::fs::remove_file(plan.dist.join("ph")).unwrap();
        plan.bin = tmp.path().join("demo2/dns-demo/bin");

        let outcome = run_docker(&plan, &logs, true);
        assert!(!outcome.passed);
        assert!(
            outcome.repos["stage"].starts_with("failed"),
            "{:?}",
            outcome.repos
        );
        assert!(
            outcome.repos["deploy"].starts_with("skipped"),
            "{:?}",
            outcome.repos
        );
        assert!(
            outcome.repos["test"].starts_with("skipped"),
            "{:?}",
            outcome.repos
        );
        assert!(
            marker.exists(),
            "teardown must run even after a stage failure"
        );
    }

    // -- the unit tier's plan (issue steps 1-2) --------------------------------

    /// Worktrees for the named recipe-table repositories, each in a fake
    /// directory named after its repository.
    fn fixture_worktrees(repos: &[&str]) -> Vec<(&'static recipes::Recipe, PathBuf)> {
        repos
            .iter()
            .map(|name| {
                let recipe = recipes::RECIPES
                    .iter()
                    .find(|recipe| recipe.repo == *name)
                    .expect("fixture names a recipe-table repository");
                (recipe, PathBuf::from(format!("/wt/{name}")))
            })
            .collect()
    }

    /// The plan for a repository whose recipe built something is `make test`
    /// in its worktree; `zl-zpr-visaservice` gets `make pregen ZPLC=<dist>/zplc`
    /// prepended, named `pregen` so a failure attributes to the step that
    /// proves compiler compatibility.
    #[test]
    fn unit_plan_runs_make_test_with_pregen_first_in_the_visa_service() {
        let worktrees = fixture_worktrees(&["zl-zpr-compiler", "zl-zpr-visaservice"]);
        let plans = unit_plan(recipes::RECIPES, &worktrees, Path::new("/build/dist"));
        assert_eq!(plans.len(), 2);

        let RepoPlan::Run { repo, steps, .. } = &plans[0] else {
            panic!("compiler must run, not skip: {:?}", plans[0]);
        };
        assert_eq!(repo, "zl-zpr-compiler");
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].name, "test");
        assert_eq!(steps[0].program, "make");
        assert_eq!(steps[0].args, ["test"]);

        let RepoPlan::Run {
            repo,
            worktree,
            steps,
        } = &plans[1]
        else {
            panic!("visa service must run, not skip: {:?}", plans[1]);
        };
        assert_eq!(repo, "zl-zpr-visaservice");
        assert_eq!(worktree, Path::new("/wt/zl-zpr-visaservice"));
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0].name, "pregen");
        assert_eq!(steps[0].program, "make");
        assert_eq!(steps[0].args, ["pregen", "ZPLC=/build/dist/zplc"]);
        assert_eq!(steps[1].name, "test");
        assert_eq!(steps[1].args, ["test"]);
    }

    /// A repository whose recipe builds nothing has no unit suite: it is
    /// planned as a skip with the reason, never omitted (approved Q2 on
    /// zipline#61 — `zl-zpr-demo`; the docker tier covers it).
    #[test]
    fn unit_plan_skips_a_repository_that_builds_nothing_with_a_reason() {
        let worktrees = fixture_worktrees(&["zl-zpr-demo"]);
        let plans = unit_plan(recipes::RECIPES, &worktrees, Path::new("/build/dist"));
        assert_eq!(plans.len(), 1);
        let RepoPlan::Skip { repo, reason } = &plans[0] else {
            panic!("demo must skip, not run: {:?}", plans[0]);
        };
        assert_eq!(repo, "zl-zpr-demo");
        assert!(reason.contains("no unit tests"), "{reason}");
        assert!(reason.contains("docker"), "{reason}");
    }

    /// The plan is in build order — the recipe table's order — whatever
    /// order the worktrees arrived in.
    #[test]
    fn unit_plan_is_in_build_order_regardless_of_input_order() {
        let worktrees = fixture_worktrees(&["zl-zpr-coredns", "zl-zpr-compiler"]);
        let plans = unit_plan(recipes::RECIPES, &worktrees, Path::new("/d"));
        let repos: Vec<&str> = plans
            .iter()
            .map(|plan| match plan {
                RepoPlan::Run { repo, .. } | RepoPlan::Skip { repo, .. } => repo.as_str(),
            })
            .collect();
        assert_eq!(repos, ["zl-zpr-compiler", "zl-zpr-coredns"]);
    }

    // -- the unit tier's runner (issue steps 1-2) ------------------------------

    /// A one-step run plan whose command is a shell fixture.
    fn fixture_plan(repo: &str, dir: &Path, name: &'static str, script: &str) -> RepoPlan {
        RepoPlan::Run {
            repo: repo.to_string(),
            worktree: dir.to_path_buf(),
            steps: vec![TierStep {
                name,
                program: "sh",
                args: vec!["-c".to_string(), script.to_string()],
            }],
        }
    }

    /// A passing sweep: every repository records `passed`, the tier passes,
    /// and each step's output landed in its log.
    #[test]
    fn run_unit_records_passes_and_writes_logs() {
        let tmp = tempfile::tempdir().unwrap();
        let logs = tmp.path().join("logs");
        std::fs::create_dir_all(&logs).unwrap();

        let plans = vec![fixture_plan("repo-a", tmp.path(), "test", "echo tested")];
        let outcome = run_unit(&plans, &logs, true);
        assert!(outcome.passed);
        assert_eq!(outcome.repos["repo-a"], "passed");
        let log = std::fs::read_to_string(logs.join("repo-a-test.log")).unwrap();
        assert!(log.contains("tested"), "{log}");
    }

    /// A failing repository fails the tier naming the failing step and the
    /// log path in its entry — and the sweep keeps going, so a later
    /// repository still runs and reports (issue step 2).
    #[test]
    fn run_unit_keeps_going_after_a_failure_and_names_the_step() {
        let tmp = tempfile::tempdir().unwrap();
        let logs = tmp.path().join("logs");
        std::fs::create_dir_all(&logs).unwrap();

        let plans = vec![
            fixture_plan("repo-bad", tmp.path(), "pregen", "echo mismatch; exit 1"),
            fixture_plan("repo-good", tmp.path(), "test", "echo fine"),
        ];
        let outcome = run_unit(&plans, &logs, true);
        assert!(!outcome.passed);
        let bad = &outcome.repos["repo-bad"];
        assert!(bad.starts_with("failed"), "{bad}");
        assert!(bad.contains("pregen"), "failing step not named: {bad}");
        assert!(bad.contains("repo-bad-pregen.log"), "log not named: {bad}");
        // The sweep kept going: the later repository ran and passed.
        assert_eq!(outcome.repos["repo-good"], "passed");
    }

    /// A failing first step stops that repository's later steps — `pregen`
    /// failing means its `make test` must not run against stale fixtures —
    /// while the sweep still proceeds to other repositories.
    #[test]
    fn run_unit_stops_a_repository_at_its_first_failing_step() {
        let tmp = tempfile::tempdir().unwrap();
        let logs = tmp.path().join("logs");
        std::fs::create_dir_all(&logs).unwrap();
        let marker = tmp.path().join("ran-anyway");

        let plans = vec![RepoPlan::Run {
            repo: "repo-vs".to_string(),
            worktree: tmp.path().to_path_buf(),
            steps: vec![
                TierStep {
                    name: "pregen",
                    program: "sh",
                    args: vec!["-c".to_string(), "exit 1".to_string()],
                },
                TierStep {
                    name: "test",
                    program: "sh",
                    args: vec!["-c".to_string(), format!("touch {}", marker.display())],
                },
            ],
        }];
        let outcome = run_unit(&plans, &logs, true);
        assert!(!outcome.passed);
        assert!(outcome.repos["repo-vs"].contains("pregen"));
        assert!(!marker.exists(), "test step ran after pregen failed");
    }

    /// A planned skip lands in the outcome as `skipped: <reason>` and does
    /// not fail the tier — visible, never omitted (approved Q2).
    #[test]
    fn run_unit_reports_skips_without_failing_the_tier() {
        let tmp = tempfile::tempdir().unwrap();
        let logs = tmp.path().join("logs");
        std::fs::create_dir_all(&logs).unwrap();

        let plans = vec![
            fixture_plan("repo-a", tmp.path(), "test", "true"),
            RepoPlan::Skip {
                repo: "zl-zpr-demo".to_string(),
                reason: "no unit tests (docker tier covers it)".to_string(),
            },
        ];
        let outcome = run_unit(&plans, &logs, true);
        assert!(outcome.passed, "a skip must not fail the tier");
        assert_eq!(
            outcome.repos["zl-zpr-demo"],
            "skipped: no unit tests (docker tier covers it)"
        );
    }
}
