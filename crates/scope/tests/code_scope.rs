//! scripts/code-scope with bracken-scope (task 0837da3df7a4): a Rust file changed only in comments, from the
//! merge base to the head and from main's tip to the merged tree, names a doc's row and never needs the full
//! table, but its head is never docs-only, since clippy reads comments (finding c40ed7b8d3f8); anything else
//! stays code. One test per copy of the script (decision 4bf44934cb67 (5)).

use std::path::{Path, PathBuf};
use std::process::Command;

const TREE: &str = "//! The tree.\n\n/// Its name.\npub const NAME: &str = \"tree\";\n\n\
                    #[cfg(unix)]\npub fn f() -> u8 {\n    // one\n    1\n}\n";

/// The reviewer's case (finding c40ed7b8d3f8): the comment keeps clippy's collapsible_if quiet, so deleting it
/// fails `cargo clippy -- -D warnings`, though the tokens stay the same.
const CLIPPY: &str = "pub fn f(a: bool, b: bool) -> u8 {\n    if a {\n        // Why b matters.\n        \
                      if b {\n            return 1;\n        }\n    }\n    0\n}\n";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// A change to a file's text.
type Edit = dyn Fn(&str) -> String;

fn put(dir: &Path, path: &str, text: &str) {
    let file = dir.join(path);
    std::fs::create_dir_all(file.parent().expect("a parent")).expect("its directory");
    std::fs::write(file, text).expect("written");
}

struct Repo {
    _tmp: tempfile::TempDir,
    dir: PathBuf,
}

impl Repo {
    /// A repository holding `<scripts>/code-scope` as scripts/code-scope and a few crates, on main.
    fn new(scripts: &str) -> Repo {
        let tmp = tempfile::tempdir().expect("a temp dir");
        let dir = tmp.path().join("repo");
        std::fs::create_dir_all(dir.join("scripts")).expect("scripts/");
        // cp, so this process never holds the script open for writing while a parallel test forks.
        let from = repo_root().join(scripts).join("code-scope");
        assert!(Command::new("cp").arg(&from).arg(dir.join("scripts/code-scope")).status().expect("cp").success());
        put(&dir, "crates/tree/src/lib.rs", TREE);
        put(&dir, "crates/bracken/src/lib.rs", "pub fn g() {}\n");
        put(&dir, "crates/demo/src/broken.rs", "// broken\nfn f() {\n");
        put(&dir, "crates/demo/src/clippy.rs", CLIPPY);
        put(&dir, "crates/tree/src/merge.rs", "a x\n// c1\n// c2\n// c3\nb\n");
        put(&dir, "crates/experiment/src/plan.rs", "const TASK: &str = include_str!(\"task.rs\");\n");
        put(&dir, "crates/experiment/src/task.rs", "// The task.\npub fn t() {}\n");
        put(&dir, "README.md", "# Readme\n");
        let repo = Repo { _tmp: tmp, dir };
        for args in [
            &["init", "-q", "-b", "main"][..],
            &["config", "user.name", "t"],
            &["config", "user.email", "t@example.invalid"],
            &["add", "-A"],
            &["commit", "-qm", "main"],
        ] {
            repo.git(args);
        }
        repo
    }

    fn git(&self, args: &[&str]) {
        let out = Command::new("git").args(args).current_dir(&self.dir).output().expect("git");
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    }

    /// A head off main with `edits`, each a path and a change to its text from main's.
    fn head(&self, edits: &[(&str, &Edit)]) {
        self.git(&["checkout", "-q", "-B", "topic", "main"]);
        for (path, edit) in edits {
            let file = self.dir.join(path);
            let text = std::fs::read_to_string(&file).unwrap_or_default();
            put(&self.dir, path, &edit(&text));
        }
        self.git(&["add", "-A"]);
        self.git(&["commit", "-qm", "topic"]);
    }

