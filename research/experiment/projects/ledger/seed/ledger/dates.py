"""Dates in journal text and on the command line: always YYYY-MM-DD."""

import datetime
import re

_DATE = re.compile(r"(\d{4})-(\d{2})-(\d{2})")

_DAYS_IN_MONTH = (31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31)


def days_in_month(year, month):
    """How many days `month` (1 to 12) of `year` has."""
    return _DAYS_IN_MONTH[month - 1]


def parse_date(text):
    """The date written as `text` in YYYY-MM-DD form.

    Raises ValueError if it isn't one, or names a day that doesn't exist.
    """
    match = _DATE.fullmatch(text.strip())
    if not match:
        raise ValueError(f"invalid date: {text} (expected YYYY-MM-DD)")
    year, month, day = (int(part) for part in match.groups())
    if not 1 <= month <= 12:
        raise ValueError(f"invalid date: {text} (no month {month})")
    if not 1 <= day <= days_in_month(year, month):
        raise ValueError(f"invalid date: {text} (no day {day} in that month)")
    return datetime.date(year, month, day)


def format_date(date):
    """`date` as journal text writes it."""
    return date.isoformat()


def in_range(date, begin=None, end=None):
    """Whether `date` is on or after `begin` and before `end`; either may be None, for no limit."""
    if begin is not None and date < begin:
        return False
    if end is not None and date >= end:
        return False
    return True
