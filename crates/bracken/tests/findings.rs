//! The merge gate's findings check has one definition, `bracken findings` (task `dea48fd16754`): a
//! differential test against the jq copy scripts/merge has held until now, kept here verbatim as the oracle.
//! It runs both on the cases `log --open`'s test covers and on seeded random Grounds, for every pr claim and
//! two triagers, and requires the same untriaged, undeferred and deferred findings and the same count.
//!
//! scripts/merge now reads `findings` (the second PR of answer `945b6e49fa35`): a guard checks that it (and the
//! kickoff template) does, and holds no copy of the rules. The oracle stays, as the record of what the rule was.

use std::path::Path;
use std::process::Command;

use manyroot::{Body, ContributionId, Identity, RelationKind, Store, Verdict};
use serde_json::Value;

/// scripts/merge's findings check, section 3, from `contradicts=` to the loop's `done`, as it stood when
/// `findings` replaced it.
const OLD_CHECK: &str = r##"contradicts=$(jq -c --arg id "$full" '[.referrers[] | select(.kind == "relation"
  and .body.kind == "contradicts" and .body.to == $id)]' <<<"$shown")
findings=$(jq -r '[.[].body.from] | unique | .[]' <<<"$contradicts")
untriaged=() undeferred=() deferred=() findings_count=0
current_tasks=$(bk log --tag task --current --json) || fail "couldn't read the current tasks"
for f in $findings; do
  about=$(bk show "$f" --json) || fail "couldn't read finding $f, so the findings check can't decide"
  [ "$(jq -r .retracted <<<"$about")" = false ] || continue
  jq -e --arg f "$f" --arg by "$(jq -r .author <<<"$about")" --arg who "$triager" 'any(.[]; .body.from == $f
    and (.retracted == false or (.author != $by and .author_name != $who)))' <<<"$contradicts" >/dev/null || continue
  findings_count=$((findings_count + 1))
  verdicts=$(jq -r --arg who "$triager" --arg f "$f" '[.referrers[] | select(.kind == "evidence"
    and .retracted == false and .author_name == $who and .body.target == $f) | .body.verdict] | unique | join(" ")' <<<"$about")
  case " $verdicts " in
    "  ") untriaged+=("${f:0:12}"); continue ;;
    *" pass "*) ;;
    *) continue ;; # rejected: stays on record, doesn't block
  esac
  # Only the triager defers (process 4.2): both the depends_on relation and the task must be theirs,
  # and the task current: not retracted, and not superseded by its author.
  task=""
  for t in $(jq -r --arg f "$f" --arg who "$triager" '.referrers[] | select(.kind == "relation"
    and .retracted == false and .author_name == $who and .body.kind == "depends_on" and .body.to == $f)
    | .body.from' <<<"$about"); do
    if jq -e --arg t "$t" --arg who "$triager" 'any(.[]; .id == $t and .author_name == $who
      and (.body.tags | index("deferred")))' <<<"$current_tasks" >/dev/null; then
      task=$t
      break
    fi
  done
  if [ -n "$task" ]; then deferred+=("${f:0:12}->${task:0:12}"); else undeferred+=("${f:0:12}"); fi
done
"##;

/// The oracle: the old check, run as the script ran it, with `bk` the binary under test. Prints its lists.
fn old(dir: &Path, claim: &ContributionId, triager: &str) -> String {
    let script = format!(
        "set -eo pipefail\nbk() {{ \"$BRACKEN\" \"$@\"; }}\nfail() {{ echo \"fail: $*\" >&2; exit 1; }}\n\
         triager=$2\nshown=$(bk show \"$1\" --json)\nfull=$(jq -r .id <<<\"$shown\")\n{OLD_CHECK}\
         echo \"untriaged=${{untriaged[*]:-}}\"\necho \"undeferred=${{undeferred[*]:-}}\"\n\
         echo \"deferred=${{deferred[*]:-}}\"\necho \"count=$findings_count\"\n"
    );
    let out = Command::new("bash")
        .args(["-c", &script, "old-check", &claim.to_hex(), triager])
        .env("BRACKEN", env!("CARGO_BIN_EXE_bracken"))
        .env("BRACKEN_DIR", dir)
        .env_remove("BRACKEN_AGENT")
        .output()
        .expect("bash runs");
    assert!(out.status.success(), "the old check failed: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).expect("utf-8")
}

/// The new read, printed as the old check prints.
fn new(dir: &Path, claim: &ContributionId, triager: &str) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_bracken"))
        .args(["findings", &claim.to_hex(), "--triager", triager, "--json"])
        .env("BRACKEN_DIR", dir)
        .env_remove("BRACKEN_AGENT")
        .output()
        .expect("bracken runs");
    assert!(out.status.success(), "findings failed: {}", String::from_utf8_lossy(&out.stderr));
    let v: Value = serde_json::from_slice(&out.stdout).expect("json");
    let findings = v["findings"].as_array().expect("a list");
    let short = |v: &Value| v.as_str().expect("an id")[..12].to_owned();
    let of = |state: &str| -> Vec<String> {
        findings.iter().filter(|f| f["triage"] == state).map(|f| short(&f["id"])).collect()
    };
    let deferred: Vec<String> = findings
        .iter()
        .filter(|f| f["triage"] == "deferred")
        .map(|f| format!("{}->{}", short(&f["id"]), short(&f["task"])))
        .collect();
    for f in findings {
        let blocks = matches!(f["triage"].as_str(), Some("untriaged" | "confirmed"));
        assert_eq!(f["blocks"], blocks, "{f}");
    }
    let blocking = findings.iter().filter(|f| f["blocks"] == true).count();
    assert_eq!(v["blocking"], blocking);
    format!(
        "untriaged={}\nundeferred={}\ndeferred={}\ncount={}\n",
        of("untriaged").join(" "),
        of("confirmed").join(" "),
        deferred.join(" "),
        findings.len()
    )
}

