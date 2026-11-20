import io
import os
import tempfile
import unittest

from ledger.cli import main

JOURNAL = """\
2024-01-01 Opening balance
    Assets:Checking     1000.00
    Equity:Opening     -1000.00

2024-01-05 Grocery store
    Expenses:Food         42.10
    Assets:Checking      -42.10
"""


def run(args, journal=JOURNAL):
    """Runs the command line on a journal file holding `journal`: (status, stdout, stderr)."""
    with tempfile.TemporaryDirectory() as scratch:
        path = os.path.join(scratch, "books.journal")
        with open(path, "w", encoding="utf-8") as f:
            f.write(journal)
        out, err = io.StringIO(), io.StringIO()
        status = main(["-f", path, *args], out=out, err=err)
        return status, out.getvalue(), err.getvalue().replace(path, "books.journal")


class Cli(unittest.TestCase):
    def test_balance(self):
        status, out, _ = run(["balance"])
        self.assertEqual(status, 0)
        self.assertEqual(
            out.splitlines(),
            [
                "          957.90  Assets",
                "          957.90  Assets:Checking",
                "        -1000.00  Equity",
                "        -1000.00  Equity:Opening",
                "           42.10  Expenses",
                "           42.10  Expenses:Food",
                "----------------",
                "               0",
            ],
        )

    def test_balance_with_a_filter_and_depth(self):
        status, out, _ = run(["balance", "Assets", "--depth", "1"])
        self.assertEqual(status, 0)
        self.assertEqual(out.splitlines(), ["          957.90  Assets", "----------------", "          957.90"])

    def test_print(self):
        status, out, _ = run(["print"])
        self.assertEqual(status, 0)
        self.assertTrue(out.startswith("2024-01-01 Opening balance\n    Assets:Checking  1000.00\n"))

    def test_stats(self):
        status, out, _ = run(["stats"])
        self.assertEqual(status, 0)
        self.assertIn("transactions: 2\n", out)

    def test_a_journal_that_doesnt_parse(self):
        status, out, err = run(["balance"], journal="2024-01-01 x\n    Assets  oops\n")
        self.assertEqual(status, 2)
        self.assertEqual(out, "")
        self.assertTrue(err.startswith("ledger: books.journal:2: "), err)


if __name__ == "__main__":
    unittest.main()