    /// code-scope's exit code and output, with bracken-scope as `tokens` names it.
    fn scope(&self, args: &[&str], tokens: &str) -> (i32, String) {
        let out = Command::new(self.dir.join("scripts/code-scope"))
            .args(args)
            .current_dir(&self.dir)
            .env("CODE_SCOPE_TOKENS", tokens)
            .output()
            .expect("code-scope ran");
        (out.status.code().expect("an exit code"), String::from_utf8_lossy(&out.stdout).into_owned())
    }

    /// What the head is to code-scope: whether it needs the full table, its packages, whether it's docs-only.
    fn verdict(&self, tokens: &str) -> (bool, Vec<String>, bool) {
        let (full, out) = self.scope(&["full", "main", "topic"], tokens);
        assert!(full < 2, "{out}");
        let (code, packages) = self.scope(&["packages", "main", "topic"], tokens);
        assert_eq!(code, 0, "{packages}");
        let (docs, out) = self.scope(&["docs-only", "main", "topic"], tokens);
        assert!(docs < 2, "{out}");
        (full == 0, packages.lines().map(str::to_owned).collect(), docs == 0)
    }
}

const HELPER: &str = env!("CARGO_BIN_EXE_bracken-scope");

fn comment_only_changes_are_a_docs_row(scripts: &str) {
    let repo = Repo::new(scripts);
    let comment = |t: &str| t.replace("// one", "// one, the\n    // only one");
    let tree = vec!["bracken".to_owned(), "bracken-tree".to_owned()];
    let bracken = vec!["bracken".to_owned(), "bracken-demo".to_owned(), "bracken-experiment".to_owned()];

    // No full table, a doc's row, and never docs-only.
    repo.head(&[("crates/tree/src/lib.rs", &comment)]);
    assert_eq!(repo.verdict(HELPER), (false, tree.clone(), false), "{scripts}: a comment in crates/tree");
    let layout = |t: &str| t.replace("    1\n}", "    1 }");
    repo.head(&[("crates/tree/src/lib.rs", &layout)]);
    assert_eq!(repo.verdict(HELPER), (false, tree.clone(), false), "{scripts}: layout");
    repo.head(&[("crates/tree/src/lib.rs", &layout), ("README.md", &|t: &str| format!("{t}More.\n"))]);
    assert_eq!(repo.verdict(HELPER), (false, tree.clone(), false), "{scripts}: layout and docs");
    repo.head(&[
        ("crates/tree/src/lib.rs", &comment),
        ("crates/bracken/src/lib.rs", &|_: &str| "pub fn h() {}\n".into()),
    ]);
    let all = ["bracken", "bracken-demo", "bracken-experiment", "bracken-tree"].map(str::to_owned).to_vec();
    assert_eq!(repo.verdict(HELPER), (false, all, false), "{scripts}: a comment and code elsewhere");
    // The reviewer's collapsible_if case: comment-only by its tokens, so a doc's row, but not docs-only, so
    // its light run lints it.
    repo.head(&[("crates/demo/src/clippy.rs", &|t: &str| t.replace("        // Why b matters.\n", ""))]);
    assert_eq!(repo.verdict(HELPER), (false, tree.clone(), false), "{scripts}: a comment clippy reads");
    let (docs, out) = repo.scope(&["docs-only", "main", "topic"], HELPER);
    assert_eq!((docs, out.trim()), (1, "code-scope: not docs: crates/demo/src/clippy.rs"), "{scripts}");

    // Code, each: a doc comment, inner or outer; a string literal; a cfg; a blank line after an attribute.
    for (what, edit) in [
        ("an outer doc comment", &(|t: &str| t.replace("/// Its name.", "/// Its own name.")) as &Edit),
        ("an inner doc comment", &|t: &str| t.replace("//! The tree.", "//! The tree, here.")),
        ("a string literal", &|t: &str| t.replace("\"tree\"", "\"trees\"")),
        ("a cfg", &|t: &str| t.replace("cfg(unix)", "cfg(windows)")),
        ("a blank line after an attribute", &|t: &str| t.replace("(unix)]\n", "(unix)]\n\n")),
    ] {
        repo.head(&[("crates/tree/src/lib.rs", edit)]);
        assert_eq!(repo.verdict(HELPER), (true, tree.clone(), false), "{scripts}: {what}");
    }
    // A file that doesn't lex, and one included as text, are code whatever changed in them.
    repo.head(&[("crates/demo/src/broken.rs", &|t: &str| t.replace("// broken", "// still broken"))]);
    assert_eq!(repo.verdict(HELPER), (false, bracken.clone(), false), "{scripts}: a file that doesn't lex");
    repo.head(&[("crates/experiment/src/task.rs", &|t: &str| t.replace("// The task.", "// The task, here."))]);
    assert_eq!(repo.verdict(HELPER), (false, bracken.clone(), false), "{scripts}: a file included as text");

    // Without bracken-scope, every Rust file is code.
    repo.head(&[("crates/tree/src/lib.rs", &comment)]);
    assert_eq!(repo.verdict(""), (true, tree.clone(), false), "{scripts}: no bracken-scope");

    // Comment-only from the merge base to the head, but not in the merged tree (finding 24e15486a4a4): the
    // head moves x across the comments, main makes one of them code, and git merges the two cleanly into
    // tokens neither side has. So it's code, and needs the full table.
    let moved = |t: &str| t.replace("a x\n", "a\n").replace("\nb\n", "\nx b\n");
    repo.head(&[("crates/tree/src/merge.rs", &moved)]);
    assert_eq!(repo.verdict(HELPER), (false, tree.clone(), false), "{scripts}: before main moves");
    repo.git(&["checkout", "-q", "main"]);
    let file = repo.dir.join("crates/tree/src/merge.rs");
    let text = std::fs::read_to_string(&file).expect("read");
    std::fs::write(&file, text.replace("// c2", "y")).expect("written");
    repo.git(&["commit", "-qam", "main moves"]);
    assert_eq!(repo.verdict(HELPER), (true, tree, false), "{scripts}: tokens neither side tested");
}

