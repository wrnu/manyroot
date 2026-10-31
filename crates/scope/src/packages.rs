//! Which workspace packages a change affects (task de42fa5c776d, owner decision 73f2ba4c08e5): guppy's
//! determinator, over the package graphs of the two commits as `cargo metadata` gives them, and the rules
//! in `.config/determinator.toml` for paths outside the crates. `scripts/code-scope packages` runs it,
//! through `bracken-scope packages`, for the light counted run's scope, in `scripts/verify-seq --light` and
//! `scripts/merge` alike, after dropping the files changed only in comments.
//!
//! The rules are compiled in, so the tool a checkout builds judges by that checkout's rules: `merge` runs
//! main's, never the head's own.

use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail, ensure};
use camino::Utf8Path;
use determinator::Determinator;
use determinator::rules::DeterminatorRules;
use guppy::CargoMetadata;
use guppy::graph::{DependencyDirection, PackageGraph};

/// The rules file, as this build of the tool was compiled with it.
pub const RULES_TOML: &str = include_str!("../../../.config/determinator.toml");

/// The packages whose tests run another package's binary, which cargo's graph doesn't show: each binary's
/// package, and the packages that run it. When a change affects the binary's package through cargo's
/// graph (a path in it or in any crate it depends on, or a manifest), they're affected too; a rule's
/// marking it isn't enough, since docs and scripts mark bracken for its tests, not its build. The rules
/// file can't say this: the determinator's package rules follow every way a package is affected.
pub const RUN_BINARY: &[(&str, &[&str])] = &[("bracken", &["bracken-demo", "bracken-experiment"])];

/// The rules in [`RULES_TOML`].
pub fn rules() -> Result<DeterminatorRules> {
    DeterminatorRules::parse(RULES_TOML).context("parsing .config/determinator.toml")
}

