import unittest
from decimal import Decimal

from ledger import format_amount, parse_amount, parse_date


class ParseAmount(unittest.TestCase):
    def test_whole_and_fractional_amounts(self):
        self.assertEqual(parse_amount("42.10"), Decimal("42.10"))
        self.assertEqual(parse_amount("-3"), Decimal("-3"))
        self.assertEqual(parse_amount(" 7.5 "), Decimal("7.5"))

    def test_text_that_isnt_an_amount_is_refused(self):
        for text in ["", "abc", "1.2.3", "--4", "4-", "1,000"]:
            with self.subTest(text=text), self.assertRaises(ValueError):
                parse_amount(text)


class FormatAmount(unittest.TestCase):
    def test_at_least_two_places(self):
        self.assertEqual(format_amount(Decimal("3")), "3.00")
        self.assertEqual(format_amount(Decimal("7.5")), "7.50")
        self.assertEqual(format_amount(Decimal("42.10")), "42.10")

    def test_negative_amounts(self):
        self.assertEqual(format_amount(Decimal("-42.1")), "-42.10")

    def test_zero_has_no_sign(self):
        self.assertEqual(format_amount(Decimal("-0.00")), "0.00")


class ParseDate(unittest.TestCase):
    def test_iso_dates(self):
        self.assertEqual(parse_date("2024-01-05").isoformat(), "2024-01-05")
        self.assertEqual(parse_date("2023-12-31").isoformat(), "2023-12-31")

    def test_days_that_dont_exist_are_refused(self):
        for text in ["2024-13-01", "2024-04-31", "2023-02-29", "2024-1-5", "05/01/2024"]:
            with self.subTest(text=text), self.assertRaises(ValueError):
                parse_date(text)


if __name__ == "__main__":
    unittest.main()
