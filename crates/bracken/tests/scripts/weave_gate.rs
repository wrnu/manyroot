//! scripts/weave-gate, verify-seq --weave and scripts/merge's Weave pass (plan cc496dcee0e3 step 1b,
//! design note 4f98bbf64314, decision bbbf898cbcfe): the switch, the pointer recorded and counted, each
//! fallback, a forged or stale pointer refused, and a Weave Ground that may be taken over by a forged
//! marker refused (finding 26d9361e0a9f).

use crate::support::*;

/// A stand-in for scripts/weave, keeping the Weave Ground's state as files in `$T/weave`. A manifest is
/// `blake3:` and the sha-256 of git's tree id, so `verify` compares trees. `green --manifest` reads the
/// results `validate` wrote, one '<check> <pass|fail> <result>' line each; `$T/weave/fail-<check>` makes
/// that check fail, `no-<check>` leaves it out of the policy, `unrecorded-<check>` makes validate stop
/// there, recording nothing for it, as the real one does for a check that can't run, and `extra` lists
/// more policy checks; `show` knows the ids in `proposals`. The reads weave-gate's takeover check makes
/// (`id list`, `weave check`, `weave head`, and `show` of anything but a proposal) go to the real bracken,
/// on the throwaway Weave Ground at `$T/weave/ground` that [`weave_ground`] builds.
const WEAVE: &str = r#"#!/bin/bash
set -euo pipefail
W="$T/weave"
mkdir -p "$W"
as=''
if [ "$1" = --as ]; then as=$2; shift 2; fi
echo "${as:-none} $*" >>"$W/calls"
hash_of() { echo "blake3:$(git -C "$1" rev-parse "$2^{tree}" | shasum -a 256 | cut -c1-64)"; }
opt() { local want=$1; shift; while [ $# -gt 0 ]; do [ "$1" != "$want" ] || { echo "$2"; return; }; shift; done; }
if [ "$1" = show ] && grep -qxF "$2" "$W/proposals" 2>/dev/null; then exit 0; fi
case "$1 ${2:-}" in
  "id "* | "show "* | "weave check" | "weave head") exec "$REAL_BRACKEN" --dir "$W/ground" "$@" ;;
esac
[ "$1" = weave ] || exit 9
sub=$2
shift 2
case "$sub" in
  import) echo "{\"manifest\":\"$(hash_of "$(opt --repo "$@")" "$(opt --from-git "$@")")\"}" ;;
  verify) [ "$(hash_of "$(opt --repo "$@")" "$(opt --git "$@")")" = "$(opt --manifest "$@")" ] ;;
  validate)
    m=$(opt --manifest "$@")
    state="$W/state-${m#blake3:}"
    while [ $# -gt 0 ]; do
      if [ "$1" = --check ]; then
        if [ -e "$W/unrecorded-$2" ]; then
          echo "progress: running $2" >&2
          echo "error: check $2 can't run: no cargo-nextest on PATH; nothing was written for it" >&2
          exit 1
        fi
        n=$(( $(cat "$W/n" 2>/dev/null || echo 0) + 1 )); echo "$n" >"$W/n"
        verdict=pass; [ ! -e "$W/fail-$2" ] || verdict=fail
        { grep -v "^$2 " "$state" 2>/dev/null || true; echo "$2 $verdict $(printf '%064x' "$n")"; } >"$state.new"
        mv "$state.new" "$state"
        shift
      fi
      shift
    done ;;
  green)
    m=$(opt --manifest "$@")
    state="$W/state-${m#blake3:}"
    checks='[]'
    for c in fmt clippy table mac $(cat "$W/extra" 2>/dev/null); do
      [ ! -e "$W/no-$c" ] || continue
      line=$(grep "^$c " "$state" 2>/dev/null || true)
      verdict=$(cut -d' ' -f2 <<<"$line") id=$(cut -d' ' -f3 <<<"$line")
      checks=$(jq --arg c "$c" --arg v "$verdict" --arg id "$id" '. + [{check: $c, method: ($c + " blake3:x"),
        green: ($v == "pass"), passes: (if $v == "pass" then [{result: $id}] else [] end),
        fails: (if $v == "fail" then [{result: $id}] else [] end), replaced: []}]' <<<"$checks")
    done
    jq -n --arg m "$m" --argjson c "$checks" --arg v "$(cat "$W/version" 2>/dev/null || echo "$(printf '%064x' 7)")" \
      '{line: "l", version: $v, manifest: $m, green: (($c | length) > 0 and all($c[]; .green)), checks: $c}' ;;
  *) exit 9 ;;
esac
"#;