/// The package graph of `commit` in the repository at `repo`: its tracked files (without the Ground,
/// which no manifest reads) unpacked in a throwaway directory, and `cargo metadata --offline --locked`
/// there, so nothing is fetched and the lock file is taken as it is. It's filtered to this host's
/// platform: a build fetches only its own platform's crates, and without the filter cargo metadata needs
/// every platform's (a Linux box lacks Windows' anstyle-wincon). The workspace's packages and the edges
/// between them hold on every platform here, so the filter loses nothing the determinator reads; the test
/// `no_edge_between_workspace_packages_is_platform_specific` keeps that so (task 7000db5f8726).
pub fn graph_at(repo: &Path, commit: &str) -> Result<PackageGraph> {
    let dir = tempfile::tempdir().context("making a directory to unpack it in")?;
    let mut archive = Command::new("git")
        .args(["-C"])
        .arg(repo)
        .args(["archive", "--format=tar", commit, "--", ".", ":(exclude).bracken"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("running git archive")?;
    let tar = Command::new("tar")
        .args(["-x", "-f", "-", "-C"])
        .arg(dir.path())
        .stdin(archive.stdout.take().expect("piped"))
        .output()
        .context("running tar")?;
    let archived = archive.wait_with_output().context("waiting for git archive")?;
    ensure!(archived.status.success(), "git archive {commit} failed: {}", String::from_utf8_lossy(&archived.stderr));
    ensure!(tar.status.success(), "unpacking {commit} failed: {}", String::from_utf8_lossy(&tar.stderr));
    let manifest = dir.path().join("Cargo.toml");
    ensure!(manifest.exists(), "{commit} has no Cargo.toml at its root");
    let out = Command::new("cargo")
        .args(["metadata", "--offline", "--locked", "--format-version", "1", "--filter-platform"])
        .arg(host()?)
        .arg("--manifest-path")
        .arg(&manifest)
        .stdin(Stdio::null())
        .output()
        .context("running cargo metadata")?;
    if !out.status.success() {
        bail!("cargo metadata for {commit} failed: {}", String::from_utf8_lossy(&out.stderr));
    }
    let json = String::from_utf8(out.stdout).context("cargo metadata printed something not UTF-8")?;
    let metadata = CargoMetadata::parse_json(&json).context("parsing cargo metadata")?;
    metadata.build_graph().with_context(|| format!("building the package graph of {commit}"))
}

/// This host's target triple, as rustc names it: `$RUSTC` if it names one, as cargo takes it, else `rustc`.
fn host() -> Result<String> {
    let rustc = std::env::var_os("RUSTC").filter(|r| !r.is_empty()).unwrap_or_else(|| "rustc".into());
    let shown = rustc.to_string_lossy().into_owned();
    let out = Command::new(&rustc)
        .arg("-vV")
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("running {shown} -vV"))?;
    ensure!(out.status.success(), "{shown} -vV failed: {}", String::from_utf8_lossy(&out.stderr));
    let text = String::from_utf8_lossy(&out.stdout);
    let host =
        text.lines().find_map(|l| l.strip_prefix("host: ")).with_context(|| format!("{shown} -vV named no host"))?;
    Ok(host.trim().to_owned())
}

/// The workspace packages that the change from `old` to `new`, through `paths`, affects, by name and
/// sorted: the determinator's, with `rules`, and the packages [`RUN_BINARY`] adds. A path no rule names
/// and no crate holds affects them all.
pub fn affected<'p>(
    old: &PackageGraph,
    new: &PackageGraph,
    rules: &DeterminatorRules,
    paths: impl IntoIterator<Item = &'p str>,
) -> Result<Vec<String>> {
    let paths: Vec<&str> = paths.into_iter().collect();
    let rules = for_workspace(rules, new);
    let mut determinator = Determinator::new(old, new);
    determinator.set_rules(&rules).context("resolving the rules against the head's workspace")?;
    determinator.add_changed_paths(paths.iter().map(|p| Utf8Path::new(*p)));
    let mut names = workspace_names(determinator.compute().affected_set);
    // The binaries' builds: the same change through cargo's graph alone, the paths inside the crates and
    // no rules.
    let members: Vec<&Utf8Path> = new.workspace().iter_by_path().map(|(path, _)| path).collect();
    let in_crates = paths.iter().map(|p| Utf8Path::new(*p)).filter(|p| members.iter().any(|m| p.starts_with(m)));
    let no_rules = DeterminatorRules::parse("use-default-rules = false").expect("a rules file with no rules parses");
    let mut builds = Determinator::new(old, new);
    builds.set_rules(&no_rules).expect("no rules name no package");
    builds.add_changed_paths(in_crates);
    let built = workspace_names(builds.compute().affected_set);
    let workspace = new.workspace();
    for (binary, runners) in RUN_BINARY {
        if built.iter().any(|n| n == binary) {
            names.extend(runners.iter().filter(|n| workspace.contains_name(n)).map(|n| (*n).to_owned()));
        }
    }
    names.sort();
    names.dedup();
    Ok(names)
}

/// The workspace packages in `set`, by name.
fn workspace_names(set: guppy::graph::PackageSet<'_>) -> Vec<String> {
    set.packages(DependencyDirection::Forward).filter(|p| p.in_workspace()).map(|p| p.name().to_owned()).collect()
}

/// `rules` without the package names `graph`'s workspace lacks, which the determinator refuses: a head
/// from before a package was added, or after one was removed, can't test it. A rule then marks the rest.
fn for_workspace(rules: &DeterminatorRules, graph: &PackageGraph) -> DeterminatorRules {
    use determinator::rules::DeterminatorMarkChanged;
    let workspace = graph.workspace();
    let keep = |names: &DeterminatorMarkChanged| match names {
        DeterminatorMarkChanged::Packages(names) => DeterminatorMarkChanged::Packages(
            names.iter().filter(|n| workspace.contains_name(n.as_str())).cloned().collect(),
        ),
        DeterminatorMarkChanged::All => DeterminatorMarkChanged::All,
    };
    let mut rules = rules.clone();
    for rule in &mut rules.path_rules {
        rule.mark_changed = keep(&rule.mark_changed);
    }
    for rule in &mut rules.package_rules {
        rule.on_affected.retain(|n| workspace.contains_name(n.as_str()));
        rule.mark_changed = keep(&rule.mark_changed);
    }
    rules
}

/// Every workspace package of `graph`, by name and sorted.
pub fn all(graph: &PackageGraph) -> Vec<String> {
    let mut names: Vec<String> = graph.workspace().iter().map(|p| p.name().to_owned()).collect();
    names.sort();
    names
}
