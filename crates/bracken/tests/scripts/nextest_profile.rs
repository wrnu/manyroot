//! `.config/nextest.toml`'s fast profile runs 4 tests at a time, and an explicit `-j` or `--test-threads`
//! overrides it (task c29555cbb440). Not a script, but what builders run before every pr claim.

use crate::support::*;

/// Each probe test notes how many probes are running as it starts (its own marker file included), holds
/// for a while so the others can start, then removes its marker.
const PROBE: &str = r#"
fn probe(name: &str) {
    let dir = std::path::PathBuf::from(std::env::var("PROBE_DIR").expect("PROBE_DIR is set"));
    let me = dir.join(format!("running-{name}"));
    std::fs::write(&me, "").expect("write the marker");
    let running = std::fs::read_dir(&dir)
        .expect("read the probe dir")
        .filter(|e| e.as_ref().expect("an entry").file_name().to_string_lossy().starts_with("running-"))
        .count();
    std::fs::write(dir.join(format!("seen-{name}")), running.to_string()).expect("write the count");
    std::thread::sleep(std::time::Duration::from_secs(2));
    std::fs::remove_file(&me).expect("remove the marker");
}
"#;

const TESTS: usize = 8;

/// The most probes that ran at once under `cargo nextest run <args>`, in a throwaway crate holding this
/// repository's `.config/nextest.toml`.
fn most_at_once(krate: &Path, target: &Path, args: &[&str]) -> usize {
    let probe_dir = tempfile::tempdir().unwrap();
    let out = Command::new("cargo")
        .args(["nextest", "run", "--no-fail-fast"])
        .args(args)
        .current_dir(krate)
        .env("CARGO_TARGET_DIR", target)
        .env("PROBE_DIR", probe_dir.path())
        // Only the arguments decide: nothing inherited from the nextest running this test.
        .env_remove("NEXTEST_PROFILE")
        .env_remove("NEXTEST_TEST_THREADS")
        .env_remove("NEXTEST_RETRIES")
        .output()
        .unwrap();
    assert!(out.status.success(), "cargo nextest run {args:?} failed: {}", text(&out));
    let seen: Vec<usize> = (0..TESTS)
        .map(|i| {
            let count = std::fs::read_to_string(probe_dir.path().join(format!("seen-t{i}")))
                .unwrap_or_else(|e| panic!("probe t{i} didn't run under {args:?}: {e}: {}", text(&out)));
            count.parse().unwrap()
        })
        .collect();
    seen.into_iter().max().unwrap()
}

#[test]
fn the_fast_profile_runs_four_tests_at_a_time_unless_told_otherwise() {
    let tmp = tempfile::tempdir().unwrap();
    let krate = tmp.path().join("probe");
    std::fs::create_dir_all(krate.join("tests")).unwrap();
    std::fs::create_dir_all(krate.join(".config")).unwrap();
    std::fs::write(
        krate.join("Cargo.toml"),
        "[package]\nname = \"probe\"\nversion = \"0.0.0\"\nedition = \"2024\"\n\n[workspace]\n",
    )
    .unwrap();
    std::fs::create_dir_all(krate.join("src")).unwrap();
    std::fs::write(krate.join("src/lib.rs"), "").unwrap();
    let tests: String = (0..TESTS).map(|i| format!("#[test]\nfn t{i}() {{ probe(\"t{i}\") }}\n")).collect();
    std::fs::write(krate.join("tests/probe.rs"), format!("{PROBE}\n{tests}")).unwrap();
    // This repository's config, all but the fast profile's filter, whose binary ids name this workspace's
    // test binaries: nextest refuses a filter naming none of the probe's.
    let config = std::fs::read_to_string(repo_root().join(".config/nextest.toml")).unwrap();
    let mut config: toml::Table = config.parse().unwrap();
    let fast = config.get_mut("profile").and_then(|p| p.get_mut("fast")).and_then(|f| f.as_table_mut());
    let fast = fast.expect("a fast profile");
    assert!(fast.remove("default-filter").is_some(), "the fast profile has a default-filter");
    std::fs::write(krate.join(".config/nextest.toml"), toml::to_string(&config).unwrap()).unwrap();
    let target = tmp.path().join("target");

    assert_eq!(most_at_once(&krate, &target, &["-P", "fast"]), 4, "the fast profile's default");
    assert_eq!(most_at_once(&krate, &target, &["-P", "fast", "-j", "2"]), 2, "-j overrides it");
    assert_eq!(most_at_once(&krate, &target, &["-P", "fast", "--test-threads", "6"]), 6, "--test-threads overrides it");
    // The default profile, which scripts/test-table and the counted run use, isn't capped at 4.
    let cpus = std::thread::available_parallelism().map_or(1, |n| n.get());
    if cpus > 4 {
        assert!(
            most_at_once(&krate, &target, &[]) > 4,
            "the default profile ran no more than 4 at once on {cpus} CPUs"
        );
    }
}