/// The real bracken on the throwaway Weave Ground at `ground`.
fn on_ground(ground: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_bracken"))
        .arg("--dir")
        .arg(ground)
        .args(args)
        .env_remove("BRACKEN_DIR")
        .env_remove("BRACKEN_AGENT")
        .output()
        .unwrap();
    assert!(out.status.success(), "bracken {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).unwrap()
}

/// (Re)builds the stand-in's Weave Ground, `$T/weave/ground`, with the real bracken: weave init signed by
/// `author`, writing the marker and line main, and with `integrator`, an identity named weave-integrator
/// there too when `author` isn't it. Returns the Ground's path.
fn weave_ground(tmp: &Path, author: &str, integrator: bool) -> PathBuf {
    let weave = tmp.join("weave");
    std::fs::create_dir_all(&weave).unwrap();
    let ground = weave.join("ground");
    if ground.exists() {
        std::fs::remove_dir_all(&ground).unwrap();
    }
    on_ground(&ground, &["weave", "init", "--as", author]);
    if integrator && author != "weave-integrator" {
        on_ground(&ground, &["id", "new", "weave-integrator"]);
    }
    ground
}

/// A second Weave marker in `ground`, signed by a new identity, forger: as any writer can (finding
/// 26d9361e0a9f), going round scripts/weave.
fn forge_marker(ground: &Path) {
    on_ground(ground, &["id", "new", "forger"]);
    let marker = bracken_tree::Marker { v: 1, purpose: "forged".to_owned(), in_place: None };
    let path = ground.parent().unwrap().join("marker.cbor");
    std::fs::write(&path, bracken_tree::Record::encode(&marker)).unwrap();
    let path = path.to_str().unwrap();
    let media = bracken_tree::format::MARKER_MEDIA_TYPE;
    on_ground(ground, &["artifact", path, "--name", "weave", "--media-type", media, "--as", "forger"]);
}

/// A Batch whose repository also holds scripts/weave-gate and the stand-in scripts/weave, with an honest
/// Weave Ground for its reads: marker and line main by weave-integrator.
fn fixture(scripts: &str, extra: &dyn Fn(&Path, usize)) -> Batch {
    let b = Batch::shaped(
        scripts,
        3,
        &[],
        false,
        &|dir| {
            copy_executable(&repo_root().join(scripts).join("weave-gate"), &dir.join("scripts/weave-gate"));
            write_executable(&dir.join("scripts/weave"), WEAVE);
        },
        &|_| 0,
        extra,
    );
    weave_ground(b.tmp.path(), "weave-integrator", true);
    b
}

impl Batch {
    fn weave_dir(&self) -> PathBuf {
        let w = self.tmp.path().join("weave");
        std::fs::create_dir_all(&w).unwrap();
        w
    }

    /// PR #n's claim again, naming a Weave proposal the Weave Ground holds, with val's pass: its id.
    fn with_proposal(&self, n: usize) -> String {
        let proposal = format!("{:064x}", 0xabc0 + n);
        let w = self.weave_dir();
        let known = std::fs::read_to_string(w.join("proposals")).unwrap_or_default();
        std::fs::write(w.join("proposals"), format!("{known}{proposal}\n")).unwrap();
        let claim = bk(
            &self.dir,
            &[
                "claim",
                &format!("PR #{n} pr{n} @ {}: change {n}. Weave proposal {proposal}", self.shas[n - 1]),
                "--tag",
                "pr",
                "--supersedes",
                &self.claims[n - 1],
                "--as",
                "builder",
            ],
        );
        let claim = claim.lines().next().unwrap().to_owned();
        bk(&self.dir, &["evidence", &claim, "pass", "--method", "checked", "--as", "val"]);
        claim
    }

    fn gate(&self, statement: &str) -> String {
        bk(&self.dir, &["claim", statement, "--tag", "weave-gate", "--as", "manager"])
            .lines()
            .next()
            .unwrap()
            .to_owned()
    }

    fn state(&self) -> Output {
        self.command("scripts/weave-gate", &["state"]).output().unwrap()
    }

    fn record(&self, claim: &str, n: usize) -> Output {
        let pair = format!("{}:{}", &claim[..12], self.shas[n - 1]);
        self.command("scripts/verify-seq", &["--weave", &pair]).output().unwrap()
    }

    /// The manager's Weave pointers on a claim, as (method, details).
    fn pointers(&self, claim: &str) -> Vec<(String, String)> {
        let shown: serde_json::Value = serde_json::from_str(&bk(&self.dir, &["show", claim, "--json"])).unwrap();
        shown["referrers"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| r["kind"] == "evidence" && r["author_name"] == "manager")
            .filter(|r| r["body"]["method"].as_str().unwrap().starts_with("scripts/test-table --weave @ "))
            .map(|r| {
                let details = r["body"]["details"].as_str().unwrap_or_default().to_owned();
                (r["body"]["method"].as_str().unwrap().to_owned(), details)
            })
            .collect()
    }

    /// scripts/merge with the manager as the gate's validator, as the team runs it: a pointer is then one of
    /// the passes the gate counts.
    fn merge_one(&self, n: usize, claim: &str, dry: bool) -> Output {
        let pr = n.to_string();
        let mut args = vec![pr.as_str(), claim, "--validator", "manager", "--min-passes", "1", "--as", "manager"];
        if dry {
            args.push("--dry-run");
        }
        self.command("scripts/merge", &args).output().unwrap()
    }

    /// The key of git's merge of `main` with PR #n's head, as scripts/code-scope chain gives it.
    fn merge_key(&self, main: &str, n: usize) -> String {
        let out = run(&self.dir, "scripts/code-scope", &["chain", main, &self.shas[n - 1]]);
        let text = String::from_utf8(out.stdout).unwrap();
        text.lines().last().unwrap().split(' ').nth(2).unwrap().to_owned()
    }

    /// A manager pointer on `claim`, written by hand: PR #n's head on main's tip, with `details`.
    fn forge_pointer(&self, claim: &str, n: usize, details: &str) -> String {
        let main = self.forge_main();
        let key = self.merge_key(&main, n);
        let method = format!(
            "scripts/test-table --weave @ {} on {main} tree {key} manifest blake3:{}",
            self.shas[n - 1],
            "4".repeat(64)
        );
        let file = self.tmp.path().join("forged-details");
        std::fs::write(&file, details).unwrap();
        let id = bk(
            &self.dir,
            &[
                "evidence",
                claim,
                "pass",
                "--method",
                &method,
                "--details-file",
                file.to_str().unwrap(),
                "--as",
                "manager",
            ],
        );
        id.lines().next().unwrap().to_owned()
    }
}

/// The switch: off with no claim, on with one 'Weave gate: on', off with two current, and off again when
/// the manager supersedes it with 'off'. Only the manager's claims count.
#[test]
fn the_weave_gate_is_off_unless_one_manager_claim_turns_it_on() {
    for scripts in SCRIPTS {
        let b = fixture(scripts, &|_, _| {});
        let out = b.state();
        assert_eq!(out.status.code(), Some(1), "{scripts}: {}", text(&out));
        assert!(text(&out).contains("off: no current manager claim"), "{scripts}: {}", text(&out));

        bk(&b.dir, &["claim", "Weave gate: on", "--tag", "weave-gate", "--as", "builder"]);
        assert_eq!(b.state().status.code(), Some(1), "{scripts}: a builder's claim turned it on");

        // Findings 307d5bd13ec5 and 840e29c4202c: only 'Weave gate: on' as the whole first line is on.
        for statement in ["Weave gate: on-hold until the trial ends", "Weave gate: on (stage 2b)"] {
            let claim = b.gate(statement);
            let out = b.state();
            assert!(!out.status.success(), "{scripts}: {statement:?} turned it on");
            bk(&b.dir, &["retract", &claim, "--as", "manager"]);
        }
        let hold = b.gate("Weave gate: on hold until the trial ends");
        let out = b.state();
        assert!(
            !out.status.success() && text(&out).contains("doesn't have 'Weave gate: on' as its whole first line"),
            "{scripts}: {}",
            text(&out)
        );
        bk(&b.dir, &["retract", &hold, "--as", "manager"]);

        let on = b.gate("Weave gate: on\nStage 2b, owner approval e8fd45c83ba6.");
        let out = b.state();
        assert!(out.status.success(), "{scripts}: {}", text(&out));
        assert!(text(&out).starts_with(&format!("on {}", &on[..12])), "{scripts}: {}", text(&out));

        let second = b.gate("Weave gate: on");
        let out = b.state();
        assert!(!out.status.success() && text(&out).contains("2 current"), "{scripts}: {}", text(&out));
        bk(&b.dir, &["retract", &second, "--as", "manager"]);
        assert!(b.state().status.success(), "{scripts}");

        bk(
            &b.dir,
            &[
                "claim",
                "Weave gate: off (stop condition)",
                "--tag",
                "weave-gate",
                "--supersedes",
                &on,
                "--as",
                "manager",
            ],
        );
        let out = b.state();
        assert!(
            !out.status.success() && text(&out).contains("doesn't have 'Weave gate: on' as its whole first line"),
            "{scripts}: {}",
            text(&out)
        );
    }
}

/// verify-seq --weave records the pointer once line main's checks are green on main merged with the head,
/// running only the checks with no result; scripts/merge then counts it, here with main moved in code
/// since the head branched, where nothing else would do.
#[test]
fn weave_results_on_the_merge_count_in_place_of_the_table() {
    for scripts in SCRIPTS {
        let b = fixture(scripts, &|_, _| {});
        b.gate("Weave gate: on");
        b.main_moves("z.rs");
        let claim = b.with_proposal(1);
        // Without a pass of the merge, main having moved in code fails the gate.
        let out = b.merge_one(1, &claim, true);
        assert!(check_line(&out, "main").starts_with("FAIL"), "{scripts}: {}", text(&out));

        let out = b.record(&claim, 1);
        assert!(out.status.success(), "{scripts}: {}", text(&out));
        let pointers = b.pointers(&claim);
        assert_eq!(pointers.len(), 1, "{scripts}: {pointers:?}");
        let (method, details) = &pointers[0];
        let main = b.forge_main();
        assert!(method.starts_with(&format!("scripts/test-table --weave @ {} on {main} tree ", b.shas[0])), "{method}");
        for c in ["fmt", "clippy", "table", "mac"] {
            assert!(details.lines().any(|l| l.starts_with(&format!("result {c} "))), "{scripts}: {details}");
        }
        let calls = std::fs::read_to_string(b.tmp.path().join("weave/calls")).unwrap();
        assert!(
            calls.contains("weave-runner weave validate --line main --manifest blake3:")
                && calls.contains("--check fmt --check clippy --check table --check mac"),
            "{scripts}: {calls}"
        );
        assert!(
            calls.lines().any(|l| l.starts_with("none weave import ") && l.ends_with(" --as weave-integrator --json")),
            "{scripts}: {calls}"
        );

        // Again: nothing to validate, and a second pointer naming the same results.
        let out = b.record(&claim, 1);
        assert!(out.status.success(), "{scripts}: {}", text(&out));
        let calls = std::fs::read_to_string(b.tmp.path().join("weave/calls")).unwrap();
        assert_eq!(calls.matches("weave validate").count(), 1, "{scripts}: {calls}");

        let out = b.merge_one(1, &claim, false);
        assert!(out.status.success(), "{scripts}: {}", text(&out));
        let scope = check_line(&out, "scope");
        assert!(
            scope.starts_with("ok") && scope.contains("line main's Weave results on blake3:"),
            "{scripts}: {scope}"
        );
        assert!(check_line(&out, "main").starts_with("ok"), "{scripts}: {}", text(&out));
        assert_eq!(b.merged(), vec![1], "{scripts}");
    }
}

/// Each fallback records nothing and says to run git's counted run: the gate off, no proposal, a head
/// that changes scripts/test-table or Cargo.lock, a head that doesn't merge, and a check that isn't green.
#[test]
fn each_fallback_records_nothing_and_asks_for_gits_counted_run() {
    for scripts in SCRIPTS {
        let b = fixture(scripts, &|dir, n| match n {
            2 => put(dir, "Cargo.lock", "# changed\n"),
            3 => put(dir, "scripts/test-table", "#!/bin/sh\necho changed\n"),
            _ => {}
        });
        let refused = |b: &Batch, claim: &str, n: usize, why: &str| {
            let out = b.record(claim, n);
            assert_eq!(out.status.code(), Some(1), "{scripts}: {why}: {}", text(&out));
            let t = text(&out);
            assert!(t.contains(why) && t.contains("run git's counted run"), "{scripts}: {why}: {t}");
            assert!(b.pointers(claim).is_empty(), "{scripts}: {why}: a pointer was recorded");
        };
        let claim1 = b.with_proposal(1);
        refused(&b, &claim1, 1, "the Weave gate is off: no current manager claim");
        b.gate("Weave gate: on");
        refused(&b, &b.claims[0].clone(), 1, "names no 'Weave proposal");
        let claim2 = b.with_proposal(2);
        refused(&b, &claim2, 2, "the head changes Cargo.lock");
        let claim3 = b.with_proposal(3);
        refused(&b, &claim3, 3, "the head changes scripts/test-table");

        std::fs::write(b.tmp.path().join("weave/fail-table"), "").unwrap();
        refused(&b, &claim1, 1, "check table isn't green");
        std::fs::remove_file(b.tmp.path().join("weave/fail-table")).unwrap();

        // main changes the head's own file: no clean merge.
        put(&b.dir, "a1.rs", "main's own\n");
        commit_all(&b.dir, "main conflicts");
        assert!(run(&b.dir, "git", &["push", "-q", b.forge.to_str().unwrap(), "main"]).status.success());
        refused(&b, &claim1, 1, "doesn't merge cleanly onto main");
    }
}

/// Gotcha c05efcb89101: when validate records nothing for a check (a tool missing from PATH, say), record
/// says nothing was recorded, not that the check isn't green: with validate's exit code and last error
/// line, and the tools the checks need that aren't on PATH, or that they all are.
#[test]
fn a_check_validate_recorded_nothing_for_says_so_and_why() {
    for scripts in SCRIPTS {
        let b = fixture(scripts, &|_, _| {});
        b.gate("Weave gate: on");
        let claim = b.with_proposal(1);
        std::fs::write(b.weave_dir().join("unrecorded-table"), "").unwrap();
        let pair = format!("{}:{}", &claim[..12], b.shas[0]);
        let record = |path: &str| {
            let out = b.command("scripts/weave-gate", &["record", &pair]).env("PATH", path).output().unwrap();
            assert_eq!(out.status.code(), Some(1), "{scripts}: {}", text(&out));
            assert!(b.pointers(&claim).is_empty(), "{scripts}: a pointer was recorded");
            text(&out)
        };
        let stubs = b.tmp.path().join("stubs");
        // This PATH lacks every directory holding cargo-nextest or boat.
        let lacking: Vec<String> = std::env::split_paths(&std::env::var_os("PATH").unwrap())
            .filter(|d| !d.join("cargo-nextest").exists() && !d.join("boat").exists())
            .map(|d| d.display().to_string())
            .collect();
        let lacking = format!("{}:{}", stubs.display(), lacking.join(":"));
        let said = record(&lacking);
        assert!(said.contains("nothing was recorded for check table on blake3:"), "{scripts}: {said}");
        assert!(!said.contains("isn't green"), "{scripts}: it reads like a test failure: {said}");
        assert!(said.contains("progress: running table"), "{scripts}: validate's output didn't show: {said}");
        assert!(
            said.contains("weave validate exited 1: error: check table can't run: no cargo-nextest on PATH"),
            "{scripts}: {said}"
        );
        let missing = said.split("not on PATH: ").nth(1).unwrap_or_else(|| panic!("{scripts}: {said}"));
        let missing = missing.split(')').next().unwrap();
        assert!(missing.contains("cargo-nextest") && missing.contains("boat"), "{scripts}: {said}");
        assert!(said.contains("so this is no test result") && said.contains("run git's counted run"), "{scripts}");

        // Every tool there: it says so, and still that nothing was recorded.
        let tools = b.tmp.path().join("tools");
        std::fs::create_dir_all(&tools).unwrap();
        for t in ["cargo", "cargo-nextest", "boat", "timeout"] {
            write_executable(&tools.join(t), "#!/bin/sh\nexit 0\n");
        }
        let said = record(&format!("{}:{lacking}", tools.display()));
        assert!(said.contains("nothing was recorded for check table"), "{scripts}: {said}");
        assert!(said.contains("every tool the checks need is on PATH"), "{scripts}: {said}");
    }
}

/// The policy must have all four checks: a pointer isn't recorded when it lacks mac.
#[test]
fn a_policy_without_the_mac_check_records_nothing() {
    for scripts in SCRIPTS {
        let b = fixture(scripts, &|_, _| {});
        b.gate("Weave gate: on");
        std::fs::write(b.weave_dir().join("no-mac"), "").unwrap();
        let claim = b.with_proposal(1);
        let out = b.record(&claim, 1);
        assert_eq!(out.status.code(), Some(1), "{scripts}: {}", text(&out));
        assert!(text(&out).contains("line main's policy has no check mac"), "{scripts}: {}", text(&out));
    }
}

/// scripts/merge re-checks a pointer from scratch and refuses it, failing the scope check, when it's
/// forged (another manifest, another key) or stale (the results re-run since, a check failing now, the
/// gate turned off).
#[test]
fn a_forged_or_stale_pointer_is_refused() {
    for scripts in SCRIPTS {
        let b = fixture(scripts, &|_, _| {});
        let on = b.gate("Weave gate: on");
        let claim = b.with_proposal(1);
        assert!(b.record(&claim, 1).status.success(), "{scripts}");
        let (method, details) = b.pointers(&claim).remove(0);
        let scope_fails = |b: &Batch, why: &str| {
            let out = b.merge_one(1, &claim, true);
            let scope = check_line(&out, "scope");
            assert!(scope.starts_with("FAIL") && scope.contains(why), "{scripts}: {why}: {scope}");
            assert!(!out.status.success(), "{scripts}: {why}");
        };
        let forge = |b: &Batch, method: &str, details: &str| -> String {
            let file = b.tmp.path().join("details");
            std::fs::write(&file, details).unwrap();
            let id = bk(
                &b.dir,
                &[
                    "evidence",
                    &claim,
                    "pass",
                    "--method",
                    method,
                    "--details-file",
                    file.to_str().unwrap(),
                    "--as",
                    "manager",
                ],
            );
            id.lines().next().unwrap().to_owned()
        };
        let retract_all = |b: &Batch| {
            let shown: serde_json::Value = serde_json::from_str(&bk(&b.dir, &["show", &claim, "--json"])).unwrap();
            for r in shown["referrers"].as_array().unwrap() {
                if r["kind"] == "evidence" && r["author_name"] == "manager" && r["retracted"] == false {
                    bk(&b.dir, &["retract", r["id"].as_str().unwrap(), "--as", "manager"]);
                }
            }
        };
        assert!(b.merge_one(1, &claim, true).status.success(), "{scripts}: the honest pointer counts");

        // Finding 76c500f0c75d: a pointer on a merge with a commit off main, X, never counts.
        retract_all(&b);
        let x = {
            let index = b.tmp.path().join("x-index");
            let git = |args: &[&str]| {
                let out = command(&b.dir, "git", args).env("GIT_INDEX_FILE", &index).output().unwrap();
                assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
                String::from_utf8(out.stdout).unwrap().trim().to_owned()
            };
            git(&["read-tree", "origin/main"]);
            let blob = git(&["hash-object", "-w", "--stdin", "--path", "x.rs"]);
            git(&["update-index", "--add", "--cacheinfo", &format!("100644,{blob},x.rs")]);
            let tree = git(&["write-tree"]);
            git(&["commit-tree", &tree, "-p", "origin/main", "-m", "off main"])
        };
        let pair = format!("{}:{}", &claim[..12], b.shas[0]);
        let out = b.command("scripts/verify-seq", &["--weave", &pair]).env("ON", &x).output().unwrap();
        assert!(out.status.success(), "{scripts}: {}", text(&out));
        scope_fails(&b, "isn't main's tip");

        // Another manifest.
        retract_all(&b);
        let other = format!("blake3:{}", "1".repeat(64));
        let manifest = method.rsplit(' ').next().unwrap();
        forge(&b, &method.replace(manifest, &other), &details);
        scope_fails(&b, "doesn't verify equal to git's merge");

        // Another key.
        retract_all(&b);
        let key = method.split(" tree ").nth(1).unwrap().split(' ').next().unwrap();
        forge(&b, &method.replace(key, &"2".repeat(40)), &details);
        scope_fails(&b, "not the pointer's");

        // Results that aren't the ones counted: named by hand.
        retract_all(&b);
        forge(&b, &method, &format!("result fmt {}\n", "3".repeat(64)));
        scope_fails(&b, "aren't the ones counted");

        // The honest pointer again, then a re-run since: stale.
        retract_all(&b);
        forge(&b, &method, &details);
        assert!(
            b.command(
                "scripts/weave",
                &[
                    "--as",
                    "weave-runner",
                    "weave",
                    "validate",
                    "--line",
                    "main",
                    "--manifest",
                    manifest,
                    "--check",
                    "fmt"
                ]
            )
            .status()
            .unwrap()
            .success()
        );
        scope_fails(&b, "aren't the ones counted");

        // A check that fails now.
        retract_all(&b);
        std::fs::write(b.tmp.path().join("weave/fail-clippy"), "").unwrap();
        assert!(
            b.command(
                "scripts/weave",
                &[
                    "--as",
                    "weave-runner",
                    "weave",
                    "validate",
                    "--line",
                    "main",
                    "--manifest",
                    manifest,
                    "--check",
                    "clippy"
                ]
            )
            .status()
            .unwrap()
            .success()
        );
        forge(&b, &method, &details);
        scope_fails(&b, "check clippy isn't green");

        // The gate off: even an honest pointer doesn't count, and alone it can't carry the gate.
        retract_all(&b);
        std::fs::remove_file(b.tmp.path().join("weave/fail-clippy")).unwrap();
        assert!(
            b.command(
                "scripts/weave",
                &[
                    "--as",
                    "weave-runner",
                    "weave",
                    "validate",
                    "--line",
                    "main",
                    "--manifest",
                    manifest,
                    "--check",
                    "clippy"
                ]
            )
            .status()
            .unwrap()
            .success()
        );
        assert!(b.record(&claim, 1).status.success(), "{scripts}");
        assert!(b.merge_one(1, &claim, true).status.success(), "{scripts}: counted again");
        bk(&b.dir, &["claim", "Weave gate: off", "--tag", "weave-gate", "--supersedes", &on, "--as", "manager"]);
        scope_fails(&b, "the Weave gate is off");
    }
}

/// Finding cfd70a4b5283: a pointer that doesn't count is ignored, not a failure, so git's counted run on
/// the same claim still lets the PR merge; and check 5 doesn't take the ignored pointer's main.
#[test]
fn a_pointer_that_doesnt_count_is_ignored_beside_gits_counted_run() {
    for scripts in SCRIPTS {
        let b = fixture(scripts, &|_, _| {});
        let on = b.gate("Weave gate: on");
        b.main_moves("z.rs");
        let claim = b.with_proposal(1);
        assert!(b.record(&claim, 1).status.success(), "{scripts}");
        bk(&b.dir, &["claim", "Weave gate: off", "--tag", "weave-gate", "--supersedes", &on, "--as", "manager"]);
        // Alone, the ignored pointer can't carry the gate.
        let out = b.merge_one(1, &claim, true);
        assert!(check_line(&out, "scope").contains("no other manager pass of the table"), "{scripts}: {}", text(&out));

        // Beside a run of the head alone, without --on, check 5 doesn't take the ignored pointer's main:
        // main moved in code since the head branched, so nothing tested the merge.
        let plain = format!("scripts/test-table @ {}", b.shas[0]);
        let alone = bk(&b.dir, &["evidence", &claim, "pass", "--method", &plain, "--as", "manager"]);
        let out = b.merge_one(1, &claim, true);
        assert!(check_line(&out, "scope").starts_with("ok"), "{scripts}: {}", text(&out));
        assert!(check_line(&out, "main").starts_with("FAIL"), "{scripts}: {}", text(&out));
        bk(&b.dir, &["retract", alone.lines().next().unwrap(), "--as", "manager"]);

        // With git's counted run of the merge beside it, the PR merges, saying the pointer was ignored.
        let main = b.forge_main();
        let key = b.merge_key(&main, 1);
        let method = format!("scripts/test-table @ {} on {main} tree {key}", b.shas[0]);
        bk(&b.dir, &["evidence", &claim, "pass", "--method", &method, "--as", "manager"]);
        let out = b.merge_one(1, &claim, true);
        assert!(out.status.success(), "{scripts}: {}", text(&out));
        let scope = check_line(&out, "scope");
        assert!(scope.starts_with("ok") && scope.contains("ignored: the Weave gate is off"), "{scripts}: {scope}");
        assert!(check_line(&out, "main").contains("tested"), "{scripts}: {}", text(&out));
    }
}

/// Finding 0b2cafdc271d: merge re-checks the git-run cases itself, so a pointer written by hand on a head
/// that changes Cargo.lock or scripts/test-table, or on a claim naming no proposal, never counts; nor does
/// one where a further policy check fails.
#[test]
fn merge_refuses_a_forged_pointer_on_a_head_that_needs_gits_run() {
    for scripts in SCRIPTS {
        let b = fixture(scripts, &|dir, n| match n {
            2 => put(dir, "Cargo.lock", "# changed\n"),
            3 => put(dir, "scripts/test-table", "#!/bin/sh\necho changed\n"),
            _ => {}
        });
        b.gate("Weave gate: on");
        let details = format!("result fmt {}\n", "5".repeat(64));
        for (n, why) in [(2, "the head changes Cargo.lock"), (3, "the head changes scripts/test-table")] {
            let claim = b.with_proposal(n);
            b.forge_pointer(&claim, n, &details);
            let out = b.merge_one(n, &claim, true);
            let scope = check_line(&out, "scope");
            assert!(!out.status.success() && scope.starts_with("FAIL") && scope.contains(why), "{scripts}: {scope}");
        }
        // Finding f6bf883e41d7: with the pointer ignored, other validators' passes can't stand in for the
        // manager's run, however many there are.
        let claim = b.with_proposal(2);
        bk(&b.dir, &["evidence", &claim, "pass", "--method", "re-reviewed", "--as", "val"]);
        b.forge_pointer(&claim, 2, &details);
        let args = [
            "2",
            &claim,
            "--validator",
            "manager",
            "--validator",
            "val",
            "--min-passes",
            "2",
            "--as",
            "manager",
            "--dry-run",
        ];
        let out = b.command("scripts/merge", &args).output().unwrap();
        assert!(check_line(&out, "gate").starts_with("ok"), "{scripts}: {}", text(&out));
        let scope = check_line(&out, "scope");
        assert!(
            !out.status.success() && scope.starts_with("FAIL") && scope.contains("no other manager pass of the table"),
            "{scripts}: {scope}"
        );

        let original = b.claims[0].clone();
        b.forge_pointer(&original, 1, &details);
        let out = b.merge_one(1, &original, true);
        let scope = check_line(&out, "scope");
        assert!(scope.starts_with("FAIL") && scope.contains("names no 'Weave proposal"), "{scripts}: {scope}");

        // A fifth policy check that isn't green: refused by record, and by merge for an honest pointer
        // recorded before the check was added.
        let claim = b.with_proposal(1);
        std::fs::write(b.weave_dir().join("extra"), "lint\n").unwrap();
        let out = b.record(&claim, 1);
        assert!(text(&out).contains("not every check of line main's policy is green"), "{scripts}: {}", text(&out));
        std::fs::remove_file(b.weave_dir().join("extra")).unwrap();
        assert!(b.record(&claim, 1).status.success(), "{scripts}");
        std::fs::write(b.weave_dir().join("extra"), "lint\n").unwrap();
        let out = b.merge_one(1, &claim, true);
        let scope = check_line(&out, "scope");
        assert!(
            scope.starts_with("FAIL") && scope.contains("not every check of line main's policy is green"),
            "{scripts}: {scope}"
        );
    }
}

/// Finding 76c500f0c75d, by check 5's rule: a pointer recorded on an earlier main still counts once main
/// has moved only in docs the head doesn't touch, and doesn't once main has moved in code.
#[test]
fn a_pointer_counts_only_while_main_has_moved_since_only_in_docs() {
    for scripts in SCRIPTS {
        let b = fixture(scripts, &|_, _| {});
        b.gate("Weave gate: on");
        let claim = b.with_proposal(1);
        assert!(b.record(&claim, 1).status.success(), "{scripts}");
        b.main_moves("docs/later.md");
        let out = b.merge_one(1, &claim, true);
        assert!(out.status.success(), "{scripts}: docs only: {}", text(&out));
        b.main_moves("y.rs");
        let out = b.merge_one(1, &claim, true);
        let scope = check_line(&out, "scope");
        assert!(!out.status.success() && scope.contains("has moved since its main"), "{scripts}: {scope}");
    }
}

/// weave-gate check on its own (note (a) on finding 76c500f0c75d): a pointer whose main is an ancestor of
/// the tip but older than the head's branch point is refused, before anything else is read.
#[test]
fn check_refuses_a_main_older_than_the_heads_branch_point() {
    for scripts in SCRIPTS {
        let b = fixture(scripts, &|_, _| {});
        b.gate("Weave gate: on");
        let claim = b.with_proposal(1);
        let rev = |r: &str| String::from_utf8(run(&b.dir, "git", &["rev-parse", r]).stdout).unwrap().trim().to_owned();
        let (tip, older) = (b.forge_main(), rev("origin/main~1"));
        let key = b.merge_key(&older, 1);
        let manifest = format!("blake3:{}", "6".repeat(64));
        let method = format!("scripts/test-table --weave @ {} on {older} tree {key} manifest {manifest}", b.shas[0]);
        let pointer = bk(&b.dir, &["evidence", &claim, "pass", "--method", &method, "--as", "manager"]);
        let pointer = pointer.lines().next().unwrap();
        let full: serde_json::Value = serde_json::from_str(&bk(&b.dir, &["show", &claim, "--json"])).unwrap();
        let full = full["id"].as_str().unwrap();
        let args = ["check", full, &b.shas[0], &older, &key, &manifest, pointer, &tip];
        let out = b.command("scripts/weave-gate", &args).output().unwrap();
        assert!(!out.status.success(), "{scripts}: {}", text(&out));
        assert!(text(&out).contains("doesn't hold the head's merge base"), "{scripts}: {}", text(&out));
    }
}

/// Finding 26d9361e0a9f, decision 03534914ec55: record and check both refuse, failing closed, a Weave
/// Ground that may have been taken over: an extra marker, line main by another identity, no key of
/// weave-integrator's, and a second line named main. An honest pointer recorded before stops counting.
#[test]
fn a_weave_ground_that_may_be_taken_over_is_refused() {
    for scripts in SCRIPTS {
        let b = fixture(scripts, &|_, _| {});
        b.gate("Weave gate: on");
        let claim = b.with_proposal(1);
        let out = b.record(&claim, 1);
        assert!(out.status.success(), "{scripts}: the honest Ground: {}", text(&out));
        assert!(b.merge_one(1, &claim, true).status.success(), "{scripts}: the honest pointer counts");
        let refused = |b: &Batch, why: &str| {
            let out = b.record(&claim, 1);
            let t = text(&out);
            assert_eq!(out.status.code(), Some(1), "{scripts}: {why}: {t}");
            assert!(
                t.contains("refused: ") && t.contains(why) && t.contains("run git's counted run"),
                "{scripts}: {t}"
            );
            assert_eq!(b.pointers(&claim).len(), 1, "{scripts}: {why}: a pointer was recorded");
            let out = b.merge_one(1, &claim, true);
            let scope = check_line(&out, "scope");
            assert!(!out.status.success() && scope.starts_with("FAIL"), "{scripts}: {why}: {}", text(&out));
            assert!(text(&out).contains(why), "{scripts}: {why}: {}", text(&out));
        };

        let ground = b.tmp.path().join("weave/ground");
        forge_marker(&ground);
        refused(&b, "extra marker(s)");

        weave_ground(b.tmp.path(), "other", true);
        refused(&b, "is by other (");
        refused(&b, ", not weave-integrator (");

        weave_ground(b.tmp.path(), "other", false);
        refused(&b, "holds no key it can sign with as weave-integrator");

        let ground = weave_ground(b.tmp.path(), "weave-integrator", true);
        assert!(b.merge_one(1, &claim, true).status.success(), "{scripts}: honest again");
        on_ground(&ground, &["id", "new", "forger"]);
        on_ground(&ground, &["weave", "line", "create", "main", "--owner-decision", &"a".repeat(12), "--as", "forger"]);
        refused(&b, "--line main names no single line");
    }
}
