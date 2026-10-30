import unittest
from decimal import Decimal

from ledger import ParseError, parse

JOURNAL = """\
; Opening the books
2024-01-01 Opening balance
    Assets:Checking     1000.00
    Equity:Opening     -1000.00

2024-01-05 Grocery store
    Expenses:Food         42.10
    Assets:Checking      -42.10
# a hash comment
2024-01-07
    Expenses:Rent        700
    Assets:Checking     -700
"""


class Parse(unittest.TestCase):
    def test_transactions_in_file_order(self):
        journal = parse(JOURNAL)
        self.assertEqual([t.description for t in journal.transactions], ["Opening balance", "Grocery store", ""])
        self.assertEqual([t.date.isoformat() for t in journal.transactions], ["2024-01-01", "2024-01-05", "2024-01-07"])

    def test_postings_and_their_lines(self):
        journal = parse(JOURNAL)
        grocery = journal.transactions[1]
        self.assertEqual(grocery.line, 6)
        self.assertEqual([p.account for p in grocery.postings], ["Expenses:Food", "Assets:Checking"])
        self.assertEqual([p.amount for p in grocery.postings], [Decimal("42.10"), Decimal("-42.10")])
        self.assertEqual([p.line for p in grocery.postings], [7, 8])

    def test_every_transaction_balances(self):
        self.assertTrue(all(t.is_balanced() for t in parse(JOURNAL).transactions))

    def test_empty_text_is_an_empty_journal(self):
        self.assertEqual(len(parse("")), 0)
        self.assertEqual(len(parse("; only a comment\n\n")), 0)


class Errors(unittest.TestCase):
    def assert_error_at(self, text, line):
        with self.assertRaises(ParseError) as caught:
            parse(text, filename="books.journal")
        self.assertEqual(caught.exception.line, line)
        self.assertEqual(caught.exception.filename, "books.journal")
        self.assertTrue(str(caught.exception).startswith(f"books.journal:{line}: "))

    def test_posting_outside_a_transaction(self):
        self.assert_error_at("    Assets:Cash  5.00\n", 1)

    def test_bad_amount(self):
        self.assert_error_at("2024-01-01 x\n    Assets:Cash  five\n    Income  -5\n", 2)

    def test_missing_amount(self):
        self.assert_error_at("2024-01-01 x\n    Assets:Cash  5\n    Income\n", 3)

    def test_bad_account_name(self):
        self.assert_error_at("2024-01-01 x\n    Assets::Cash  5\n    Income  -5\n", 2)


if __name__ == "__main__":
    unittest.main()
