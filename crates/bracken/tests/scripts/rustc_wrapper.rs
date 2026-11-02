//! scripts/rustc-wrapper. Each test runs the script and its kickoff-template copy.

use crate::support::*;

/// Task 23e73664dc45 (owner decision 8ac9b0239d90): scripts/rustc-wrapper, cargo's rustc wrapper. Without
/// sccache on PATH it runs the compiler itself, with the caller's environment as it is. With sccache, it
/// runs sccache with only what a compile needs: cargo's variables and the names it lists, never one that
/// looks like a secret or isn't listed, with SCCACHE_CACHE_SIZE at 15G unless the caller set it, and
/// sccache's own log settings dropped. Cargo's names that aren't shell identifiers
/// (`CARGO_BIN_EXE_<bin-with-hyphens>`) pass too: bash 5 leaves them out of its own list of variables. Clippy
/// skips sccache. The wrapper prints nothing of its own. It's run under every bash on PATH and under
/// /bin/bash (macOS's 3.2).
#[test]
fn rustc_wrapper_uses_sccache_when_installed_and_gives_it_no_secrets() {
    let tmp = tempfile::tempdir().unwrap();
    let (bin, stubs, seen) = (tmp.path().join("bin"), tmp.path().join("stubs"), tmp.path().join("seen"));
    std::fs::create_dir(&bin).unwrap();
    std::fs::create_dir(&stubs).unwrap();
    // A compiler, a clippy and an sccache that each record how they were run, and with what variables. In
    // bash, not sh: Linux's dash drops a name that isn't a shell identifier (CARGO_BIN_EXE_a-b) before its
    // child runs, so the record would miss what the wrapper passed.
    let record = |who: &str| {
        format!(
            "#!/bin/bash\n{{ echo {who} \"$@\"; IFS= read -r j 2>/dev/null <&3; echo \"fd3=$j\"; /usr/bin/env | sort; }} >\"{}\"\n",
            seen.display()
        )
    };
    write_executable(&bin.join("rustc"), &record("rustc"));
    write_executable(&bin.join("clippy-driver"), &record("clippy-driver"));
    write_executable(&stubs.join("sccache"), &record("sccache"));
    let wrapper = repo_root().join("scripts/rustc-wrapper");
    let mut bashes = vec![PathBuf::from("/bin/bash")];
    for dir in std::env::split_paths(&std::env::var_os("PATH").unwrap()) {
        if let Ok(bash) = dir.join("bash").canonicalize()
            && !bashes.iter().any(|b| b.canonicalize().ok().as_ref() == Some(&bash))
        {
            bashes.push(bash);
        }
    }
    for bash in bashes.iter().filter(|b| b.exists()) {
        rustc_wrapper_under(bash, &wrapper, &bin, &stubs, &seen, tmp.path());
    }
}

