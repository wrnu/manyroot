"""Runs a tree's public tests with the standard library's unittest, inside the Python profile.

Arguments: the tree, the tests' directory relative to it, and a marker. The tests' own output goes to
standard error; standard output gets only the marker and one JSON line of results, written last.
Agent code can write to standard output too, so a result is what the tests reported unless the agent's
code forged it: run_tests is feedback for the agent, never a grade.
"""

import json
import os
import sys
import unittest

tree, tests, marker = sys.argv[1], sys.argv[2], sys.argv[3]
out = os.fdopen(os.dup(1), "w")
sys.stdout = sys.stderr
sys.path.insert(0, tree)
os.chdir(tree)

outcomes = []


class Result(unittest.TextTestResult):
    def addSuccess(self, test):
        super().addSuccess(test)
        outcomes.append((test.id(), "pass"))

    def addFailure(self, test, err):
        super().addFailure(test, err)
        outcomes.append((test.id(), "fail"))

    def addError(self, test, err):
        super().addError(test, err)
        outcomes.append((test.id(), "error"))

    def addSkip(self, test, reason):
        super().addSkip(test, reason)
        outcomes.append((test.id(), "skip"))

    def addExpectedFailure(self, test, err):
        super().addExpectedFailure(test, err)
        outcomes.append((test.id(), "pass"))

    def addUnexpectedSuccess(self, test):
        super().addUnexpectedSuccess(test)
        outcomes.append((test.id(), "fail"))

    def addSubTest(self, test, subtest, err):
        super().addSubTest(test, subtest, err)
        if err is not None:
            failed = issubclass(err[0], test.failureException)
            outcomes.append((subtest.id(), "fail" if failed else "error"))


start = os.path.join(tree, tests)
suite = unittest.TestLoader().discover(start_dir=start, top_level_dir=start, pattern="test_*.py")
unittest.TextTestRunner(stream=sys.stderr, resultclass=Result, verbosity=2).run(suite)
out.write(marker + json.dumps({"tests": outcomes}) + "\n")
out.flush()
