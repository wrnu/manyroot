//! scripts/pi-prefix. Each test runs the script and its kickoff-template copy.

use crate::support::*;

/// pi-prefix (task 68675575fd58) checks node's tarball against its pin before it unpacks anything, and
/// builds only a new prefix. Stubs stand in for the platform and the download, so this runs anywhere:
/// `uname` says macOS on arm64, and `curl` writes bytes that aren't the pinned release.
#[test]
fn pi_prefix_refuses_a_node_tarball_that_doesnt_match_its_pin() {
    let tmp = tempfile::tempdir().unwrap();
    let stubs = tmp.path().join("stubs");
    std::fs::create_dir(&stubs).unwrap();
    write_executable(&stubs.join("uname"), "#!/bin/sh\n[ \"$1\" = -s ] && echo Darwin || echo arm64\n");
    write_executable(
        &stubs.join("curl"),
        "#!/bin/sh\nwhile [ $# -gt 0 ]; do [ \"$1\" = -o ] && out=$2; shift; done\nprintf 'not node' >\"$out\"\n",
    );
    let path = format!("{}:{}", stubs.display(), std::env::var("PATH").unwrap());
    let script = repo_root().join("scripts/pi-prefix");
    let prefix = tmp.path().join("prefix");
    let out = Command::new(&script).arg(&prefix).env("PATH", &path).output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{stderr}");
    assert!(stderr.contains("does not match its pinned sha256; not unpacking it"), "{stderr}");
    assert!(!prefix.join("bin").exists(), "nothing unpacked");

    // Only a new prefix: an existing directory is refused before anything is downloaded.
    std::fs::create_dir(tmp.path().join("there")).unwrap();
    let out = Command::new(&script).arg(tmp.path().join("there")).env("PATH", &path).output().unwrap();
    assert!(String::from_utf8_lossy(&out.stderr).contains("exists: pi-prefix builds a new one"));
    // Elsewhere than macOS on arm64, nothing at all. (Here, the real uname would let it build.)
    if !cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        let out = Command::new(&script).arg(&prefix).output().unwrap();
        assert!(String::from_utf8_lossy(&out.stderr).contains("builds for macOS on arm64 only"));
    }
}
