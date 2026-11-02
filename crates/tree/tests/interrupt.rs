//! Finding 02ab829d2caf: a SIGINT, SIGTERM or SIGHUP to the process running a check stops the check as
//! its timeout does: a box or mac check's group gets a SIGTERM and its grace, so the box script's trap
//! deletes its box, and the run is an error with nothing of it recorded. The signal is the process's, so
//! this test has its binary to itself.

#![cfg(target_os = "macos")]

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use bracken_tree::check::{CheckError, Runner};
use bracken_tree::format::{Executor, ExecutorKind};
use bracken_tree::secrets::KnownKeys;
use bracken_tree::{CheckDefinition, ContentHash, Entry, Manifest, Mode};

// In a macos_only module: outside checks run only on this Mac, and scripts/test-table --split
// runs only macos_only tests here.
mod macos_only {
    use super::*;

    #[test]
    fn a_sigint_to_validate_gives_an_outside_check_its_sigterm_and_grace() {
        use std::os::unix::fs::PermissionsExt;
        let base = tempfile::tempdir().expect("a base directory");
        let root = base.path().canonicalize().expect("canonical");
        let vendor = root.join("vendor");
        std::fs::create_dir(&vendor).expect("mkdir vendor");
        let mut runner = Runner::new(&vendor);
        runner.base = root.clone();
        runner.outside_timeout = Duration::from_secs(60);
        runner.outside_grace = Duration::from_secs(10);

        // The script interrupts its parent, this process, as a Ctrl-C would, then waits to be stopped.
        let marker = root.join("cleaned");
        let script = format!(
            "#!/bin/bash\ntrap 'touch {}; exit 143' TERM\nkill -INT $PPID\nsleep 30 &\nwait\n",
            marker.display()
        );
        let dir = root.join("runner");
        std::fs::create_dir(&dir).expect("mkdir runner");
        std::fs::write(dir.join("run.sh"), &script).expect("write the script");
        std::fs::set_permissions(dir.join("run.sh"), std::fs::Permissions::from_mode(0o755)).expect("chmod");
        let executor = Executor {
            kind: ExecutorKind::Mac,
            script: "run.sh".to_owned(),
            script_hash: ContentHash::of(script.as_bytes()),
        };
        let argv = vec!["mac".to_owned()];
        let check = CheckDefinition { name: "mac", argv: &argv, executor: Some(&executor) };
        let bytes = b"one\n";
        let manifest = Manifest::new(vec![Entry {
            path: "a.txt".to_owned(),
            mode: Mode::File,
            hash: ContentHash::of(bytes),
            size: bytes.len() as u64,
        }]);
        let blobs = BTreeMap::from([(ContentHash::of(bytes), bytes.to_vec())]);

        let started = Instant::now();
        let err = runner
            .run_outside(&check, &dir, &manifest, |h| blobs.get(h).cloned(), &KnownKeys::new(), &vec![], &vec![], &[])
            .expect_err("interrupted");
        assert!(matches!(err, CheckError::Interrupted(2)), "{err}");
        assert!(marker.exists(), "the script's TERM trap didn't run");
        assert!(started.elapsed() < Duration::from_secs(30), "it waited out the check: {:?}", started.elapsed());
    }
}