fn agree(dir: &Path, claims: &[ContributionId]) -> usize {
    let mut nonempty = 0;
    for claim in claims {
        for triager in ["manager", "builder"] {
            let (o, n) = (old(dir, claim, triager), new(dir, claim, triager));
            assert_eq!(n, o, "claim {claim}, triager {triager}");
            nonempty += usize::from(!o.ends_with("count=0\n"));
        }
    }
    nonempty
}

/// scripts/merge's findings check is `bracken findings` (task dea48fd16754), not a copy of its rules: the
/// copy above is only the oracle now.
#[test]
fn scripts_merge_reads_findings_and_keeps_no_copy() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    for script in ["scripts/merge", ".claude/skills/agent-team-kickoff/templates/merge"] {
        let text = std::fs::read_to_string(root.join(script)).expect("readable");
        assert!(text.contains(r#"bk findings "$full" --triager "$triager" --json"#), "{script} reads findings");
        assert!(!text.contains("current_tasks=") && !text.contains("verdicts="), "{script} keeps no copy of its rules");
    }
}

struct Ground {
    store: Store,
    ids: Vec<Identity>,
}

impl Ground {
    fn new(dir: &Path) -> Ground {
        let store = Store::init(dir, "findings").expect("a Ground");
        let ids = ["manager", "builder", "reviewer", "security"]
            .iter()
            .map(|n| store.create_identity(n).expect("an id"))
            .collect();
        Ground { store, ids }
    }
    fn put(&mut self, who: usize, body: Body) -> ContributionId {
        self.store.contribute(&self.ids[who], body).expect("contributes")
    }
    fn claim(&mut self, who: usize, text: &str, tags: &[&str]) -> ContributionId {
        self.put(who, Body::Claim { statement: text.into(), tags: tags.iter().map(|t| (*t).to_owned()).collect() })
    }
    fn relate(&mut self, who: usize, from: ContributionId, kind: RelationKind, to: ContributionId) -> ContributionId {
        self.put(who, Body::Relation { from, kind, to, note: String::new() })
    }
    fn evidence(&mut self, who: usize, target: ContributionId, pass: bool, method: &str) -> ContributionId {
        let verdict = if pass { Verdict::Pass } else { Verdict::Fail };
        self.put(who, Body::Evidence { target, verdict, method: method.into(), details: String::new() })
    }
    fn retract(&mut self, who: usize, target: ContributionId) -> ContributionId {
        self.put(who, Body::Retract { target, reason: "withdrawn".into() })
    }
}

const MANAGER: usize = 0;
const BUILDER: usize = 1;
const REVIEWER: usize = 2;

/// The cases of `log --open`'s test (cli.rs), each against a pr claim of its own where it matters.
#[test]
fn findings_agrees_with_the_old_check_on_the_open_findings_cases() {
    let tmp = tempfile::tempdir().expect("a temp dir");
    let mut g = Ground::new(tmp.path());
    let old_pr = g.claim(BUILDER, "PR #1 a @ 1", &["pr"]);
    let pr = g.claim(BUILDER, "PR #1 a @ 2", &["pr"]);
    g.relate(BUILDER, pr, RelationKind::Supersedes, old_pr);
    let on = |g: &mut Ground, text: &str, target: ContributionId| {
        let f = g.claim(REVIEWER, text, &["finding"]);
        g.relate(REVIEWER, f, RelationKind::Contradicts, target);
        f
    };
    on(&mut g, "untriaged", pr);
    let f = on(&mut g, "confirmed", pr);
    g.evidence(MANAGER, f, true, "triage");
    let f = on(&mut g, "confirmed, on the old head", old_pr);
    g.evidence(MANAGER, f, true, "triage");
    let f = on(&mut g, "rejected", pr);
    g.evidence(MANAGER, f, false, "triage");
    let f = on(&mut g, "withdrawn", pr);
    g.retract(REVIEWER, f);
    let f = on(&mut g, "a builder's fail", pr);
    g.evidence(BUILDER, f, false, "triage");
    let f = on(&mut g, "pass then a builder's fail", pr);
    g.evidence(MANAGER, f, true, "triage");
    g.evidence(BUILDER, f, false, "triage");
    let f = on(&mut g, "the manager's pass and fail", pr);
    g.evidence(MANAGER, f, true, "triage");
    g.evidence(MANAGER, f, false, "triage");
    let f = on(&mut g, "its author's own pass", pr);
    g.evidence(REVIEWER, f, true, "triage");
    let f = g.claim(REVIEWER, "linked by the builder, who retracts the link", &["finding"]);
    let link = g.relate(BUILDER, f, RelationKind::Contradicts, pr);
    g.evidence(MANAGER, f, true, "triage");
    g.retract(BUILDER, link);
    let f = g.claim(REVIEWER, "its author retracts their link", &["finding"]);
    let link = g.relate(REVIEWER, f, RelationKind::Contradicts, pr);
    g.evidence(MANAGER, f, true, "triage");
    g.retract(REVIEWER, link);
    let f = g.claim(REVIEWER, "the manager links it, then retracts the link", &["finding"]);
    let link = g.relate(MANAGER, f, RelationKind::Contradicts, pr);
    g.retract(MANAGER, link);
    let f = on(&mut g, "deferred", pr);
    g.evidence(MANAGER, f, true, "triage");
    let task = g.claim(MANAGER, "later", &["task", "deferred"]);
    g.relate(MANAGER, task, RelationKind::DependsOn, f);
    let f = on(&mut g, "deferred to a task its author superseded", pr);
    g.evidence(MANAGER, f, true, "triage");
    let t1 = g.claim(MANAGER, "later", &["task", "deferred"]);
    g.relate(MANAGER, t1, RelationKind::DependsOn, f);
    let t2 = g.claim(MANAGER, "later, again", &["task"]);
    g.relate(MANAGER, t2, RelationKind::Supersedes, t1);
    let f = on(&mut g, "deferred to a retracted task, then to a live one", pr);
    g.evidence(MANAGER, f, true, "triage");
    let t1 = g.claim(MANAGER, "later", &["task", "deferred"]);
    g.relate(MANAGER, t1, RelationKind::DependsOn, f);
    g.retract(MANAGER, t1);
    let t2 = g.claim(MANAGER, "later", &["task", "deferred"]);
    g.relate(MANAGER, t2, RelationKind::DependsOn, f);
    let f = on(&mut g, "deferred to a task not tagged deferred", pr);
    g.evidence(MANAGER, f, true, "triage");
    let t = g.claim(MANAGER, "later", &["task"]);
    g.relate(MANAGER, t, RelationKind::DependsOn, f);
    let f = on(&mut g, "deferred by the builder", pr);
    g.evidence(MANAGER, f, true, "triage");
    let t = g.claim(BUILDER, "later", &["task", "deferred"]);
    g.relate(BUILDER, t, RelationKind::DependsOn, f);
    let f = on(&mut g, "a retracted triage", pr);
    let e = g.evidence(MANAGER, f, true, "triage");
    g.retract(MANAGER, e);
    for tags in [&["note"][..], &[][..]] {
        let f = g.claim(BUILDER, "a disagreement, not tagged finding", tags);
        g.relate(BUILDER, f, RelationKind::Contradicts, pr);
    }
    let f = on(&mut g, "linked twice, once retracted", pr);
    let link = g.relate(REVIEWER, f, RelationKind::Contradicts, pr);
    g.retract(REVIEWER, link);
    // A merged pr claim: the gate still reads its findings (log --open doesn't).
    let merged = g.claim(BUILDER, "PR #2 b @ 1", &["pr"]);
    let f = on(&mut g, "against a merged head", merged);
    g.evidence(MANAGER, f, true, "triage");
    g.evidence(MANAGER, merged, true, "merge gate @ 1");
    let nothing = g.claim(BUILDER, "PR #3 c @ 1", &["pr"]);
    let dir = g.store.dir().to_owned();
    drop(g);
    assert!(agree(&dir, &[pr, old_pr, merged, nothing]) >= 4, "the cases have findings to compare");

    // The text form: one line per finding, then the count.
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_bracken"))
            .args(args)
            .env("BRACKEN_DIR", &dir)
            .env_remove("BRACKEN_AGENT")
            .output()
            .expect("runs")
    };
    let text = String::from_utf8(run(&["findings", &pr.to_hex()]).stdout).expect("utf-8");
    let lines: Vec<&str> = text.lines().collect();
    let last = lines.last().expect("a summary line");
    assert!(last.contains("blocking"), "{text}");
    assert!(lines[..lines.len() - 1].iter().all(|l| l.contains(" blocks ") || l.contains(" ok ")), "{text}");
    assert!(lines.iter().any(|l| l.contains("deferred ->")), "a deferral names its task: {text}");
    let refused = run(&["findings", &pr.to_hex(), "--triager", "nobody"]);
    assert!(!refused.status.success() && String::from_utf8_lossy(&refused.stderr).contains("who triages"));
}