fn rustc_wrapper_under(bash: &Path, wrapper: &Path, bin: &Path, stubs: &Path, seen: &Path, home: &Path) {
    let compile = |path: &str, compiler: &str, extra: &[(&str, &str)]| -> String {
        let mut command = Command::new(bash);
        command
            .arg(wrapper)
            .arg(bin.join(compiler))
            .args(["--crate-name", "x"])
            .env_clear()
            .env("PATH", path)
            .env("HOME", home)
            .env("CARGO_PKG_NAME", "x")
            .env("CARGO_BIN_EXE_a-b", "/bin/a-b")
            .env("CARGO_PKG_AUTHORS", "Ann")
            .env("CARGO_BIN_EXE_tokenizer", "/bin/tokenizer")
            .env("CARGO_FORGE_TOKEN", "s4")
            .env("RUST_API_KEY", "s5")
            .env("RUSTC_BOOTSTRAP", "1")
            .env("SCCACHE_SERVER_PORT", "4226")
            .env("SCCACHE_RECACHE", "1")
            .env("OUT_DIR", "/out")
            .env("UNLISTED", "u")
            .env("ANTHROPIC_AUTH_TOKEN", "s1")
            .env("CARGO_REGISTRY_TOKEN", "s2")
            .env("OPENROUTER_API_KEY", "s3")
            .env("SCCACHE_LOG", "debug");
        command.envs(extra.iter().copied());
        let out = command.output().unwrap();
        assert!(out.status.success(), "{bash:?}: {}", String::from_utf8_lossy(&out.stderr));
        assert!(out.stdout.is_empty() && out.stderr.is_empty(), "{bash:?}: the wrapper prints nothing of its own");
        std::fs::read_to_string(seen).unwrap()
    };
    let has = |seen: &str, line: &str| seen.lines().any(|l| l == line);

    // No sccache: the compiler itself, with the environment untouched.
    let base = "/usr/bin:/bin";
    let s = compile(base, "rustc", &[]);
    assert!(s.starts_with("rustc --crate-name x\n"), "{s}");
    assert!(has(&s, "ANTHROPIC_AUTH_TOKEN=s1") && has(&s, "UNLISTED=u"), "{s}");
    assert!(!s.contains("SCCACHE_CACHE_SIZE"), "{s}");

    // With sccache: sccache runs the compiler, and gets only what a compile needs.
    let with = format!("{}:{base}", stubs.display());
    let s = compile(&with, "rustc", &[]);
    assert!(s.starts_with(&format!("sccache {} --crate-name x\n", bin.join("rustc").display())), "{s}");
    let path = format!("PATH={with}");
    let uid = String::from_utf8(Command::new("id").arg("-u").output().unwrap().stdout).unwrap();
    let socket = format!("SCCACHE_SERVER_UDS=/tmp/bracken-sccache-{}/server.sock", uid.trim());
    for kept in [
        "CARGO_PKG_NAME=x",
        "CARGO_BIN_EXE_a-b=/bin/a-b",
        // Cargo's own, though AUTH and TOKEN are in their names (finding 58223509a78e).
        "CARGO_PKG_AUTHORS=Ann",
        "CARGO_BIN_EXE_tokenizer=/bin/tokenizer",
        // A counted run's, from test-table.
        "SCCACHE_RECACHE=1",
        "RUSTC_BOOTSTRAP=1",
        // The server on a unix socket in a 0700 directory of this user's, never a port (finding 292898be0ce0).
        &socket,
        "OUT_DIR=/out",
        &path,
        "SCCACHE_CACHE_SIZE=15G",
        "SCCACHE_IGNORE_SERVER_IO_ERROR=1",
    ] {
        assert!(has(&s, kept), "{bash:?}: {kept}: {s}");
    }
    for gone in [
        "ANTHROPIC_AUTH_TOKEN",
        "CARGO_REGISTRY_TOKEN",
        "OPENROUTER_API_KEY",
        "SCCACHE_LOG",
        "SCCACHE_SERVER_PORT",
        "UNLISTED",
        // Not cargo's own, though they start like them (finding 289f76c488af).
        "CARGO_FORGE_TOKEN",
        "RUST_API_KEY",
    ] {
        assert!(!s.contains(gone), "{bash:?}: {gone}: {s}");
    }
    // A cache size the caller set stays.
    assert!(has(&compile(&with, "rustc", &[("SCCACHE_CACHE_SIZE", "3G")]), "SCCACHE_CACHE_SIZE=3G"));
    // Clippy skips the cache, so a lint is never answered from one made under other flags.
    let s = compile(&with, "clippy-driver", &[]);
    assert!(s.starts_with("clippy-driver --crate-name x\n"), "{s}");
    assert!(has(&s, "ANTHROPIC_AUTH_TOKEN=s1"), "{s}");

    // sccache failing to reach or start its server exits 2 before the compiler runs: the wrapper runs the
    // compiler itself (finding 20a5ceea1005), as it does for any exit 2 (findings 5e48b5f2a88e, 38094ff61ec4)
    // but one. sccache also exits 2 when a compile it ran was killed by a signal, its note last on stdout:
    // that's passed on, not rerun (finding ec492ce1a7d1), whatever the compiler printed on stderr. Without the
    // copy of stdout (here a mktemp that fails), every exit 2 is rerun (finding 2bc99ca95dc3). Any other exit,
    // the compile's own, is passed on. sccache's output passes on, and an open fd 3, as cargo's jobserver is,
    // reaches sccache and the compiler (finding c3665d850bc9).
    let failing = stubs.parent().unwrap().join("failing");
    let nocopy = stubs.parent().unwrap().join("nocopy");
    std::fs::create_dir_all(&failing).unwrap();
    std::fs::create_dir_all(&nocopy).unwrap();
    write_executable(&nocopy.join("mktemp"), "#!/bin/bash\nexit 1\n");
    let jobserver = stubs.parent().unwrap().join("jobserver");
    // A line for each stub that reads it: sccache's, then the compiler's.
    std::fs::write(&jobserver, "jobserver\n".repeat(4)).unwrap();
    let startup = "echo 'sccache: error: Server startup failed: No such file or directory (os error 2)' >&2\n\
                   echo 'Run with SCCACHE_LOG=debug SCCACHE_NO_DAEMON=1 to get more information' >&2";
    let denied = "echo 'sccache: error: Operation not permitted (os error 1)' >&2";
    let dropped = "echo 'sccache: error: failed to execute compile' >&2\n\
                   echo 'sccache: caused by: Failed to send data to or receive data from server' >&2";
    let killed = "echo 'sccache: Compile terminated by signal 11'";
    let decoy_killed = "echo 'sccache: error: Operation not permitted (os error 1)' >&2\n\
                        echo 'sccache: Compile terminated by signal 11'";
    let not_last = "echo 'sccache: Compile terminated by signal 11'\necho 'later'";
    // The compiler's stdout ended without a newline, so sccache's note ends its last line (finding 19b295a1bb9a).
    let after_partial = "printf 'partial'; echo 'sccache: Compile terminated by signal 11'";
    for (code, says, copy, ran) in [
        (2, startup, true, "rustc"),
        (2, denied, true, "rustc"),
        (2, dropped, true, "rustc"),
        (2, "", true, "rustc"),
        (2, not_last, true, "rustc"),
        (2, killed, true, "sccache"),
        (2, decoy_killed, true, "sccache"),
        (2, after_partial, true, "sccache"),
        (2, denied, false, "rustc"),
        (2, killed, false, "rustc"),
        (1, "", true, "sccache"),
        (101, "", true, "sccache"),
    ] {
        write_executable(
            &failing.join("sccache"),
            &format!(
                "#!/bin/bash\n{{ echo sccache \"$@\"; IFS= read -r j 2>/dev/null <&3; echo \"fd3=$j\"; }} >\"{}\"\n{says}\nexit {code}\n",
                seen.display()
            ),
        );
        let path = if copy {
            format!("{}:{base}", failing.display())
        } else {
            format!("{}:{}:{base}", nocopy.display(), failing.display())
        };
        let mut command = Command::new(bash);
        command
            .args(["-c", "exec 3<\"$0\"; exec \"$@\"", jobserver.to_str().unwrap(), bash.to_str().unwrap()])
            .arg(wrapper)
            .arg(bin.join("rustc"))
            .args(["--crate-name", "x"])
            .env_clear()
            .env("PATH", path)
            .env("HOME", home);
        let out = command.output().unwrap();
        let case = format!("{bash:?}: sccache exit {code}, {says:?}, copy {copy}");
        let s = std::fs::read_to_string(seen).unwrap();
        assert!(s.starts_with(&format!("{ran} ")), "{case}: {s}");
        assert!(s.lines().any(|l| l == "fd3=jobserver"), "{case}: fd 3 didn't reach {ran}: {s}");
        let want = if ran == "rustc" { 0 } else { code };
        assert_eq!(out.status.code(), Some(want), "{case}");
        let (stdout, stderr) = (String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        let shown = format!("{stdout}{stderr}");
        for line in says.lines().filter_map(|l| l.split('\'').nth(1)) {
            assert!(shown.contains(line), "{case}: sccache's {line:?} didn't pass on: {shown}");
        }
        assert!(stdout.contains("signal") == says.contains("signal"), "{case}: stdout stays stdout: {shown}");
    }
}