#[test]
fn comment_only_changes_are_a_docs_row_in_scripts() {
    comment_only_changes_are_a_docs_row("scripts");
}

#[test]
fn comment_only_changes_are_a_docs_row_in_the_kickoff_templates() {
    comment_only_changes_are_a_docs_row(".claude/skills/agent-team-kickoff/templates");
}

/// bracken-scope comment-only answers for one file: yes, or no with why.
#[test]
fn bracken_scope_answers_for_one_file() {
    let repo = Repo::new("scripts");
    repo.head(&[
        ("crates/tree/src/lib.rs", &|t: &str| t.replace("// one", "// uno")),
        ("crates/tree/src/new.rs", &|_: &str| "// new\n".into()),
        ("crates/bracken/src/lib.rs", &|_: &str| "pub fn h() {}\n".into()),
    ]);
    let ask = |file: &str| {
        let out = Command::new(HELPER)
            .args(["comment-only", "main", "topic", file])
            .current_dir(&repo.dir)
            .output()
            .expect("bracken-scope ran");
        (out.status.code().expect("an exit code"), String::from_utf8_lossy(&out.stdout).trim().to_owned())
    };
    assert_eq!(ask("crates/tree/src/lib.rs"), (0, "yes".into()));
    assert_eq!(ask("crates/tree/src/new.rs"), (1, "no: added or deleted".into()));
    assert_eq!(ask("crates/bracken/src/lib.rs"), (1, "no: its tokens changed, or a side doesn't lex".into()));
    assert_eq!(ask("README.md"), (1, "no: not a Rust file".into()));
    assert_eq!(ask("crates/demo/src/broken.rs"), (1, "no: not changed, or more than one path".into()));
}