/// A small deterministic generator (xorshift64*).
struct Rng(u64);

impl Rng {
    fn below(&mut self, n: usize) -> usize {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        (self.0.wrapping_mul(0x2545_f491_4f6c_dd1d) % n as u64) as usize
    }
    fn pick<T: Copy>(&mut self, xs: &[T]) -> T {
        xs[self.below(xs.len())]
    }
}

/// Random Grounds: pr claims, findings and links by anyone, links and findings retracted by their authors,
/// triage by anyone (passes, fails, retracted), and deferral tasks by anyone, tagged deferred or not,
/// sometimes superseded or retracted.
#[test]
fn findings_agrees_with_the_old_check_on_random_grounds() {
    for seed in 1..=4u64 {
        let tmp = tempfile::tempdir().expect("a temp dir");
        let mut g = Ground::new(tmp.path());
        let mut rng = Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1);
        let agents = [0usize, 1, 2, 3];
        let mut prs = Vec::new();
        let mut findings: Vec<(ContributionId, usize)> = Vec::new();
        let mut written: Vec<(ContributionId, usize)> = Vec::new();
        let mut tasks: Vec<(ContributionId, usize)> = Vec::new();
        for _ in 0..4 {
            prs.push(g.claim(BUILDER, &format!("PR #{} x @ 1", prs.len() + 1), &["pr"]));
        }
        for step in 0..160 {
            let who = rng.pick(&agents);
            match rng.below(10) {
                0..=1 => {
                    let f =
                        g.claim(who, &format!("finding {step}"), if rng.below(4) == 0 { &[] } else { &["finding"] });
                    findings.push((f, who));
                    written.push((f, who));
                }
                2..=3 if !findings.is_empty() => {
                    let (f, _) = rng.pick(&findings);
                    let link = g.relate(who, f, RelationKind::Contradicts, rng.pick(&prs));
                    written.push((link, who));
                }
                4..=5 if !findings.is_empty() => {
                    let (f, _) = rng.pick(&findings);
                    let who = if rng.below(3) == 0 { who } else { MANAGER };
                    let e = g.evidence(who, f, rng.below(3) != 0, "triage");
                    written.push((e, who));
                }
                6 if !findings.is_empty() => {
                    let tags: &[&str] = match rng.below(3) {
                        0 => &["task"],
                        _ => &["task", "deferred"],
                    };
                    let owner = if rng.below(3) == 0 { who } else { MANAGER };
                    let t = g.claim(owner, &format!("task {step}"), tags);
                    tasks.push((t, owner));
                    written.push((t, owner));
                    let (f, _) = rng.pick(&findings);
                    let by = if rng.below(4) == 0 { who } else { owner };
                    let d = g.relate(by, t, RelationKind::DependsOn, f);
                    written.push((d, by));
                }
                7 if !tasks.is_empty() => {
                    // A task superseded by its author (it stops being current) or by someone else (it doesn't).
                    let (t, owner) = rng.pick(&tasks);
                    let by = if rng.below(2) == 0 { owner } else { who };
                    let next = g.claim(by, &format!("task {step}, again"), &["task"]);
                    g.relate(by, next, RelationKind::Supersedes, t);
                }
                8..=9 if !written.is_empty() => {
                    let (target, author) = rng.pick(&written);
                    let r = g.retract(author, target);
                    written.push((r, author));
                }
                _ => {}
            }
        }
        // A merge-gate pass on one pr claim: the gate still reads its findings.
        g.evidence(MANAGER, prs[0], true, "merge gate @ 1");
        let dir = g.store.dir().to_owned();
        drop(g);
        assert!(agree(&dir, &prs) > 0, "seed {seed} has findings to compare");
    }
}
