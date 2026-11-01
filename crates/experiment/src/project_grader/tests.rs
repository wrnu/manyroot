//! These need Docker, as `python::tests` do: where it isn't running, each says so and passes without checking
//! anything.

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
            \x20           with self.assertRaises(PermissionError):\n\
            \x20               subprocess.run([sys.executable, '-m', 'tiny.ls'], cwd=d, capture_output=True)\n";
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
        assert_eq!(g.groups[0].counts, Counts { pass: 7, ..Counts::default() }, "{g:#?}");
    }

    /// A tree, and one group of `tests` judging `mean-empty`, graded at the end with the project's `limits`.
    fn graded(python: &Python, stats: &str, test: &str, limits: &Value) -> GroupGrade {
        let groups = json!([{"item": "mean-empty", "file": "tests/test_x.py", "applies": "always"}]);
        let s = setup_with(groups, &[("test_x.py", test)], limits);
        let tree = s.root.join("tree");
        write(&tree.join("tiny/__init__.py"), "");
        write(&tree.join("tiny/stats.py"), stats);
        let mut g = grade(python, &s.project, &s.set, Tree::Dir(&tree), 1.0).expect("graded");
        g.groups.remove(0)
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
                    \x20       self.assertEqual(stats.items(), [1])\n\
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
