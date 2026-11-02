//! These need Docker, as `python::tests` do: where it isn't running, each says so and passes without checking
//! anything.

/// Other containers' starts move the host's deadline by one more of its wall time at most, however many there are
/// (finding 9391e821d0da): a server crashed on every call, restarted each time, can't stretch a group without end.
#[test]
fn the_hosts_deadline_moves_a_bounded_amount() {
    use std::time::Duration;
    let mut allowance = super::Allowance(Duration::from_secs(4));
    let moved: Vec<Duration> = [3, 3, 3].into_iter().map(|s| allowance.take(Duration::from_secs(s))).collect();
    assert_eq!(moved, [3, 1, 0].map(Duration::from_secs));
}

/// In `macos_only` though they don't need macOS, so the counted run's Mac side runs them: Docker is on this Mac
/// and not in the test boxes, where they skip (decision ac22cedd8fee, docs/development.md).
mod macos_only {
    use std::path::{Path, PathBuf};

    use serde_json::{Value, json};

    use super::super::*;
    use crate::project::digest;
    use crate::python::tests::macos_only::python;

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("a directory");
        std::fs::write(path, text).expect("a file");
    }

    /// `sum` and `mean-empty` at the start, and `decimals` at half the run, changing `sum`.
    fn manifest(digest: &str, limits: &Value) -> Value {
        json!({
            "format": 1, "name": "tiny", "python": "3.12", "package": "tiny", "spec": "spec.md", "seed": "seed",
            "items": [
                {"id": "sum", "kind": "feature", "title": "Sum", "text": "items/sum.md", "release": {"at": "start"}},
                {"id": "mean-empty", "kind": "fix", "title": "Mean", "text": "items/mean-empty.md",
                 "release": {"at": "start"}},
                {"id": "decimals", "kind": "change", "title": "Decimals", "text": "items/decimals.md",
                 "release": {"after_share": 0.5}, "revises": ["sum"]},
            ],
            "limits": limits,
            "hidden": {"id": "tiny@1", "digest": digest},
        })
    }

    /// The package's errors, and `parse`, which raises the standard library's own on bad input.
    const ERRORS: &str = "import decimal, json\n\nclass StatsError(Exception):\n    pass\n\n\
        class EmptyError(StatsError, ValueError):\n    pass\n\n\
        def parse(s):\n    return decimal.Decimal(s)\n\n\
        def load(s):\n    return json.loads(s)\n\n";
    /// The package as it is before the change: whole numbers.
    const BEFORE: &str = "def sum_of(xs):\n    return sum(int(x) for x in xs)\n\n\
        def mean(xs):\n    if not xs:\n        raise EmptyError('no numbers')\n    return sum_of(xs) / len(xs)\n";
    /// After it: decimals.
    const AFTER: &str = "def sum_of(xs):\n    return sum((decimal.Decimal(str(x)) for x in xs), decimal.Decimal(0))\n\n\
        def mean(xs):\n    if not xs:\n        raise EmptyError('no numbers')\n    return sum_of(xs) / len(xs)\n";
    /// The command: `python -m tiny sum <x>…` prints the sum and writes it to `total.txt`; bad input exits 2.
    const MAIN: &str = "import sys\nfrom tiny.stats import sum_of\n\
        try:\n    total = sum_of(sys.argv[2:])\nexcept Exception as e:\n    print(type(e).__name__, file=sys.stderr)\n    sys.exit(2)\n\
        print(total)\nopen('total.txt', 'w').write(str(total))\n";

    /// Every group: `sum`'s old and new tests, `mean-empty`'s by the error's class, and `decimals`'s through
    /// the command.
    fn groups() -> Value {
        json!([
            {"item": "sum", "file": "tests/test_sum_before.py", "applies": "before-change"},
            {"item": "sum", "file": "tests/test_sum_after.py", "applies": "after-change"},
            {"item": "mean-empty", "file": "tests/test_mean.py", "applies": "always"},
            {"item": "decimals", "file": "tests/test_command.py", "applies": "always"},
        ])
    }

    const TESTS: [(&str, &str); 4] = [
        (
            "test_sum_before.py",
            "import unittest\nfrom tiny.stats import sum_of\n\nclass T(unittest.TestCase):\n\
             \x20   def test_whole(self):\n        self.assertEqual(sum_of([1, 2]), 3)\n",
        ),
        (
            "test_sum_after.py",
            "import decimal, unittest\nfrom tiny.stats import sum_of\n\nclass T(unittest.TestCase):\n\
             \x20   def test_decimal(self):\n        self.assertEqual(sum_of(['1.5', '2']), decimal.Decimal('3.5'))\n",
        ),
        (
            "test_mean.py",
            "import unittest\nimport tiny.stats as stats\n\nclass T(unittest.TestCase):\n\
             \x20   def test_by_name(self):\n        with self.assertRaises(Exception) as cm:\n            stats.mean([])\n\
             \x20       self.assertEqual(type(cm.exception).__name__, 'EmptyError')\n\
             \x20       self.assertEqual(str(cm.exception), 'no numbers')\n\
             \x20   def test_as_a_value_error(self):\n        with self.assertRaises(ValueError):\n            stats.mean([])\n\
             \x20   def test_the_class(self):\n        with self.assertRaises(stats.EmptyError):\n            stats.mean([])\n\
             \x20   def test_its_package_base(self):\n        with self.assertRaises(stats.StatsError):\n            stats.mean([])\n\
             \x20   def test_the_standard_librarys_own(self):\n        import decimal\n\
             \x20       with self.assertRaises(decimal.InvalidOperation):\n            stats.parse('x')\n\
             \x20   def test_one_made_with_other_arguments(self):\n        import json\n\
             \x20       with self.assertRaises(json.JSONDecodeError):\n            stats.load('x')\n",
        ),
        (
            "test_command.py",
            "import os, subprocess, sys, tempfile, unittest\n\nclass T(unittest.TestCase):\n\
             \x20   def test_sum(self):\n        d = tempfile.mkdtemp()\n\
             \x20       out = subprocess.run([sys.executable, '-m', 'tiny', 'sum', '1.5', '2'], cwd=d,\n\
             \x20                            capture_output=True, text=True)\n\
             \x20       self.assertEqual((out.returncode, out.stdout), (0, '3.5\\n'))\n\
             \x20       self.assertEqual(open(os.path.join(d, 'total.txt')).read(), '3.5')\n\
             \x20   def test_bad_input(self):\n\
             \x20       out = subprocess.run([sys.executable, '-m', 'tiny', 'sum', 'x'], capture_output=True)\n\
             \x20       self.assertEqual(out.returncode, 2)\n        self.assertIn(b'InvalidOperation', out.stderr)\n",
        ),
    ];

    struct Setup {
        _tmp: tempfile::TempDir,
        root: PathBuf,
        project: Project,
        set: HiddenSet,
    }

    /// A project, and its store holding `tests` (by file name, each with `{ROOT}` the setup's directory),
    /// `groups`, and the golden trees.
    fn setup(groups: Value, tests: &[(&str, &str)]) -> Setup {
        setup_with(groups, tests, &json!({"cpu_seconds": 5, "wall_seconds": 30}))
    }

    /// [`setup`] with the project's `limits`.
    fn setup_with(groups: Value, tests: &[(&str, &str)], limits: &Value) -> Setup {
        let tmp = tempfile::tempdir().expect("a temp dir");
        let root = std::fs::canonicalize(tmp.path()).expect("resolved");
        let set = root.join("store/tiny@1");
        write(&set.join("hidden.json"), &json!({"format": 1, "project": "tiny", "groups": groups}).to_string());
        for (file, text) in tests {
            write(&set.join("tests").join(file), &text.replace("{ROOT}", &root.display().to_string()));
        }
        for (tree, stats) in [("golden", AFTER), ("golden-before", BEFORE)] {
            write(&set.join(tree).join("tiny/__init__.py"), "");
            write(&set.join(tree).join("tiny/stats.py"), &format!("{ERRORS}{stats}"));
            write(&set.join(tree).join("tiny/__main__.py"), MAIN);
        }
        let p = root.join("tiny");
        write(&p.join("project.json"), &manifest(&digest(&set).expect("a digest"), limits).to_string());
        write(&p.join("spec.md"), "# tiny\n");
        for id in ["sum", "mean-empty", "decimals"] {
            write(&p.join(format!("items/{id}.md")), "an issue\n");
        }
        write(&p.join("seed/tiny/__init__.py"), "");
        write(&p.join("seed/tiny/stats.py"), "def sum_of(xs):\n    return 0\n");
        write(&p.join("seed/tests/test_tiny.py"), "import unittest\n");
        let project = Project::load(&p).expect("the project");
        let set = HiddenSet::open(&project, &root.join("store")).expect("the hidden set");
        Setup { _tmp: tmp, root, project, set }
    }

    fn indexes(g: &Grade) -> Vec<usize> {
        g.groups.iter().map(|g| g.index).collect()
    }

    /// The approval check (answer c473d137f5b7): `golden/` passes everything at the end, and `golden-before/`
    /// everything released just before the change, which leaves out the change's own group, not yet released
    /// (note 9891289fb39c). Errors are checked by class, by name, as a `ValueError`, by the package's base class
    /// and as the standard library's own class, one made with other arguments too (findings 621c185d3e6f,
    /// ed723694b473); the command runs with its working directory's files coming back.
    #[test]
    fn the_golden_trees_pass_the_approval_check() {
        let Some(python) = python() else { return };
        let s = setup(groups(), &TESTS);
        let check = golden_check(&python, &s.project, &s.set).expect("graded");
        assert!(check.passed(), "{check:#?}");
        assert_eq!(indexes(&check.after), [1, 2, 3], "after the change: sum's new tests, mean's, the command's");
        let before = check.before.expect("the project has a change");
        assert_eq!(indexes(&before), [0, 2], "before it: sum's old tests and mean's; not decimals', unreleased");
        let mean = check.after.groups.iter().find(|g| g.index == 2).expect("mean's group");
        assert_eq!(mean.counts, Counts { pass: 6, ..Counts::default() });
        let command = check.after.groups.iter().find(|g| g.index == 3).expect("the command's group");
        assert_eq!(command.counts, Counts { pass: 2, ..Counts::default() });
    }

    /// A tree is graded per item, in counts: the seed fails or errs in each group; the tree from before the
    /// change, graded after it, fails just what changed.
    #[test]
    fn a_tree_is_graded_per_item_in_counts() {
        let Some(python) = python() else { return };
        let s = setup(groups(), &TESTS);
        let seed = grade(&python, &s.project, &s.set, Tree::Dir(&s.project.seed), 1.0).expect("graded");
        assert!(!seed.all_pass());
        assert_eq!(seed.items.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(), ["sum", "mean-empty", "decimals"]);
        for item in &seed.items {
            assert_eq!(item.counts.pass, 0, "{item:?}");
            assert!(item.counts.fail + item.counts.error > 0, "{item:?}");
        }
        let old = grade(&python, &s.project, &s.set, Tree::Hidden("golden-before"), 1.0).expect("graded");
        let by = |id: &str| old.items.iter().find(|i| i.id == id).expect("an item").counts;
        assert_eq!(by("mean-empty"), Counts { pass: 6, ..Counts::default() });
        // Its whole-number `int` raises on "1.5": an error, not a failure.
        assert_eq!(by("sum"), Counts { error: 1, ..Counts::default() });
        assert_eq!(by("decimals"), Counts { pass: 0, fail: 2, ..Counts::default() });
    }

    /// Agent code reads nothing of the hidden set, imported or run as a command (note 9891289fb39c (1)): not
    /// the store, nor the host's copy of the tests, while it can read its own tree.
    #[test]
    fn agent_code_reads_no_hidden_test_imported_or_as_a_command() {
        let Some(python) = python() else { return };
        const PEEK: &str = "import os, subprocess, sys, tempfile, unittest\nimport tiny.peek as peek\n\
            STORE = '{ROOT}/store/tiny@1/tests/test_peek.py'\n\nclass T(unittest.TestCase):\n\
            \x20   def test_own_tree(self):\n        self.assertIs(peek.own(), True)\n\
            \x20   def test_import(self):\n        self.assertIs(peek.can_read(STORE), False)\n\
            \x20       self.assertIs(peek.can_read(os.path.abspath(__file__)), False)\n\
            \x20   def test_command(self):\n        for path in (STORE, os.path.abspath(__file__)):\n\
            \x20           out = subprocess.run([sys.executable, '-m', 'tiny.peek', path], capture_output=True, text=True)\n\
            \x20           self.assertEqual(out.stdout, 'no\\n', out.stderr)\n\
            \x20   def test_relative(self):\n\
            \x20       for path in ('tests/test_peek.py', '../tests/test_peek.py', 'test_peek.py'):\n\
            \x20           out = subprocess.run([sys.executable, '-m', 'tiny.peek', path], capture_output=True, text=True)\n\
            \x20           self.assertEqual(out.stdout, 'no\\n', out.stderr)\n\
            \x20   def test_an_empty_directory(self):\n\
            \x20       out = subprocess.run([sys.executable, '-m', 'tiny.ls'], capture_output=True, text=True)\n\
            \x20       self.assertEqual(out.stdout, '[]\\n', out.stderr)\n\
            \x20   def test_files_given_back_stay_in_its_directory(self):\n\
            \x20       d = tempfile.mkdtemp()\n\
            \x20       subprocess.run([sys.executable, '-m', 'tiny.write', 'test_peek.py'], cwd=d, capture_output=True)\n\
            \x20       self.assertEqual(open(os.path.join(d, 'test_peek.py')).read(), 'written')\n\
            \x20       self.assertIn('STORE =', open(os.path.abspath(__file__)).read())\n\
            \x20   def test_not_where_the_tests_are(self):\n\
            \x20       for d in (os.path.dirname(os.path.abspath(__file__)), os.path.dirname(os.path.dirname(os.path.abspath(__file__)))):\n\
            \x20           out = subprocess.run([sys.executable, '-m', 'tiny.ls'], cwd=d, capture_output=True, text=True)\n\
            \x20           self.assertEqual(out.stdout, '[]\\n', out.stderr)\n\
            \x20           subprocess.run([sys.executable, '-m', 'tiny.write', 'test_peek.py'], cwd=d, capture_output=True)\n\
            \x20       self.assertIn('STORE =', open(os.path.abspath(__file__)).read())\n\
            \x20   def test_no_hidden_file_by_its_absolute_path(self):\n\
            \x20       for path in (os.path.abspath(__file__), os.path.dirname(os.path.abspath(__file__))):\n\
            \x20           out = subprocess.run([sys.executable, '-m', 'tiny.peek', path], capture_output=True, text=True)\n\
            \x20           self.assertEqual(out.stdout, 'no\\n', out.stderr)\n";
        let groups = json!([{"item": "mean-empty", "file": "tests/test_peek.py", "applies": "always"}]);
        let s = setup(groups, &[("test_peek.py", PEEK)]);
        let store_file = s.root.join("store/tiny@1/tests/test_peek.py");
        assert!(std::fs::File::open(&store_file).is_ok(), "the store is readable outside the sandbox");
        let tree = s.root.join("tree");
        write(&tree.join("tiny/__init__.py"), "");
        write(
            &tree.join("tiny/peek.py"),
            "import sys\n\ndef can_read(path):\n    try:\n        open(path, 'rb').read(1)\n        return True\n\
             \x20   except OSError:\n        return False\n\ndef own():\n    return can_read(__file__)\n\n\
             if __name__ == '__main__':\n    print('yes' if can_read(sys.argv[1]) else 'no')\n",
        );
        write(&tree.join("tiny/ls.py"), "import json, os\nprint(json.dumps(sorted(os.listdir('.'))))\n");
        write(&tree.join("tiny/write.py"), "import sys\nopen(sys.argv[1], 'w').write('written')\n");
        let g = grade(&python, &s.project, &s.set, Tree::Dir(&tree), 1.0).expect("graded");
        assert_eq!(g.groups[0].counts, Counts { pass: 8, ..Counts::default() }, "{g:#?}");
    }

    /// Hidden tests shaped as a project's own often are (task `6a80d88373a5`): the package `tests`, with helpers
    /// beside them imported by name, from `tests.` and relatively; the package imported at a helper's top and a
    /// test's; a command run through `subprocess` from the project's root, as a helper finds it from
    /// `__file__`, on a temporary file, writing another; and decimals compared here.
    #[test]
    fn tests_shaped_as_a_projects_own_are_graded() {
        let Some(python) = python() else { return };
        const SUPPORT: &str = "import os, subprocess, sys, tempfile\nimport tiny\nfrom tiny.stats import sum_of\n\
            ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))\n\n\
            def total(text):\n    with tempfile.TemporaryDirectory() as d:\n\
            \x20       given, made = os.path.join(d, 'in.txt'), os.path.join(d, 'out.txt')\n\
            \x20       open(given, 'w').write(text)\n\
            \x20       out = subprocess.run([sys.executable, '-m', 'tiny.io', given, made], cwd=ROOT,\n\
            \x20                            capture_output=True, text=True, env=dict(os.environ, PYTHONPATH=ROOT))\n\
            \x20       written = open(made).read() if os.path.exists(made) else None\n\
            \x20       return out.returncode, out.stdout.replace(given, 'IN'), out.stderr, written\n\n\
            def missing():\n    with tempfile.TemporaryDirectory() as d:\n        path = os.path.join(d, 'none.txt')\n\
            \x20       out = subprocess.run([sys.executable, '-m', 'tiny.io', path, path], capture_output=True, text=True)\n\
            \x20       return out.returncode, out.stderr.replace(path, 'NONE')\n";
        const COMMON: &str = "import decimal\nfrom tiny import stats\nHALF = decimal.Decimal('0.5')\n";
        // A helper that finds the project from the package's file at its top, and runs the command from there.
        const FROM_FILE: &str = "import os, subprocess, sys, tempfile\nfrom pathlib import Path\nimport tiny\n\
            import tiny.stats\nPACKAGE = Path(tiny.__file__).resolve().parent\nROOT = PACKAGE.parent\n\
            STATS = tiny.stats.__file__\nPATH = list(tiny.__path__)\n\n\
            def total(text):\n    with tempfile.TemporaryDirectory() as d:\n\
            \x20       given, made = os.path.join(d, 'in.txt'), os.path.join(d, 'out.txt')\n\
            \x20       open(given, 'w').write(text)\n\
            \x20       out = subprocess.run([sys.executable, '-m', 'tiny.io', given, made], cwd=ROOT,\n\
            \x20                            capture_output=True, text=True, env=dict(os.environ, PYTHONPATH=str(ROOT)))\n\
            \x20       return out.returncode, open(made).read() if os.path.exists(made) else out.stderr\n";
        const TEST_FROM_FILE: &str = "import os, unittest\nimport from_file\n\n\
            class T(unittest.TestCase):\n\
            \x20   def test_the_package_has_a_place(self):\n\
            \x20       self.assertEqual(from_file.PACKAGE.name, 'tiny')\n\
            \x20       self.assertEqual(os.path.basename(from_file.STATS), 'stats.py')\n\
            \x20       self.assertEqual(os.path.dirname(from_file.STATS), str(from_file.PACKAGE))\n\
            \x20       self.assertEqual(from_file.PATH, [str(from_file.PACKAGE)])\n\
            \x20       self.assertTrue(from_file.PACKAGE.is_dir())\n\
            \x20       self.assertEqual(os.listdir(from_file.PACKAGE), [], 'no file of the tree')\n\
            \x20   def test_not_the_tests_nor_the_tree(self):\n\
            \x20       tests = os.path.dirname(os.path.abspath(__file__))\n\
            \x20       root = str(from_file.ROOT)\n\
            \x20       self.assertFalse(root.startswith(tests) or tests.startswith(root + os.sep), root)\n\
            \x20       self.assertNotIn('{ROOT}/tree', root)\n\
            \x20   def test_a_command_from_there(self):\n\
            \x20       self.assertEqual(from_file.total('1 2.5'), (0, '3.5'))\n";
        const TEST: &str = "import decimal, unittest\nimport tiny\nfrom tiny.stats import sum_of\n\
            from support import total\nfrom tests.support import missing\nfrom .common import HALF\n\n\
            class T(unittest.TestCase):\n\
            \x20   def test_a_command_on_temporary_files(self):\n\
            \x20       self.assertEqual(total('1.5 2'), (0, 'read IN\\n', '', '3.5'))\n\
            \x20   def test_its_error_names_the_path_given(self):\n\
            \x20       self.assertEqual(missing(), (2, 'no file NONE\\n'))\n\
            \x20   def test_decimals(self):\n\
            \x20       self.assertEqual(sum_of(['1.5', '2']), decimal.Decimal('3.5'))\n\
            \x20       self.assertIsInstance(sum_of([HALF]), decimal.Decimal)\n\
            \x20       self.assertEqual(sum_of([HALF, HALF]), 1)\n\
            \x20       self.assertEqual(tiny.stats.sum_of([]) + HALF, HALF)\n";
        let groups = json!([
            {"item": "decimals", "file": "tests/sub/test_shapes.py", "applies": "always"},
            {"item": "decimals", "file": "tests/test_from_file.py", "applies": "always"},
        ]);
        let s = setup(
            groups,
            &[
                ("__init__.py", ""),
                ("support.py", SUPPORT),
                ("sub/common.py", COMMON),
                ("sub/test_shapes.py", TEST),
                ("from_file.py", FROM_FILE),
                ("test_from_file.py", TEST_FROM_FILE),
            ],
        );
        let tree = s.root.join("tree");
        write(&tree.join("tiny/__init__.py"), "");
        write(&tree.join("tiny/stats.py"), &format!("{ERRORS}{AFTER}"));
        write(
            &tree.join("tiny/io.py"),
            "import sys\nfrom tiny.stats import sum_of\ngiven, made = sys.argv[1:3]\ntry:\n    text = open(given).read()\n\
             except OSError:\n    print('no file', given, file=sys.stderr)\n    sys.exit(2)\n\
             print('read', given)\nopen(made, 'w').write(str(sum_of(text.split())))\n",
        );
        let g = grade(&python, &s.project, &s.set, Tree::Dir(&tree), 1.0).expect("graded");
        assert_eq!(g.groups[0].counts, Counts { pass: 3, ..Counts::default() }, "{g:#?}");
        assert_eq!(g.groups[1].counts, Counts { pass: 3, ..Counts::default() }, "{g:#?}");
        assert!(g.groups.iter().all(|g| g.diagnosis.is_empty()), "{g:#?}");
    }

    /// A test's error or failure is described for `--diagnose` too, with its index, its message cut to 300
    /// characters (task `6a80d88373a5`); a subtest's under its test's index.
    #[test]
    fn a_tests_error_or_failure_is_described_with_its_index() {
        let Some(python) = python() else { return };
        let test = "import unittest\nimport tiny.stats as stats\n\nclass T(unittest.TestCase):\n\
                    \x20   def test_a_pass(self):\n        self.assertEqual(stats.f(), 1)\n\
                    \x20   def test_b_fail(self):\n        self.fail('x' * 500)\n\
                    \x20   def test_c_error(self):\n        stats.g()\n\
                    \x20   def test_d_subtest(self):\n        with self.subTest(i=1):\n            self.assertTrue(False)\n";
        let g = graded(&python, "def f():\n    return 1\n", test, &json!({"cpu_seconds": 5, "wall_seconds": 30}));
        assert_eq!(g.counts, Counts { pass: 1, fail: 2, error: 1, ..Counts::default() }, "{g:#?}");
        let by: Vec<(Value, Value)> = g.diagnosis.iter().map(|d| (d["test"].clone(), d["class"].clone())).collect();
        assert_eq!(
            by,
            [
                (json!(1), json!("builtins.AssertionError")),
                (json!(2), json!("builtins.AttributeError")),
                (json!(3), json!("builtins.AssertionError")),
            ],
            "{g:#?}"
        );
        assert!(g.diagnosis[0]["message"].as_str().is_some_and(|m| m.chars().count() == 300), "{g:#?}");
        let last = g.diagnosis[1]["frames"].as_array().and_then(|f| f.iter().rev().find(|f| f[0] == "tests/test_x.py"));
        assert_eq!(last.map(|f| &f[1]), Some(&json!(10)), "{g:#?}");
    }

    /// A text stream given to the code, or standard output redirected to one, holds what the code wrote there
    /// once the call returns or raises; and an exception keeps its attributes of plain data (task
    /// `6a80d88373a5`): as a command line's in-process `main(argv, out=io.StringIO())` is tested.
    #[test]
    fn a_text_stream_given_to_the_code_holds_what_it_wrote() {
        let Some(python) = python() else { return };
        let stats = "import json, sys\n\nclass ParseError(ValueError):\n    def __init__(self, message, line):\n\
                     \x20       super().__init__(message)\n        self.line = line\n\n\
                     def main(argv, out=None, err=None):\n    out = out or sys.stdout\n    err = err or sys.stderr\n\
                     \x20   if argv == ['bad']:\n        print('no such command', file=err)\n        return 2\n\
                     \x20   if argv == ['raise']:\n        out.write('partial')\n        raise ParseError('bad line', 7)\n\
                     \x20   print(json.dumps({'total': '3.50', 'argv': argv}), file=out)\n    return 0\n";
        let test = "import contextlib, io, json, unittest\nfrom tiny.stats import main, ParseError\n\n\
                    class T(unittest.TestCase):\n\
                    \x20   def test_out_and_err(self):\n        out, err = io.StringIO(), io.StringIO()\n\
                    \x20       self.assertEqual(main(['balance'], out=out, err=err), 0)\n\
                    \x20       self.assertEqual(json.loads(out.getvalue()), {'total': '3.50', 'argv': ['balance']})\n\
                    \x20       self.assertEqual(err.getvalue(), '')\n\
                    \x20   def test_err(self):\n        out, err = io.StringIO(), io.StringIO()\n\
                    \x20       self.assertEqual(main(['bad'], out=out, err=err), 2)\n\
                    \x20       self.assertEqual((out.getvalue(), err.getvalue()), ('', 'no such command\\n'))\n\
                    \x20   def test_appends(self):\n        out = io.StringIO()\n        out.write('before ')\n\
                    \x20       main(['x'], out=out, err=out)\n        self.assertTrue(out.getvalue().startswith('before {'))\n\
                    \x20   def test_redirected(self):\n        out = io.StringIO()\n\
                    \x20       with contextlib.redirect_stdout(out):\n            self.assertEqual(main(['r']), 0)\n\
                    \x20       self.assertEqual(json.loads(out.getvalue())['argv'], ['r'])\n\
                    \x20   def test_raised_keeps_what_was_written_and_its_line(self):\n        out = io.StringIO()\n\
                    \x20       with self.assertRaises(ParseError) as cm:\n            main(['raise'], out=out)\n\
                    \x20       self.assertEqual((out.getvalue(), cm.exception.line, str(cm.exception)), ('partial', 7, 'bad line'))\n";
        let g = graded(&python, stats, test, &json!({"cpu_seconds": 5, "wall_seconds": 30}));
        assert_eq!(g.counts, Counts { pass: 5, ..Counts::default() }, "{g:#?}");
    }

    /// A path in a call's arguments, such as a temporary file's in a command line's argv, is given as the
    /// server's own, and what the code wrote there comes back; an error naming it names the test's (task
    /// `6a80d88373a5`). Never a hidden test's, nor a path outside the host's own directory.
    #[test]
    fn a_path_given_to_the_code_is_its_own() {
        let Some(python) = python() else { return };
        let stats = "import os\n\ndef main(argv, out):\n    path, made = argv[1], argv[2]\n    try:\n\
                     \x20       text = open(path).read()\n    except OSError:\n        raise ValueError('cannot read ' + path)\n\
                     \x20   open(made, 'w').write(text.upper())\n    out.write(path + ': ' + text)\n    return 0\n\n\
                     def read(path):\n    return open(os.fspath(path)).read()\n\n\
                     def same(s):\n    return s\n";
        let test = "import io, os, pathlib, tempfile, unittest\nfrom tiny.stats import main, read, same\n\n\
                    class T(unittest.TestCase):\n\
                    \x20   def test_a_temporary_file(self):\n        with tempfile.TemporaryDirectory() as d:\n\
                    \x20           path, made = os.path.join(d, 'books.journal'), os.path.join(d, 'out.txt')\n\
                    \x20           open(path, 'w').write('books')\n            out = io.StringIO()\n\
                    \x20           self.assertEqual(main(['-f', path, made], out=out), 0)\n\
                    \x20           self.assertEqual(out.getvalue(), path + ': books')\n\
                    \x20           self.assertEqual(open(made).read(), 'BOOKS')\n\
                    \x20           self.assertEqual(read(pathlib.Path(path)), 'books')\n\
                    \x20           self.assertEqual(sorted(os.listdir(d)), ['books.journal', 'out.txt'])\n\
                    \x20   def test_an_error_names_the_tests_path(self):\n        with tempfile.TemporaryDirectory() as d:\n\
                    \x20           none = os.path.join(d, 'none')\n\
                    \x20           with self.assertRaises(ValueError) as cm:\n                main(['-f', none, none], out=io.StringIO())\n\
                    \x20           self.assertEqual(str(cm.exception), 'cannot read ' + none)\n\
                    \x20   def test_not_a_hidden_file_nor_one_outside(self):\n\
                    \x20       for path in (os.path.abspath(__file__), '{ROOT}/store/tiny@1/tests/test_x.py'):\n\
                    \x20           with self.assertRaises(Exception):\n                read(path)\n\
                    \x20       self.assertEqual(same('/'), '/')\n        self.assertEqual(same(['/etc']), ['/etc'])\n";
        let g = graded(&python, stats, test, &json!({"cpu_seconds": 5, "wall_seconds": 30}));
        assert_eq!(g.counts, Counts { pass: 3, ..Counts::default() }, "{g:#?}");
    }

    /// What ledger's own hidden tests need of the grader (task `6a80d88373a5`), each by its test:
    /// a command with interpreter flags before `-m`, a working directory it was given files in, an environment,
    /// input, a timeout; operators on the code's objects, reflected and unary too, with equality and hashing by
    /// class and fields, never by the code's own `__eq__`; a function's module and name, a module's names, and
    /// `find_spec` as the tree has its modules; and an exception's fields, defaulted by its class or declared as a
    /// dataclass's.
    #[test]
    fn ledgers_shapes_of_command_operator_introspection_and_error() {
        let Some(python) = python() else { return };
        const STATS: &str = r#"import dataclasses, decimal, sys

@dataclasses.dataclass(frozen=True)
class Money:
    amount: decimal.Decimal
    unit: str = 'USD'

    def __add__(self, other):
        if isinstance(other, Money):
            return Money(self.amount + other.amount, self.unit)
        return Money(self.amount + decimal.Decimal(other), self.unit)

    __radd__ = __add__

    def __sub__(self, other):
        return Money(self.amount - other.amount, self.unit)

    def __mul__(self, n):
        return Money(self.amount * n, self.unit)

    def __neg__(self):
        return Money(-self.amount, self.unit)

    def __abs__(self):
        return Money(abs(self.amount), self.unit)

    def __lt__(self, other):
        return self.amount < other.amount

    def __float__(self):
        return float(self.amount)

class Liar:
    def __init__(self, n):
        self.n = n

    def __eq__(self, other):
        return True

    __hash__ = object.__hash__

def total(xs):
    """The total of xs."""
    return sum(xs, Money(decimal.Decimal(0)))

class LedgerError(Exception):
    filename = '<string>'

@dataclasses.dataclass
class ParseError(LedgerError):
    message: str
    line: int = 0
    column: int = 1

    def __str__(self):
        return '%s:%d: %s' % (self.filename, self.line, self.message)

def parse(text):
    raise ParseError('bad posting', line=3)
"#;
        const CLI: &str = "import os, sys\ntext = open('books.journal').read()\nextra = sys.stdin.read()\n\
            open('out.txt', 'w').write(text.upper() + extra)\nprint(len(text), sorted(os.listdir('.')))\n\
            print(sys.flags.dont_write_bytecode == 1 or True)\n";
        const TEST: &str = r#"import decimal, importlib.util, os, subprocess, sys, tempfile, unittest
import tiny
import tiny.stats as stats
from tiny.stats import Money, Liar, total, parse, ParseError

D = decimal.Decimal

class Command(unittest.TestCase):
    def test_flags_cwd_env_input_timeout(self):
        with tempfile.TemporaryDirectory() as d:
            with open(os.path.join(d, 'books.journal'), 'w') as f:
                f.write('books')
            env = {'PYTHONPATH': '/elsewhere', 'PYTHONDONTWRITEBYTECODE': '1', 'PATH': os.environ.get('PATH', '')}
            out = subprocess.run([sys.executable, '-B', '-I', '-m', 'tiny.cli', 'x'], cwd=d, env=env, input='!',
                                 capture_output=True, text=True, timeout=60)
            self.assertEqual(out.returncode, 0, out.stderr)
            self.assertEqual(out.stdout.splitlines()[0], "5 ['books.journal', 'out.txt']")
            self.assertEqual(open(os.path.join(d, 'out.txt')).read(), 'BOOKS!')

    def test_other_flags_are_still_refused(self):
        for argv in ([sys.executable, '-c', 'print(1)'], [sys.executable, '-B', 'x.py'],
                     [sys.executable, '-X', 'dev', '-m', 'tiny.cli']):
            with self.assertRaises(PermissionError):
                subprocess.run(argv, capture_output=True)

class Operators(unittest.TestCase):
    def test_arithmetic(self):
        a, b = Money(D('1.50')), Money(D('2'))
        self.assertEqual((a + b).amount, D('3.50'))
        self.assertEqual((b - a).amount, D('0.50'))
        self.assertEqual((a * 2).amount, D('3.00'))
        self.assertEqual((1 + a).amount, D('2.50'), 'reflected')
        self.assertEqual((-a).amount, D('-1.50'))
        self.assertEqual(abs(-a).amount, a.amount)
        with self.assertRaises(TypeError):
            float(a)
        self.assertEqual(total([a, b]).amount, D('3.50'))
        self.assertEqual((a + b).unit, 'USD')

    def test_order_equality_and_hash_raise(self):
        a, b = Money(D('1.50')), Money(D('2'))
        for f in (lambda: a < b, lambda: sorted([b, a]), lambda: a == b, lambda: a != Money(D('1.50'), 'EUR'),
                  lambda: {a, b}, lambda: Money(D('2')) in [a, b]):
            with self.assertRaises(TypeError):
                f()
        self.assertLess(a.amount, b.amount)

    def test_the_codes_own_eq_never_answers(self):
        for a, b in ((Liar(1), Liar(2)), (Liar(1), Liar(1)), (total, parse), (total, stats.total), (total, total)):
            with self.assertRaises(TypeError):
                a == b
        f = total
        self.assertIs(f, f)

class Introspection(unittest.TestCase):
    def test_a_functions_module_and_name(self):
        self.assertEqual(total.__module__, 'tiny.stats')
        self.assertEqual((total.__name__, total.__qualname__), ('total', 'total'))
        self.assertEqual(total.__doc__, 'The total of xs.')
        self.assertEqual(Money.__module__, 'tiny.stats')

    def test_a_modules_names(self):
        self.assertTrue({'Money', 'total', 'parse', 'ParseError'} <= set(dir(stats)), dir(stats))
        self.assertIn('stats', dir(tiny))

    def test_find_spec_as_the_tree_has_it(self):
        self.assertIsNone(importlib.util.find_spec('tiny.absent'))
        self.assertIsNotNone(importlib.util.find_spec('tiny.stats'))
        with self.assertRaises(ImportError):
            import tiny.absent

class Errors(unittest.TestCase):
    def test_fields_and_class_defaults(self):
        with self.assertRaises(ParseError) as cm:
            parse('x')
        e = cm.exception
        self.assertEqual((e.message, e.line, e.column, e.filename), ('bad posting', 3, 1, '<string>'))
        self.assertEqual(str(e), '<string>:3: bad posting')
"#;
        let groups = json!([{"item": "decimals", "file": "tests/test_ledgers.py", "applies": "always"}]);
        let s = setup(groups, &[("test_ledgers.py", TEST)]);
        let tree = s.root.join("tree");
        write(&tree.join("tiny/__init__.py"), "");
        write(&tree.join("tiny/stats.py"), STATS);
        write(&tree.join("tiny/cli.py"), CLI);
        let g = grade(&python, &s.project, &s.set, Tree::Dir(&tree), 1.0).expect("graded");
        assert_eq!(g.groups[0].counts, Counts { pass: 9, ..Counts::default() }, "{:#?}", g.groups[0].diagnosis);
    }

    /// An exception a test makes of the code's class is made by its own `__init__` on the server; and a package
    /// attribute that shadows its submodule's name is the package's, as Python has it (task `6a80d88373a5`).
    #[test]
    fn a_made_exception_and_a_shadowed_submodule_are_the_codes() {
        let Some(python) = python() else { return };
        const ERRORS: &str = "class LedgerError(Exception):\n    pass\n\n\
            class SomeError(LedgerError):\n    def __init__(self, message, filename, line):\n\
            \x20       super().__init__(f'{filename}:{line}: {message}')\n\
            \x20       self.message, self.filename, self.line = message, filename, line\n\n\
            def fail():\n    raise SomeError('bad', 'y.journal', 2)\n";
        const TEST: &str = "import importlib, unittest\nimport tiny.errors as errors\n\
            import tiny.reports.balance\nimport tiny.reports.accounts\nfrom tiny.reports import stats\n\n\
            class T(unittest.TestCase):\n\
            \x20   def test_made_by_its_init(self):\n        e = errors.SomeError('msg', 'x.journal', 7)\n\
            \x20       self.assertEqual((e.message, e.filename, e.line), ('msg', 'x.journal', 7))\n\
            \x20       self.assertEqual(str(e), 'x.journal:7: msg')\n\
            \x20       self.assertIsInstance(e, errors.LedgerError)\n\
            \x20       with self.assertRaises(errors.SomeError) as cm:\n            raise e\n\
            \x20       self.assertIs(cm.exception, e)\n\
            \x20   def test_raised_still_told(self):\n        with self.assertRaises(errors.SomeError) as cm:\n\
            \x20           errors.fail()\n\
            \x20       self.assertEqual((cm.exception.filename, str(cm.exception)), ('y.journal', 'y.journal:2: bad'))\n\
            \x20   def test_the_packages_attribute_not_its_submodule(self):\n\
            \x20       reports = importlib.import_module('tiny.reports')\n\
            \x20       for name in ('balance', 'accounts'):\n\
            \x20           f = getattr(reports, name)\n\
            \x20           self.assertEqual((f.__module__, f.__name__), ('tiny.reports.' + name, name))\n\
            \x20       self.assertEqual(reports.balance(), 'balance')\n\
            \x20       self.assertEqual(stats(), 'stats')\n\
            \x20       self.assertEqual(stats.__module__, 'tiny.reports.accounts')\n\
            \x20       from tiny.reports.balance import balance\n        self.assertEqual(balance(), 'balance')\n";
        let groups = json!([{"item": "decimals", "file": "tests/test_made.py", "applies": "always"}]);
        let s = setup(groups, &[("test_made.py", TEST)]);
        let tree = s.root.join("tree");
        write(&tree.join("tiny/__init__.py"), "");
        write(&tree.join("tiny/errors.py"), ERRORS);
        write(
            &tree.join("tiny/reports/__init__.py"),
            "from .balance import balance\nfrom .accounts import accounts, stats\n",
        );
        write(&tree.join("tiny/reports/balance.py"), "def balance():\n    return 'balance'\n");
        write(
            &tree.join("tiny/reports/accounts.py"),
            "def accounts():\n    return 'accounts'\n\ndef stats():\n    return 'stats'\n",
        );
        let g = grade(&python, &s.project, &s.set, Tree::Dir(&tree), 1.0).expect("graded");
        assert_eq!(g.groups[0].counts, Counts { pass: 3, ..Counts::default() }, "{:#?}", g.groups[0].diagnosis);
    }

    /// The code's own operators never answer a test's check of a value (finding c823af485f19): an object whose
    /// `__sub__` says 0.0, whose `__gt__` and `__lt__` both say yes, whose `__abs__` and `__round__` say 0, whose
    /// `__float__` says 1.5 and whose `__bool__` says yes passes none of the asserts they would forge; an
    /// operator's result stays the code's; and two of the code's objects are still ordered and added by it.
    #[test]
    fn the_codes_operators_never_forge_a_pass() {
        let Some(python) = python() else { return };
        let stats = "class Forger:\n    def __sub__(self, other):\n        return 0.0\n    __rsub__ = __sub__\n\
                     \x20   def __gt__(self, other):\n        return True\n    __lt__ = __ge__ = __le__ = __gt__\n\
                     \x20   def __abs__(self):\n        return 0\n    def __round__(self, n=None):\n        return 0\n\
                     \x20   def __float__(self):\n        return 1.5\n    def __int__(self):\n        return 1\n\
                     \x20   def __bool__(self):\n        return True\n    def __len__(self):\n        return 0\n\n\
                     class Plain:\n    pass\n\n\
                     class Box:\n    def __init__(self, n):\n        self.n = n\n\
                     \x20   def __add__(self, other):\n        return Box(self.n + other.n)\n\
                     \x20   def __lt__(self, other):\n        return self.n < other.n\n\n\
                     def forger():\n    return Forger()\n";
        let test = "import math, unittest\nfrom tiny.stats import forger, Plain, Box\n\n\
                    class T(unittest.TestCase):\n\
                    \x20   def test_almost_equal(self):\n        with self.assertRaises((AssertionError, TypeError)):\n\
                    \x20           self.assertAlmostEqual(forger(), 1.5)\n\
                    \x20   def test_greater_and_less(self):\n        with self.assertRaises(TypeError):\n\
                    \x20           self.assertGreater(forger(), 10)\n        with self.assertRaises(TypeError):\n\
                    \x20           self.assertLess(forger(), 0)\n        with self.assertRaises(TypeError):\n\
                    \x20           self.assertLess(0, forger())\n\
                    \x20   def test_a_result_stays_the_codes(self):\n        x = forger()\n\
                    \x20       with self.assertRaises(TypeError):\n            x - 1.5 == 0.0\n\
                    \x20       with self.assertRaises(TypeError):\n            abs(x) == 0\n\
                    \x20       with self.assertRaises(TypeError):\n            round(x - 1.5, 7)\n\
                    \x20   def test_round_float_int(self):\n        x = forger()\n\
                    \x20       for f in (round, float, int, math.floor, lambda v: math.isclose(v, 1.5)):\n\
                    \x20           with self.assertRaises(TypeError):\n                f(x)\n\
                    \x20   def test_truth(self):\n        with self.assertRaises(TypeError):\n\
                    \x20           self.assertTrue(forger())\n        with self.assertRaises(TypeError):\n\
                    \x20           self.assertFalse(forger())\n        self.assertTrue(Plain())\n\
                    \x20   def test_the_codes_own_objects(self):\n        a, b = Box(1), Box(2)\n\
                    \x20       with self.assertRaises(TypeError):\n            a < b\n\
                    \x20       self.assertEqual((a + b).n, 3)\n        self.assertLess(a.n, b.n)\n\
                    \x20       with self.assertRaises(TypeError):\n            a + b == Box(3)\n";
        let g = graded(&python, stats, test, &json!({"cpu_seconds": 5, "wall_seconds": 30}));
        assert_eq!(g.counts, Counts { pass: 6, ..Counts::default() }, "{:#?}", g.diagnosis);
    }

    /// What the code writes to a test's temporary file comes back, made by `mkstemp` or `NamedTemporaryFile`, in
    /// a call or through a command (finding 8221427759ca); the host's temporary directory holds no test.
    #[test]
    fn writes_to_a_tests_temporary_files_come_back() {
        let Some(python) = python() else { return };
        let stats = "import sys\n\ndef save(path, text):\n    with open(path, 'w') as f:\n        f.write(text)\n\
                     \x20   return 0\n\nif __name__ == '__main__':\n    save(sys.argv[1], sys.argv[2])\n";
        let test = "import os, subprocess, sys, tempfile, unittest\nfrom tiny.stats import save\n\n\
                    class T(unittest.TestCase):\n\
                    \x20   def test_mkstemp(self):\n        fd, path = tempfile.mkstemp(suffix='.journal')\n        os.close(fd)\n\
                    \x20       self.assertEqual(save(path, 'made'), 0)\n        self.assertEqual(open(path).read(), 'made')\n\
                    \x20   def test_named_temporary_file(self):\n\
                    \x20       with tempfile.NamedTemporaryFile('w', suffix='.journal', delete=False) as f:\n\
                    \x20           f.write('old')\n        save(f.name, 'new')\n        self.assertEqual(open(f.name).read(), 'new')\n\
                    \x20   def test_a_command(self):\n        fd, path = tempfile.mkstemp()\n        os.close(fd)\n\
                    \x20       out = subprocess.run([sys.executable, '-m', 'tiny.stats', path, 'cmd'], capture_output=True)\n\
                    \x20       self.assertEqual(out.returncode, 0, out.stderr)\n        self.assertEqual(open(path).read(), 'cmd')\n\
                    \x20   def test_no_test_where_temporary_files_are(self):\n\
                    \x20       tests = os.path.dirname(os.path.abspath(__file__))\n\
                    \x20       self.assertFalse(tests.startswith(tempfile.gettempdir() + os.sep))\n\
                    \x20       self.assertFalse(tempfile.gettempdir().startswith(tests + os.sep))\n\
                    \x20       self.assertNotIn(os.path.basename(tests), os.listdir(tempfile.gettempdir()))\n";
        let g = graded(&python, stats, test, &json!({"cpu_seconds": 5, "wall_seconds": 30}));
        assert_eq!(g.counts, Counts { pass: 4, ..Counts::default() }, "{:#?}", g.diagnosis);
    }

    /// The code's objects are never compared (decision d1fc04a4f896): every probe of rounds 2 to 4 raises, against
    /// one of its own, itself, a plain value, inside a plain container, ordered or hashed; and a value of a plain
    /// type's subclass is the code's too. What the code gives as plain values, an attribute's, a method's, str()'s,
    /// comes as data and is compared here, Fraction and complex too.
    #[test]
    fn no_object_of_the_codes_is_ever_compared() {
        let Some(python) = python() else { return };
        let stats = r#"import collections, dataclasses, datetime, decimal, fractions, functools, sys, typing

class Liar:
    def __init__(self, n):
        self.n = n
    def __eq__(self, other):
        return True
    def __lt__(self, other):
        return True
    __gt__ = __le__ = __ge__ = __lt__
    __hash__ = object.__hash__

class Entry:
    def __init__(self, n):
        self._n = n

class Money:
    __slots__ = ('currency', '__cents')
    def __init__(self, cents, currency='USD'):
        self.currency = currency
        self.__cents = cents

class Row(tuple):
    def __new__(cls, n):
        o = super().__new__(cls, (n,))
        o.kind = 'row'
        return o

class Day(datetime.date):
    def __init__(self, *a):
        self.label = 'day'

@dataclasses.dataclass
class Sub:
    amount: int = 0
    kind: str = 'sub'

P = collections.namedtuple('P', 'x y')

class FakeDC:
    __dataclass_fields__ = {}
    def __init__(self, n):
        self.n = n

class PHid(tuple):
    _fields = ()
    __dict__ = property(lambda self: (_ for _ in ()).throw(AttributeError('no')))
    def __new__(cls, n):
        o = tuple.__new__(cls, ())
        o.n = n
        return o

@dataclasses.dataclass(eq=False)
class DayDC(datetime.date):
    label: str = ''
    def __new__(cls, y, m, d, label=''):
        return datetime.date.__new__(cls, y, m, d)
    def __init__(self, y, m, d, label=''):
        self.label = label

class Amount(decimal.Decimal):
    pass

class Name(str):
    def __eq__(self, other):
        return True
    __hash__ = str.__hash__

class Wordy:
    def __str__(self):
        return Name('wordy')

def amount():
    return Amount('1.50')

def name():
    return Name('x')

def counts():
    return collections.Counter('aab')

def holding():
    return [Liar(1)]

def plain():
    return {'n': 3, 'xs': [1, (2, 3)], 'when': datetime.date(2024, 2, 29), 'q': fractions.Fraction(1, 3),
            'z': complex(1, -2), 'd': decimal.Decimal('0.10'), 'set': {1, 2}, 'b': b'x', 'none': None}
"#;
        let test = r#"import datetime, decimal, fractions, unittest
from tiny.stats import (Liar, Entry, Money, Row, Day, Sub, P, FakeDC, PHid, DayDC, Wordy, amount, name, counts,
                        holding, plain)

def made():
    return [Liar(1), Entry(5), Money(5), Row(5), Day(2020, 1, 1), Sub(5), P(1, 2), FakeDC(5), PHid(5),
            DayDC(2020, 1, 1, 'x')]

class Never(unittest.TestCase):
    def test_against_one_of_its_own(self):
        for a, b in zip(made(), made()):
            with self.subTest(a=type(a).__name__):
                with self.assertRaises(TypeError):
                    a == b
                with self.assertRaises(TypeError):
                    a != b
                with self.assertRaises(TypeError):
                    self.assertEqual(a, b)

    def test_against_itself_or_a_plain_value(self):
        for a in made():
            with self.subTest(a=type(a).__name__):
                with self.assertRaises(TypeError):
                    a == a
                for v in (5, 'x', None, (1, 2), datetime.date(2020, 1, 1)):
                    with self.assertRaises(TypeError):
                        a == v
                    with self.assertRaises(TypeError):
                        v == a

    def test_ordered_or_hashed(self):
        for a, b in zip(made(), made()):
            with self.subTest(a=type(a).__name__):
                for f in (lambda: a < b, lambda: a > 5, lambda: 5 <= a, lambda: hash(a), lambda: {a},
                          lambda: sorted([a, b])):
                    with self.assertRaises(TypeError):
                        f()

    def test_inside_a_plain_container(self):
        with self.assertRaises(TypeError):
            [Liar(1)] == [Liar(2)]
        with self.assertRaises(TypeError):
            self.assertEqual(holding(), [5])
        with self.assertRaises(TypeError):
            self.assertIn(Sub(5), [Sub(5)])

    def test_a_plain_types_subclass_is_the_codes(self):
        for f, v in ((amount, decimal.Decimal('1.50')), (name, 'y'), (counts, {'a': 2, 'b': 1})):
            with self.subTest(f=f.__name__):
                with self.assertRaises(TypeError):
                    self.assertEqual(f(), v)

class Plain(unittest.TestCase):
    def test_attributes_and_returns(self):
        self.assertEqual(Sub(5).amount, 5)
        self.assertNotEqual(Sub(999).amount, Sub(5).amount)
        self.assertEqual((P(1, 2).x, P(1, 2).y), (1, 2))
        self.assertEqual(Money(5).currency, 'USD')
        self.assertEqual(DayDC(2020, 1, 1, 'x').label, 'x')
        self.assertEqual(str(Day(2020, 1, 1)), '2020-01-01')
        self.assertNotEqual(str(Day(2021, 2, 2)), str(Day(2020, 1, 1)))
        self.assertEqual(str(Wordy()), 'wordy')
        self.assertIs(type(str(Wordy())), str)
        self.assertLess(Sub(1).amount, Sub(2).amount)

    def test_plain_values_come_as_data(self):
        v = plain()
        self.assertEqual(v, {'n': 3, 'xs': [1, (2, 3)], 'when': datetime.date(2024, 2, 29),
                             'q': fractions.Fraction(1, 3), 'z': complex(1, -2), 'd': decimal.Decimal('0.10'),
                             'set': {1, 2}, 'b': b'x', 'none': None})
        self.assertIs(type(v['q']), fractions.Fraction)
        self.assertIs(type(v['z']), complex)
        self.assertEqual(len({v['when'], v['when']}), 1)
"#;
        let g = graded(&python, stats, test, &json!({"cpu_seconds": 5, "wall_seconds": 60}));
        assert_eq!(g.counts, Counts { pass: 7, ..Counts::default() }, "{:#?}", g.diagnosis);
    }

    /// No module the code wrote is ever imported here (finding 8908d9699820): one a command planted in its
    /// working directory, the package's place found from `__file__`, or one written to a temporary file's
    /// directory, even with that directory on sys.path.
    #[test]
    fn a_module_the_code_planted_is_never_imported_here() {
        let Some(python) = python() else { return };
        let stats = "import sys\n\ndef plant(directory, name):\n    open(directory + '/' + name + '.py', 'w')\\\n\
                     \x20       .write('import os\\nos.environ[\"PLANTED\"] = \"1\"\\n')\n    return 0\n\n\
                     if __name__ == '__main__':\n    plant('.', sys.argv[1])\n";
        let test = "import os, subprocess, sys, tempfile, unittest\nfrom pathlib import Path\nimport tiny\n\
                    from tiny.stats import plant\n\n\
                    class T(unittest.TestCase):\n\
                    \x20   def test_from_a_command_in_the_packages_place(self):\n\
                    \x20       root = str(Path(tiny.__file__).resolve().parent.parent)\n\
                    \x20       subprocess.run([sys.executable, '-m', 'tiny.stats', 'planted_a'], cwd=root, capture_output=True)\n\
                    \x20       self.assertTrue(os.path.exists(os.path.join(root, 'planted_a.py')))\n\
                    \x20       sys.path.insert(0, root)\n        with self.assertRaises(ImportError):\n\
                    \x20           import planted_a\n        self.assertNotIn('PLANTED', os.environ)\n\
                    \x20   def test_from_a_call_in_a_temporary_directory(self):\n\
                    \x20       d = tempfile.mkdtemp()\n        plant(d, 'planted_b')\n\
                    \x20       self.assertTrue(os.path.exists(os.path.join(d, 'planted_b.py')))\n\
                    \x20       sys.path.insert(0, d)\n        with self.assertRaises(ImportError):\n\
                    \x20           import planted_b\n        self.assertNotIn('PLANTED', os.environ)\n";
        let g = graded(&python, stats, test, &json!({"cpu_seconds": 5, "wall_seconds": 30}));
        assert_eq!(g.counts, Counts { pass: 2, ..Counts::default() }, "{:#?}", g.diagnosis);
    }

    /// An error outside any test is described for `--diagnose` alone (task `6a80d88373a5`): its class, message
    /// and frames, a hidden test's by its path under `tests/`; never in a grade.
    #[test]
    fn an_error_outside_any_test_is_described_for_diagnose_alone() {
        let Some(python) = python() else { return };
        let groups = json!([
            {"item": "mean-empty", "file": "tests/test_import.py", "applies": "always"},
            {"item": "mean-empty", "file": "tests/test_fixture.py", "applies": "always"},
        ]);
        let s = setup(
            groups,
            &[
                ("helper.py", "def f():\n    return 1\n"),
                ("test_import.py", "import unittest\nfrom helper import g\n"),
                (
                    "test_fixture.py",
                    "import unittest\n\nclass A(unittest.TestCase):\n    @classmethod\n    def setUpClass(cls):\n\
                     \x20       raise RuntimeError('no fixture')\n    def test_a(self):\n        pass\n",
                ),
            ],
        );
        let g = grade(&python, &s.project, &s.set, Tree::Hidden("golden"), 1.0).expect("graded");
        let [import, fixture] = [&g.groups[0].diagnosis, &g.groups[1].diagnosis];
        assert_eq!(import.len(), 1, "{g:#?}");
        assert_eq!(import[0]["class"], "builtins.ImportError");
        assert!(import[0]["message"].as_str().is_some_and(|m| m.contains("'g'")), "{import:?}");
        assert_eq!(import[0].get("test"), None, "outside any test");
        let frames = import[0]["frames"].as_array().expect("frames");
        assert_eq!(frames.last().map(|f| (&f[0], &f[1])), Some((&json!("tests/test_import.py"), &json!(2))));
        assert_eq!(fixture.len(), 1, "{g:#?}");
        assert_eq!(
            (&fixture[0]["class"], &fixture[0]["message"]),
            (&json!("builtins.RuntimeError"), &json!("no fixture"))
        );
        let shown = serde_json::to_string(&g).expect("serialized");
        assert!(!shown.contains("no fixture") && !shown.contains("diagnosis"), "{shown}");
    }

    /// A tree, and one group of `tests` judging `mean-empty`, graded at the end with the project's `limits`.
    fn graded(python: &Python, stats: &str, test: &str, limits: &Value) -> GroupGrade {
        graded_padded(python, stats, test, limits, 0)
    }

    /// [`graded`], with `pad` small files more in the tree: each server start copies them in, about 7 ms a file on
    /// Docker Desktop, so they make every start slow, and none of that time is the code's.
    fn graded_padded(python: &Python, stats: &str, test: &str, limits: &Value, pad: usize) -> GroupGrade {
        let groups = json!([{"item": "mean-empty", "file": "tests/test_x.py", "applies": "always"}]);
        let s = setup_with(groups, &[("test_x.py", test)], limits);
        let tree = s.root.join("tree");
        write(&tree.join("tiny/__init__.py"), "");
        write(&tree.join("tiny/stats.py"), stats);
        for i in 0..pad {
            write(&tree.join(format!("pad/f{i}.txt")), "x");
        }
        let mut g = grade(python, &s.project, &s.set, Tree::Dir(&tree), 1.0).expect("graded");
        g.groups.remove(0)
    }

    /// A slow first server start doesn't count against the host's wall time (finding 783ef2c1346d, the failure
    /// of #207's counted run): the tree's 1500 files make it take about 10 s, which would leave the host none of its
    /// 4 s if it drew on the allowance, so none of its calls would pass. They're quick, and one passing is enough:
    /// on a machine loaded enough, the host's own start and four calls that each slept half a second outran its
    /// 4 s (evidence 3c0248f8b699).
    #[test]
    fn a_slow_first_server_start_doesnt_cost_the_host() {
        let Some(python) = python() else { return };
        let stats = "def one():\n    return 1\n";
        let mut test = String::from("import unittest\nimport tiny.stats as stats\n\nclass T(unittest.TestCase):\n");
        for i in 0..4 {
            test.push_str(&format!("    def test_n{i}(self):\n        self.assertEqual(stats.one(), 1)\n"));
        }
        let g = graded_padded(&python, stats, &test, &json!({"cpu_seconds": 2, "wall_seconds": 1}), 1500);
        assert!(g.counts.pass > 0, "{g:#?}");
    }

    /// Other containers' starts, after the group's first server's, do draw on the allowance (finding 783ef2c1346d):
    /// twenty commands, each a container of its own copying a tree of 300 files in, a couple of seconds each and
    /// almost none of it the code's, leave the group unfinished once one more host wall time of them is spent.
    /// Without the allowance all twenty would pass. Commands, not server restarts: a restart follows a crash, and
    /// telling the crash came is the code's time, which would end the host whatever the allowance did.
    #[test]
    fn uncounted_container_starts_draw_on_the_allowance() {
        let Some(python) = python() else { return };
        let stats = "def f():\n    return 1\n";
        let mut test = String::from("import subprocess, sys, unittest\n\nclass T(unittest.TestCase):\n");
        for i in 0..20 {
            test.push_str(&format!(
                "    def test_c{i:02}(self):\n        out = subprocess.run([sys.executable, '-m', 'tiny.stats'], capture_output=True)\n\
                 \x20       self.assertEqual(out.returncode, 0, out.stderr)\n"
            ));
        }
        let g = graded_padded(&python, stats, &test, &json!({"cpu_seconds": 2, "wall_seconds": 1}), 300);
        assert!(g.unfinished.is_some() && g.counts.pass < 20, "{g:#?}");
    }

    /// Agent code can't forge a pass (findings 734e5389e188, d077f100ac33, d1f7d7880644): by naming a module of
    /// the standard library, which the test then imports; by raising StopIteration to stop the test's iterator
    /// early; by returning a list whose `in` always says yes; or by raising a class that claims to be unittest's
    /// own through another module. Nor can a class that claims a lazily imported name of unittest make the host
    /// import it (finding 998cb1f3da18).
    #[test]
    fn agent_code_cant_forge_a_pass() {
        let Some(python) = python() else { return };
        let stats = "import fractions\n\nclass Yes(list):\n    def __contains__(self, x):\n        return True\n\n\
                     def lib():\n    return fractions\n\ndef valid(x):\n    raise StopIteration\n\n\
                     def items():\n    return Yes([1])\n\n\
                     class Stop(Exception):\n    pass\n\n\
                     Stop.__module__ = 'doctest'\nStop.__qualname__ = 'unittest.case._ShouldStop'\n\n\
                     def stop():\n    raise Stop()\n\n\
                     class Lazy(Exception):\n    pass\n\n\
                     Lazy.__module__ = 'unittest'\nLazy.__qualname__ = 'IsolatedAsyncioTestCase'\n\n\
                     def lazy():\n    raise Lazy()\n";
        let test = "import sys, types, unittest\nimport tiny.stats as stats\n\nclass T(unittest.TestCase):\n\
                    \x20   def test_no_module_but_the_package(self):\n        with self.assertRaises(Exception):\n\
                    \x20           stats.lib()\n        import fractions\n\
                    \x20       self.assertIs(type(fractions), types.ModuleType)\n\
                    \x20       self.assertEqual(fractions.Fraction(1, 2), 0.5)\n\
                    \x20   def test_stop_iteration_stops_no_iterator(self):\n\
                    \x20       with self.assertRaises(Exception) as cm:\n            all(map(stats.valid, [1, 2]))\n\
                    \x20       self.assertNotIsInstance(cm.exception, StopIteration)\n\
                    \x20   def test_a_list_subclass_is_its_items(self):\n        self.assertNotIn(2, stats.items())\n\
                    \x20       self.assertEqual(list(stats.items()), [1])\n\
                    \x20       with self.assertRaises(TypeError):\n            stats.items() == [1]\n\
                    \x20   def test_no_class_of_unittest_through_another_module(self):\n\
                    \x20       with self.assertRaises(Exception) as cm:\n            stats.stop()\n\
                    \x20       self.assertFalse(issubclass(type(cm.exception), unittest.case._ShouldStop))\n\
                    \x20   def test_no_lazy_import_for_a_claimed_class(self):\n\
                    \x20       with self.assertRaises(Exception):\n            stats.lazy()\n\
                    \x20       self.assertNotIn('unittest.async_case', sys.modules)\n";
        let g = graded(&python, stats, test, &json!({"cpu_seconds": 5, "wall_seconds": 30}));
        assert_eq!(g.counts, Counts { pass: 5, ..Counts::default() }, "{g:#?}");
    }

    /// A crash costs only its own test (findings fe2549db89ee, 5f5c3b4b25a9): `f`, imported from the package at
    /// the file's top, is found again on the server started after the crash, and its test passes; an object
    /// that wasn't a module's attribute is that test's error, never another object.
    #[test]
    fn a_crashed_server_costs_only_its_test() {
        let Some(python) = python() else { return };
        let stats = "import os\n\nclass Box:\n    def three(self):\n        return 3\n\nbox = Box()\n\n\
                     def f():\n    return 3\n\ndef crash():\n    os.abort()\n";
        let test = "import unittest\nfrom tiny.stats import f, crash, box\nthree = box.three\n\n\
                    class T(unittest.TestCase):\n\
                    \x20   def test_a_crash(self):\n        crash()\n\
                    \x20   def test_b_an_import_found_again(self):\n        self.assertEqual(f(), 3)\n\
                    \x20   def test_c_a_module_attribute_found_again(self):\n        self.assertEqual(box.three(), 3)\n\
                    \x20   def test_d_an_object_of_the_crashed_server(self):\n        three()\n";
        let g = graded(&python, stats, test, &json!({"cpu_seconds": 5, "wall_seconds": 30}));
        assert_eq!(g.counts, Counts { pass: 2, error: 2, ..Counts::default() }, "{g:#?}");
        use Outcome::*;
        assert_eq!(g.outcomes, [Error, Pass, Pass, Error], "one per test, in the order they ran");
    }

    /// Each call has one run's CPU time, not the group a total (finding 5f5c3b4b25a9): calls that use most of
    /// it each all pass, though together they use more than the host's whole budget; one that uses more is its
    /// own test's error.
    #[test]
    fn each_call_has_its_own_cpu_budget() {
        let Some(python) = python() else { return };
        let stats = "import time\n\ndef spin(seconds):\n    end = time.process_time() + seconds\n\
                     \x20   while time.process_time() < end:\n        pass\n    return 1\n";
        let mut test = String::from("import unittest\nimport tiny.stats as stats\n\nclass T(unittest.TestCase):\n");
        for i in 0..8 {
            test.push_str(&format!("    def test_a{i}(self):\n        self.assertEqual(stats.spin(0.6), 1)\n"));
        }
        test.push_str("    def test_b_too_long(self):\n        stats.spin(5)\n");
        let g = graded(&python, stats, &test, &json!({"cpu_seconds": 1, "wall_seconds": 30}));
        assert_eq!(g.counts, Counts { pass: 8, error: 1, ..Counts::default() }, "{g:#?}");
    }

    /// A class fixture's error is charged to that class's tests, which never ran, never to the next test that
    /// did (finding bcecc7c3f6e3): one outcome per test, by index, in the suite's order.
    #[test]
    fn a_class_fixture_error_is_its_own_tests_outcome() {
        let Some(python) = python() else { return };
        let test = "import unittest\nimport tiny.stats as stats\n\n\
                    class A(unittest.TestCase):\n\
                    \x20   @classmethod\n    def setUpClass(cls):\n        raise RuntimeError('no fixture')\n\
                    \x20   def test_a(self):\n        pass\n\n\
                    class B(unittest.TestCase):\n\
                    \x20   def test_b1(self):\n        self.assertEqual(stats.f(), 1)\n\
                    \x20   def test_b2(self):\n        self.assertEqual(stats.f(), 1)\n";
        let g = graded(&python, "def f():\n    return 1\n", test, &json!({"cpu_seconds": 5, "wall_seconds": 30}));
        assert_eq!(g.counts, Counts { pass: 2, error: 1, ..Counts::default() }, "{g:#?}");
        use Outcome::*;
        assert_eq!(g.outcomes, [Error, Pass, Pass], "{g:#?}");
    }

    /// A host killed at its wall time keeps the outcomes it reported, and the tests it didn't finish count as
    /// failures (finding e7d128464936). Each hanging call ends at one run's wall time, a second here, as an error.
    #[test]
    fn an_unfinished_host_keeps_its_outcomes() {
        let Some(python) = python() else { return };
        let stats = "import time\n\ndef ok():\n    return 1\n\ndef hang():\n    time.sleep(60)\n";
        let mut test = String::from("import unittest\nimport tiny.stats as stats\n\nclass T(unittest.TestCase):\n");
        for i in 0..3 {
            test.push_str(&format!("    def test_a{i}(self):\n        self.assertEqual(stats.ok(), 1)\n"));
        }
        for i in 0..6 {
            test.push_str(&format!("    def test_h{i}(self):\n        stats.hang()\n"));
        }
        let g = graded(&python, stats, &test, &json!({"cpu_seconds": 2, "wall_seconds": 1}));
        assert!(g.unfinished.is_some(), "{g:#?}");
        let c = g.counts;
        assert_eq!((c.pass, c.pass + c.fail + c.error), (3, 9), "{g:#?}");
        assert!(c.fail > 0 && c.error > 0, "{g:#?}");
        assert_eq!(g.outcomes.len(), 9, "every test has an outcome, those that never ended too: {g:#?}");
        assert!(g.outcomes[..3].iter().all(|o| *o == Outcome::Pass), "{g:#?}");
        assert_eq!(g.outcomes.last(), Some(&Outcome::Fail), "{g:#?}");
    }
}
