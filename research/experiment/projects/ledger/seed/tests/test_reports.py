import datetime
import unittest
from decimal import Decimal

from ledger import accounts, balance, format_journal, parse, stats

JOURNAL = """\
2024-01-01 Opening balance
    Assets:Checking     1000.00
    Equity:Opening     -1000.00

2024-01-05 Grocery store
    Expenses:Food:Groceries   42.10
    Assets:Checking          -42.10

2024-02-01 Rent
    Expenses:Rent        700
    Assets:Checking     -700

2024-02-03 Bakery
    Expenses:Food:Bakery   6.40
    Assets:Cash           -6.40
"""


def rows(report):
    return {row["account"]: row["amount"] for row in report["accounts"]}


class Balance(unittest.TestCase):
    def test_every_account_and_its_ancestors(self):
        report = balance(parse(JOURNAL))
        self.assertEqual(
            rows(report),
            {
                "Assets": Decimal("251.50"),
                "Assets:Cash": Decimal("-6.40"),
                "Assets:Checking": Decimal("257.90"),
                "Equity": Decimal("-1000.00"),
                "Equity:Opening": Decimal("-1000.00"),
                "Expenses": Decimal("748.50"),
                "Expenses:Food": Decimal("48.50"),
                "Expenses:Food:Bakery": Decimal("6.40"),
                "Expenses:Food:Groceries": Decimal("42.10"),
                "Expenses:Rent": Decimal("700"),
            },
        )
        self.assertEqual([row["account"] for row in report["accounts"]], sorted(rows(report)))
        self.assertEqual(report["total"], 0)

    def test_a_date_range_includes_begin_and_leaves_out_end(self):
        report = balance(parse(JOURNAL), begin=datetime.date(2024, 1, 5), end=datetime.date(2024, 2, 3))
        self.assertEqual(rows(report)["Assets:Checking"], Decimal("-742.10"))
        self.assertNotIn("Assets:Cash", rows(report))
        self.assertNotIn("Equity", rows(report))

    def test_depth(self):
        report = balance(parse(JOURNAL), depth=1)
        self.assertEqual(sorted(rows(report)), ["Assets", "Equity", "Expenses"])

    def test_a_filter(self):
        report = balance(parse(JOURNAL), filters=["Expenses:Food"])
        self.assertEqual(rows(report)["Expenses"], Decimal("48.50"))
        self.assertEqual(report["total"], Decimal("48.50"))


class Others(unittest.TestCase):
    def test_accounts(self):
        listed = accounts(parse(JOURNAL))
        self.assertEqual(listed[0]["account"], "Assets:Cash")
        checking = [row for row in listed if row["account"] == "Assets:Checking"][0]
        self.assertEqual(checking["postings"], 3)
        self.assertEqual(checking["first"], datetime.date(2024, 1, 1))
        self.assertEqual(checking["last"], datetime.date(2024, 2, 1))

    def test_stats(self):
        self.assertEqual(
            stats(parse(JOURNAL)),
            {
                "transactions": 4,
                "postings": 8,
                "accounts": 6,
                "first": datetime.date(2024, 1, 1),
                "last": datetime.date(2024, 2, 3),
                "unbalanced": 0,
            },
        )

    def test_print_parses_back_to_the_same_journal(self):
        journal = parse(JOURNAL)
        again = parse(format_journal(journal))
        self.assertEqual(
            [(t.date, t.description, [(p.account, p.amount) for p in t.postings]) for t in again.transactions],
            [(t.date, t.description, [(p.account, p.amount) for p in t.postings]) for t in journal.transactions],
        )


if __name__ == "__main__":
    unittest.main()
