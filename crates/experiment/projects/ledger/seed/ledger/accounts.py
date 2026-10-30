"""Account names and the account tree.

An account name is segments joined by ":", such as "Expenses:Food:Groceries". Its parent is the name
without its last segment, so "Expenses:Food" is the parent of "Expenses:Food:Groceries", and
"Expenses" is a top-level account.
"""

SEPARATOR = ":"


def segments(account):
    """The segments of `account`'s name."""
    return account.split(SEPARATOR)


def depth(account):
    """How many segments `account` has: 1 for a top-level account."""
    return len(segments(account))


def parent(account):
    """`account`'s parent, or None for a top-level account."""
    parts = segments(account)
    if len(parts) == 1:
        return None
    return SEPARATOR.join(parts[:-1])


def ancestors(account):
    """`account` and every account above it, nearest first."""
    result = []
    while account is not None:
        result.append(account)
        account = parent(account)
    return result


def is_within(account, other):
    """Whether `account` is `other` or one of its descendants."""
    return account == other or account.startswith(other + SEPARATOR)


def check_name(account):
    """Raises ValueError unless `account` is a valid name: non-empty segments with no surrounding spaces."""
    if not account:
        raise ValueError("empty account name")
    for part in segments(account):
        if not part or part != part.strip():
            raise ValueError(f"invalid account name: {account}")


def roll_up(amounts):
    """Account totals with every ancestor added, from a dict of each account's own total.

    Returns a dict from each account, and each of its ancestors, to the total of it and its
    descendants.
    """
    totals = {}
    for account, amount in amounts.items():
        for name in ancestors(account):
            totals[name] = totals.get(name, 0) + amount
    return totals
