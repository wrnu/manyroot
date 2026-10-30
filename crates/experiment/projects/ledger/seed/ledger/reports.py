"""Reports on a journal: the balance report, and the journal printed back as text.

Each report has two halves: a function returning plain data (dicts, lists, str, Decimal, date), which
the library's callers use, and a render_* function laying that data out as text for the CLI.
"""

from decimal import Decimal

from .accounts import depth as account_depth
from .accounts import roll_up
from .amount import format_amount
from .dates import format_date
from .text import LEFT, RIGHT, rule, table

# The width of a report's amount column.
AMOUNT_WIDTH = 16


def matches(filters, account):
    """Whether `account` is selected by `filters`, a sequence of strings: any of them occurring in
    its name, or no filters at all."""
    if not filters:
        return True
    return any(f in account for f in filters)


def balance(journal, filters=(), begin=None, end=None, depth=None):
    """Each account's balance, with its descendants', from `begin` up to but not including `end`.

    Returns {"accounts": [{"account": name, "amount": Decimal}, ...], "total": Decimal}. The rows are
    the accounts that `filters` select which have postings in the range, and all their ancestors,
    sorted by name; with `depth`, only accounts of at most that many segments. The total is the sum
    of the selected postings.
    """
    own = {}
    for _, posting in journal.postings(begin, end):
        if matches(filters, posting.account):
            own[posting.account] = own.get(posting.account, Decimal("0")) + posting.amount
    totals = roll_up(own)
    rows = []
    for account in sorted(totals):
        if depth is not None and account_depth(account) > depth:
            continue
        rows.append({"account": account, "amount": totals[account]})
    return {"accounts": rows, "total": sum(own.values(), Decimal("0"))}


def amount_column(amount):
    """`amount` right-aligned in the amount column."""
    return format_amount(amount).rjust(AMOUNT_WIDTH)


def render_balance(report):
    """The balance report as text: one line per account, a rule, and the total."""
    lines = [f"{amount_column(row['amount'])}  {row['account']}" for row in report["accounts"]]
    lines.append(rule(AMOUNT_WIDTH))
    total = report["total"]
    lines.append("0".rjust(AMOUNT_WIDTH) if total == 0 else amount_column(total))
    return "\n".join(lines) + "\n"


def accounts(journal, filters=()):
    """Every account `filters` select, with how many postings name it and the dates of the first and
    last of them: [{"account": name, "postings": int, "first": date, "last": date}, ...], sorted by
    name. Only accounts postings name are listed, not their ancestors."""
    found = {}
    for transaction, posting in journal.postings():
        if not matches(filters, posting.account):
            continue
        row = found.setdefault(
            posting.account,
            {"account": posting.account, "postings": 0, "first": transaction.date, "last": transaction.date},
        )
        row["postings"] += 1
        row["first"] = min(row["first"], transaction.date)
        row["last"] = max(row["last"], transaction.date)
    return [found[name] for name in sorted(found)]


def render_accounts(rows):
    """The accounts report as text: one line per account, with its postings and their date range."""
    cells = [
        [row["account"], str(row["postings"]), format_date(row["first"]), format_date(row["last"])] for row in rows
    ]
    return table(cells, [LEFT, RIGHT, LEFT, LEFT])


def stats(journal):
    """Counts about the journal: {"transactions", "postings", "accounts": int, "first", "last": date or
    None, "unbalanced": int}, the last being how many transactions don't sum to zero."""
    first, last = journal.date_range()
    return {
        "transactions": len(journal),
        "postings": sum(len(t.postings) for t in journal),
        "accounts": len(journal.accounts()),
        "first": first,
        "last": last,
        "unbalanced": sum(1 for t in journal if not t.is_balanced()),
    }


def render_stats(report):
    """The stats as text, one "name: value" line each."""
    shown = []
    for name in ("transactions", "postings", "accounts", "first", "last", "unbalanced"):
        value = report[name]
        if value is None:
            value = "-"
        elif hasattr(value, "isoformat"):
            value = format_date(value)
        shown.append(f"{name}: {value}\n")
    return "".join(shown)


def format_transaction(transaction):
    """One transaction as journal text, without a trailing newline."""
    header = format_date(transaction.date)
    if transaction.description:
        header += " " + transaction.description
    lines = [header]
    width = max((len(p.account) for p in transaction.postings), default=0)
    for posting in transaction.postings:
        lines.append(f"    {posting.account.ljust(width)}  {format_amount(posting.amount)}")
    return "\n".join(lines)


def format_journal(journal):
    """The whole journal as text, in date order, a blank line between transactions."""
    return "\n\n".join(format_transaction(t) for t in journal.sorted()) + ("\n" if len(journal) else "")
