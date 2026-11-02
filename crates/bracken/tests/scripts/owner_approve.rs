//! scripts/owner-approve. Each test runs the script and its kickoff-template copy.

use crate::support::*;

/// Runs a command on a pseudo-terminal of its own and types `answer` when it asks, or (`answer` None) in a
/// new session with no terminal at all. Prints what the terminal showed and exits with the command's code.
const ON_A_TERMINAL: &str = r#"import os, pty, signal, sys
answer, argv = sys.argv[1], sys.argv[2:]
signal.alarm(120)
if answer == "-":
    os.setsid()
    os.execvp(argv[0], argv)
pid, fd = pty.fork()
if pid == 0:
    os.execvp(argv[0], argv)
out, sent = b"", False
while True:
    try:
        data = os.read(fd, 1024)
    except OSError:
        break
    if not data:
        break
    out += data
    if not sent and b"Type approve" in out:
        os.write(fd, answer.encode() + b"\n")
        sent = True
_, status = os.waitpid(pid, 0)
sys.stdout.buffer.write(out)
sys.exit(os.waitstatus_to_exitcode(status))
"#;

/// Task ec880b9c2c86: owner-approve signs, as owner and only on a terminal where the owner types
/// `approve`, an approval claim naming the full id, related supports it, and prints its id and no key.
/// As anyone else, without a terminal, for an unknown id, or without `approve`, it writes nothing. With
/// --delegated it asks nothing and the approval says it was signed on the owner's delegation, quoting
/// the yes; the same yes can't sign a second approval.
#[test]
fn owner_approve_signs_as_owner_only_on_a_terminal_and_never_prints_a_key() {
    for scripts in SCRIPTS {
        let approve = repo_root().join(scripts).join("owner-approve");
        let (tmp, dir) = repo(&[("scripts/owner-approve", &approve)], ".bracken/\n", |_| {});
        bk(&dir, &["init", "--name", "owner-approve"]);
        bk(&dir, &["id", "new", "owner"]);
        bk(&dir, &["id", "new", "manager"]);
        let task = bk(&dir, &["claim", "Task: a model run", "--tag", "task", "--as", "manager"]);
        let helper = tmp.path().join("on-a-terminal.py");
        std::fs::write(&helper, ON_A_TERMINAL).unwrap();
        let on_a_terminal = |answer: &str, args: &[&str]| {
            let mut all = vec!["-I", helper.to_str().unwrap(), answer, "scripts/owner-approve"];
            all.extend(args);
            run(&dir, "python3", &all)
        };
        let approvals = || bk(&dir, &["log", "--tag", "approval", "--json"]);
        let refused = |out: &Output, says: &str| {
            let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
            assert_eq!(out.status.code(), Some(1), "{scripts}: {text}");
            assert!(text.contains(says), "{scripts}: wanted {says:?}: {text}");
            assert_eq!(approvals(), "[]", "{scripts}: wrote an approval: {text}");
        };
        let words = "run replicate 2 on the pilot settings";

        let mut as_agent = command(&dir, "scripts/owner-approve", &[&task[..12], words]);
        refused(&as_agent.env("BRACKEN_AGENT", "manager").output().unwrap(), "only the owner approves");
        refused(&run(&dir, "scripts/owner-approve", &[&task[..12], words, "--as", "val"]), "only the owner approves");
        refused(&on_a_terminal("-", &[&task[..12], words]), "needs a terminal");
        refused(&on_a_terminal("approve", &["0123456789ab", words]), "no contribution");
        refused(&on_a_terminal("yes", &[&task[..12], words]), "not approved");

        let out = on_a_terminal("approve", &[&task[..12], words]);
        let text = String::from_utf8_lossy(&out.stdout).into_owned();
        assert!(out.status.success(), "{scripts}: {text}{}", String::from_utf8_lossy(&out.stderr));
        assert!(text.contains(&task) && text.contains("Task: a model run"), "{scripts}: didn't show it: {text}");
        let id = text.lines().map(str::trim).rfind(|l| l.len() == 64).unwrap_or_else(|| panic!("{scripts}: {text}"));
        let shown: serde_json::Value = serde_json::from_str(&bk(&dir, &["show", id, "--json"])).unwrap();
        assert_eq!(shown["author_name"], "owner", "{scripts}");
        assert_eq!(shown["body"]["tags"], serde_json::json!(["approval"]), "{scripts}");
        let statement = shown["body"]["statement"].as_str().unwrap();
        assert!(statement.starts_with("Approval (owner, ") && statement.ends_with(&format!(" of {task}: {words}")));
        assert!(statement.contains(", run by the owner) of "), "{scripts}: {statement}");
        let supports = shown["referrers"].as_array().unwrap().iter().any(|r| {
            r["author_name"] == "owner" && r["body"]["kind"] == "supports" && r["body"]["to"] == task.as_str()
        });
        assert!(supports, "{scripts}: no supports relation from owner: {shown}");
        // No hex run from the owner's identity file (its keys) reached the terminal.
        let key = std::fs::read_to_string(dir.join(".bracken/identities/owner.json")).unwrap();
        for run in key.split(|c: char| !c.is_ascii_hexdigit()).filter(|r| r.len() >= 32) {
            assert!(!text.contains(run), "{scripts}: printed part of the owner's identity file");
        }

        // Delegated: the manager, as owner, with no terminal; the approval quotes the owner's yes.
        let yes = "yes, run replicate 2";
        let delegated = |args: &[&str]| {
            let mut all = vec![&task[..12], words, "--delegated", yes];
            all.extend(args);
            let mut c = command(&dir, "scripts/owner-approve", &all);
            c.env("BRACKEN_AGENT", "manager").stdin(std::process::Stdio::null()).output().unwrap()
        };
        let count = || serde_json::from_str::<serde_json::Value>(&approvals()).unwrap().as_array().unwrap().len();
        let out = delegated(&[]);
        assert_eq!(out.status.code(), Some(1), "{scripts}: signed as manager");
        assert_eq!(count(), 1, "{scripts}");
        let out = delegated(&["--as", "owner"]);
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        assert!(out.status.success(), "{scripts}: {stdout}{}", String::from_utf8_lossy(&out.stderr));
        let shown: serde_json::Value = serde_json::from_str(&bk(&dir, &["show", stdout.trim(), "--json"])).unwrap();
        assert_eq!(shown["author_name"], "owner", "{scripts}");
        assert_eq!(shown["body"]["tags"], serde_json::json!(["approval", "delegated"]), "{scripts}");
        let statement = shown["body"]["statement"].as_str().unwrap();
        let says = format!(
            "signed by the manager on the owner's delegation for this approval only; the owner said: '{yes}') of {task}: {words}"
        );
        assert!(statement.contains(&says), "{scripts}: {statement}");
        let again = delegated(&["--as", "owner"]);
        let stderr = String::from_utf8_lossy(&again.stderr);
        assert!(again.status.code() == Some(1) && stderr.contains("its own yes"), "{scripts}: {stderr}");
        assert_eq!(count(), 2, "{scripts}: the same yes signed twice");
        // Retracting the approval doesn't free its yes (finding 8d9cc19d0568).
        bk(&dir, &["retract", stdout.trim(), "--as", "owner"]);
        let again = delegated(&["--as", "owner"]);
        let stderr = String::from_utf8_lossy(&again.stderr);
        assert!(again.status.code() == Some(1) && stderr.contains("its own yes"), "{scripts}: reused: {stderr}");
        assert_eq!(count(), 2, "{scripts}: a retracted approval's yes signed again");
    }
}

