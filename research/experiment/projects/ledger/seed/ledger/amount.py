"""Amounts of money: parsing them from journal text and formatting them for reports.

An amount is a decimal.Decimal. Arithmetic on amounts is plain Decimal arithmetic, so sums are exact.
"""

import re
from decimal import Decimal, InvalidOperation

# An optional minus sign, digits, and an optional fraction.
_NUMBER = re.compile(r"-?\d+(\.\d+)?")

# The fewest decimal places a formatted amount shows.
MIN_PLACES = 2


def parse_amount(text):
    """The amount written as `text`, such as "42.10" or "-3".

    Raises ValueError for anything else.
    """
    text = text.strip()
    if not _NUMBER.fullmatch(text):
        raise ValueError(f"invalid amount: {text}")
    try:
        return Decimal(str(round(float(text), MIN_PLACES)))
    except (InvalidOperation, OverflowError) as e:
        raise ValueError(f"invalid amount: {text}") from e


def places(amount):
    """How many decimal places `amount` shows when formatted: at least MIN_PLACES."""
    exponent = amount.as_tuple().exponent
    if not isinstance(exponent, int):
        raise ValueError(f"not a finite amount: {amount}")
    return max(MIN_PLACES, -exponent)


def format_amount(amount):
    """`amount` as reports show it: at least two decimal places, a leading minus when negative."""
    shown = places(amount)
    quantized = amount.quantize(Decimal(1).scaleb(-shown))
    if quantized == 0:
        quantized = abs(quantized)
    return f"{quantized:f}"


def total(amounts):
    """The sum of `amounts`, Decimal("0") when there are none."""
    result = Decimal("0")
    for amount in amounts:
        result += amount
    return result
