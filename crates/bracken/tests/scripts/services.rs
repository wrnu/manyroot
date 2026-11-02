//! scripts/services. Each test runs the script and its kickoff-template copy.

use crate::support::*;

/// `services` (owner decision `6fed6a4c8af0`), without touching this machine's launchd: `launchctl`
/// is a stub that logs its calls and has nothing loaded, and HOME is a throwaway directory. The dry
/// run prints two valid plists that keep alive what scripts/team-server run and scripts/bk view start,
/// log under the team directory, and hold no token; it writes nothing. With --netbird the one view
/// listens on 127.0.0.1 and the NetBird address (decision 5863740ca157). uninstall removes exactly the
/// two plists, and only them.
#[test]
fn services_dry_run_holds_no_secret_and_uninstall_removes_only_its_plists() {
    for scripts in SCRIPTS {
        let copy = |name: &str| repo_root().join(scripts).join(name);
        let files = [("scripts/services", copy("services")), ("scripts/team-server", copy("team-server"))];
        let files: Vec<(&str, &Path)> = files.iter().map(|(to, from)| (*to, from.as_path())).collect();
        let (tmp, dir) = repo(&files, ".bracken/\n.bracken-team\n", |_| {});
        // A team directory as team-server setup leaves it, with a token whose value must never show.
        let team = tmp.path().join("team & <co>");
        std::fs::create_dir_all(team.join("tokens")).unwrap();
        std::fs::write(team.join("url"), "http://127.0.0.1:7391\n").unwrap();
        let secret = "5ec7e75ec7e75ec7e75ec7e75ec7e75ec7e75ec7e75ec7e75ec7e75ec7e75ec7";
        std::fs::write(team.join("tokens/alice"), secret).unwrap();
        std::fs::write(dir.join(".bracken-team"), format!("{}\n", team.display())).unwrap();
        let home = tmp.path().join("home");
        let stubs = tmp.path().join("stubs");
        std::fs::create_dir_all(&stubs).unwrap();
        write_executable(&stubs.join("launchctl"), "#!/bin/sh\necho \"$*\" >>\"$T/launchctl.log\"\nexit 1\n");
        let path = format!("{}:{}", stubs.display(), std::env::var("PATH").unwrap());
        let services = |args: &[&str]| {
            let mut c = command(&dir, "scripts/services", args);
            c.env("HOME", &home).env("PATH", &path).env("T", tmp.path()).env("BRACKEN_TOKEN", secret);
            c.output().unwrap()
        };

        let out = services(&["install", "--dry-run", "--view-port", "7399"]);
        let text = String::from_utf8(out.stdout).unwrap();
        assert!(out.status.success(), "{scripts}: {}", String::from_utf8_lossy(&out.stderr));
        assert!(!text.contains(secret) && !text.to_lowercase().contains("token"), "{scripts}: {text}");
        let plists: Vec<&str> = text.split("\n# ").collect();
        assert_eq!(plists.len(), 2, "{scripts}: {text}");
        assert!(
            !text.contains("view-local") && !text.contains("--bind"),
            "{scripts}: without --netbird, the view on 127.0.0.1 only: {text}"
        );
        for (plist, label, args) in [
            (plists[0], "jx.rs.bracken.team-server", vec!["team-server", "run"]),
            (plists[1], "jx.rs.bracken.view", vec!["bk", "view", "--port", "7399"]),
        ] {
            let body = &plist[plist.find("<?xml").expect("a plist")..];
            assert!(body.contains(&format!("<string>{label}</string>")), "{scripts}: {body}");
            for a in args {
                assert!(body.contains(a), "{scripts}: {label} lacks {a}");
            }
            let keep_alive = "<key>KeepAlive</key>\n  <dict>\n    <key>SuccessfulExit</key>\n    <false/>\n  </dict>";
            for key in ["<key>RunAtLoad</key>\n  <true/>", keep_alive] {
                assert!(body.contains(key), "{scripts}: {label} lacks {key}");
            }
            assert!(
                body.contains("team &amp; &lt;co&gt;/launchd-"),
                "{scripts}: logs under the team directory, escaped"
            );
            if std::path::Path::new("/usr/bin/plutil").exists() {
                let file = tmp.path().join(format!("{label}.plist"));
                std::fs::write(&file, body).unwrap();
                let lint = Command::new("/usr/bin/plutil").arg("-lint").arg(&file).output().unwrap();
                assert!(lint.status.success(), "{scripts}: {}", String::from_utf8_lossy(&lint.stdout));
            }
        }
        assert!(!home.join("Library").exists(), "{scripts}: the dry run wrote something");

        // No address can be named (finding bbd29eb576fc): --netbird alone moves the view, to the address
        // `netbird status` gives, and only a connected NetBird address in 100.64.0.0/10.
        for old in [&["--bind", "192.168.1.20", "--host", "x"][..], &["--bind", "100.74.240.73"]] {
            let args: Vec<&str> = ["install", "--dry-run"].iter().chain(old).copied().collect();
            assert_eq!(services(&args).status.code(), Some(2), "{scripts}: {old:?} taken");
        }
        let viewer = tmp.path().join("bracken-with-bind");
        write_executable(
            &viewer,
            "#!/bin/sh
[ \"$1 $2\" = 'view --help' ] && echo '      --bind <IP>  Repeatable: at most one loopback and one other'\nexit 0\n",
        );
        let netbird = |management: &str, ip: &str, fqdn: &str| {
            let status =
                format!("Management: {management}\nFQDN: {fqdn}\nNetBird IP: {ip}/16\nPeers count: 3/4 Connected\n");
            std::fs::write(tmp.path().join("netbird-status"), status).unwrap();
            let mut c = command(&dir, "scripts/services", &["install", "--dry-run", "--netbird"]);
            c.env("HOME", &home).env("PATH", &path).env("T", tmp.path()).env("BRACKEN_BIN", &viewer);
            c.output().unwrap()
        };
        write_executable(&stubs.join("netbird"), "#!/bin/sh\n[ \"$1\" = status ] && cat \"$T/netbird-status\"\n");
        let out = netbird("Connected", "100.74.240.73", "warren-mbp.jx.rs");
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(out.status.success(), "{scripts}: {}", String::from_utf8_lossy(&out.stderr));
        // One view, one process, on both: 127.0.0.1 and the NetBird IP, with the name for the NetBird one.
        let plists: Vec<&str> = text.split("\n# ").collect();
        assert_eq!(plists.len(), 2, "{scripts}: one view agent: {text}");
        assert!(!text.contains("view-local"), "{scripts}: {text}");
        let both = "<string>view</string>\n    <string>--port</string>\n    <string>7392</string>\n    <string>--bind</string>\n    <string>127.0.0.1</string>\n    <string>--bind</string>\n    <string>100.74.240.73</string>\n    <string>--host</string>\n    <string>warren-mbp.jx.rs</string>\n  </array>";
        assert!(
            plists[1].contains("<string>jx.rs.bracken.view</string>") && plists[1].contains(both),
            "{scripts}: {text}"
        );
        if std::path::Path::new("/usr/bin/plutil").exists() {
            let file = tmp.path().join("view-netbird.plist");
            std::fs::write(&file, &plists[1][plists[1].find("<?xml").expect("a plist")..]).unwrap();
            let lint = Command::new("/usr/bin/plutil").arg("-lint").arg(&file).output().unwrap();
            assert!(lint.status.success(), "{scripts}: {}", String::from_utf8_lossy(&lint.stdout));
        }
        assert!(!home.join("Library").exists(), "{scripts}: the dry run wrote something");
        for (management, ip, fqdn, why) in [
            ("Disconnected", "100.74.240.73", "warren-mbp.jx.rs", "isn't connected"),
            ("Connected", "192.168.1.20", "warren-mbp.jx.rs", "isn't a NetBird address"),
            ("Connected", "100.128.0.1", "warren-mbp.jx.rs", "isn't a NetBird address"),
            ("Connected", "100.63.255.1", "warren-mbp.jx.rs", "isn't a NetBird address"),
            ("Connected", "100.74.240.300", "warren-mbp.jx.rs", "isn't a NetBird address"),
            // Leading zeros, which an inet_aton-style parser reads as octal (finding 81e8ef2f49f6).
            ("Connected", "100.064.000.001", "warren-mbp.jx.rs", "isn't a NetBird address"),
            ("Connected", "100.74.240.073", "warren-mbp.jx.rs", "isn't a NetBird address"),
            // Exactly one Management line, and it says Connected.
            ("Disconnected\nManagement: Connected", "100.74.240.73", "warren-mbp.jx.rs", "isn't connected"),
            ("Connected\nManagement: Disconnected", "100.74.240.73", "warren-mbp.jx.rs", "isn't connected"),
            ("Connected (cached)", "100.74.240.73", "warren-mbp.jx.rs", "isn't connected"),
            ("Connected", "100.74.240.73", "evil host; rm", "isn't a host name"),
        ] {
            let out = netbird(management, ip, fqdn);
            let stderr = String::from_utf8_lossy(&out.stderr);
            assert!(!out.status.success() && stderr.contains(why), "{scripts}: {ip} {fqdn}: {stderr}");
            assert!(out.stdout.is_empty(), "{scripts}: printed a plist for {ip}");
        }
        // The real view (through the test's wrapper) can listen on two addresses, so --netbird goes ahead.
        let mut c = command(&dir, "scripts/services", &["install", "--dry-run", "--netbird"]);
        c.env("HOME", &home).env("PATH", &path).env("T", tmp.path());
        netbird("Connected", "100.74.240.73", "warren-mbp.jx.rs");
        let out = c.output().unwrap();
        let bindless =
            std::process::Command::new(env!("CARGO_BIN_EXE_bracken")).args(["view", "--help"]).output().unwrap();
        // services tells a view that can listen on two addresses by this phrase in its help.
        assert!(
            String::from_utf8_lossy(&bindless.stdout).contains("at most one loopback"),
            "{scripts}: bracken view's --bind help lost the phrase services checks for"
        );
        assert!(out.status.success(), "{scripts}: {}", String::from_utf8_lossy(&out.stderr));
        // A view from before decision 5863740ca157, whose --bind doesn't repeat: --netbird refuses.
        let single = tmp.path().join("bracken-single-bind");
        write_executable(&single, "#!/bin/sh\n[ \"$1 $2\" = 'view --help' ] && echo '      --bind <IP>'\nexit 0\n");
        let mut c = command(&dir, "scripts/services", &["install", "--dry-run", "--netbird"]);
        c.env("HOME", &home).env("PATH", &path).env("T", tmp.path()).env("BRACKEN_BIN", &single);
        let out = c.output().unwrap();
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(!out.status.success() && stderr.contains("two addresses"), "{scripts}: {stderr}");

        // A foreign server on the view's port at a wildcard address (finding a67053e1e3d2): lsof lists it
        // for the port, but not for 127.0.0.1:P or the NetBird IP, as the real lsof does. Install refuses,
        // naming it, and changes nothing, with --netbird or without.
        write_executable(
            &stubs.join("lsof"),
            "#!/bin/sh\ncase \" $* \" in *' -iTCP:7399 '*) echo 4242 ;; esac\nexit 0\n",
        );
        for netbird_flag in [&[][..], &["--netbird"]] {
            let args: Vec<&str> = ["install", "--view-port", "7399"].iter().chain(netbird_flag).copied().collect();
            let mut c = command(&dir, "scripts/services", &args);
            c.env("HOME", &home).env("PATH", &path).env("T", tmp.path()).env("BRACKEN_BIN", &viewer);
            let out = c.output().unwrap();
            let stderr = String::from_utf8_lossy(&out.stderr);
            assert!(!out.status.success(), "{scripts} {netbird_flag:?}: a wildcard holder wasn't refused: {stderr}");
            assert!(stderr.contains("port 7399 is taken (pid 4242)"), "{scripts} {netbird_flag:?}: {stderr}");
            assert!(!home.join("Library").exists(), "{scripts}: refused, but wrote something");
            let calls = std::fs::read_to_string(tmp.path().join("launchctl.log")).unwrap_or_default();
            assert!(!calls.contains("bootstrap") && !calls.contains("bootout"), "{scripts}: {calls}");
        }
        // status names a wildcard holder blocking the view's address, as real lsof's -F output gives it.
        write_executable(
            &stubs.join("lsof"),
            "#!/bin/sh\ncase \" $* \" in *' -iTCP:7392 '*-Fpn*) printf 'p4242\\nf3\\nn*:7392\\n' ;; esac\nexit 0\n",
        );
        let out = services(&["status"]);
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(out.status.success(), "{scripts}: {}", String::from_utf8_lossy(&out.stderr));
        assert!(
            text.contains("jx.rs.bracken.view: not loaded, nothing on 127.0.0.1:7392, held by pid 4242 on *:7392"),
            "{scripts}: {text}"
        );
        std::fs::remove_file(stubs.join("lsof")).unwrap();

        let agents = home.join("Library/LaunchAgents");
        std::fs::create_dir_all(&agents).unwrap();
        for name in ["jx.rs.bracken.team-server.plist", "jx.rs.bracken.view.plist", "someone.else.plist"] {
            std::fs::write(agents.join(name), "x").unwrap();
        }
        let out = services(&["uninstall"]);
        assert!(out.status.success(), "{scripts}: {}", String::from_utf8_lossy(&out.stderr));
        let left: Vec<_> = std::fs::read_dir(&agents).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(left, ["someone.else.plist"], "{scripts}");
        assert!(team.join("tokens/alice").exists() && team.join("url").exists(), "{scripts}: uninstall kept the rest");
        let calls = std::fs::read_to_string(tmp.path().join("launchctl.log")).unwrap_or_default();
        assert!(!calls.contains("bootout"), "{scripts}: nothing was loaded, so nothing booted out: {calls}");
    }
}