/// Task 8b650f99aacd: scripts/bk exits 1 after a local write the team server didn't confirm. A stub bk
/// does that for one command ($UNCONFIRMED): owner-approve then says the approval (and, for relate, its
/// supports relation) is written locally but not confirmed, names it, and prints the commands to
/// finish, never that it wasn't written.
#[test]
fn owner_approve_names_a_write_the_team_server_didnt_confirm() {
    for scripts in SCRIPTS {
        let approve = repo_root().join(scripts).join("owner-approve");
        let real = repo_root().join("scripts/bk");
        let (tmp, dir) = repo(
            &[("scripts/owner-approve", &approve), ("scripts/bk.real", &real)],
            ".bracken/\n",
            |dir| {
                write_executable(
                    &dir.join("scripts/bk"),
                    "#!/usr/bin/env bash\n\"$(dirname \"$0\")/bk.real\" \"$@\" || exit\n\
                 if [ \"$1\" = \"${UNCONFIRMED:-}\" ]; then\n\
                 echo \"bk: written to the local replica, but the team server didn't confirm it (stub)\" >&2; exit 1\nfi\n",
                );
            },
        );
        let _ = &tmp;
        bk(&dir, &["init", "--name", "unconfirmed"]);
        bk(&dir, &["id", "new", "owner"]);
        bk(&dir, &["id", "new", "manager"]);
        let task = bk(&dir, &["claim", "Task: a model run", "--tag", "task", "--as", "manager"]);
        let approve = |unconfirmed: &str, yes: &str| {
            let args = [&task[..12], "run it", "--delegated", yes, "--as", "owner"];
            let mut c = command(&dir, "scripts/owner-approve", &args);
            let out = c.env("UNCONFIRMED", unconfirmed).stdin(std::process::Stdio::null()).output().unwrap();
            assert_eq!(out.status.code(), Some(1), "{scripts}: {}", String::from_utf8_lossy(&out.stdout));
            String::from_utf8_lossy(&out.stderr).into_owned()
        };
        let approvals = || -> Vec<serde_json::Value> {
            serde_json::from_str::<serde_json::Value>(&bk(&dir, &["log", "--tag", "approval", "--json"]))
                .unwrap()
                .as_array()
                .unwrap()
                .clone()
        };
        let supports = |id: &str| {
            let shown: serde_json::Value = serde_json::from_str(&bk(&dir, &["show", id, "--json"])).unwrap();
            shown["referrers"].as_array().unwrap().iter().filter(|r| r["body"]["kind"] == "supports").count()
        };

        let stderr = approve("claim", "yes once");
        let written = approvals();
        assert_eq!(written.len(), 1, "{scripts}: {stderr}");
        let id = written[0]["id"].as_str().unwrap();
        assert!(!stderr.contains("wasn't written"), "{scripts}: {stderr}");
        assert!(stderr.contains(&format!("approval {id} is written locally but not confirmed")), "{scripts}: {stderr}");
        assert!(stderr.contains(&format!("scripts/bk relate {id} supports {task} --as owner")), "{scripts}: {stderr}");
        assert!(stderr.contains("scripts/bk heads --as owner"), "{scripts}: {stderr}");
        assert_eq!(supports(id), 0, "{scripts}");

        let stderr = approve("relate", "yes twice");
        let written = approvals();
        assert_eq!(written.len(), 2, "{scripts}: {stderr}");
        let id = written[1]["id"].as_str().unwrap();
        assert_eq!(supports(id), 1, "{scripts}: the relation is written locally");
        assert!(
            stderr.contains(&format!("approval {id} and its supports relation ")) && stderr.contains("not confirmed"),
            "{scripts}: {stderr}"
        );
        assert!(stderr.contains("scripts/bk heads --as owner"), "{scripts}: {stderr}");
    }
}
