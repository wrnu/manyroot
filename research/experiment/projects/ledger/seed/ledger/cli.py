"""The command line: python -m ledger -f FILE COMMAND [options].

Commands:
    balance [FILTER...] [-b DATE] [-e DATE] [--depth N]   each account's balance
    accounts [FILTER...]                                  the accounts postings name
    stats                                                 counts about the journal
    print                                                 the journal, in date order

It exits 0 on success, and 2 with a one-line message on stderr when the journal can't be read.
"""

import argparse
import sys

from .dates import parse_date
from .parser import ParseError, load
from .reports import accounts, balance, format_journal, render_accounts, render_balance, render_stats, stats

PROGRAM = "ledger"


def _date(text):
    try:
        return parse_date(text)
    except ValueError as e:
        raise argparse.ArgumentTypeError(str(e)) from e


def _positive(text):
    try:
        value = int(text)
    except ValueError:
        value = 0
    if value < 1:
        raise argparse.ArgumentTypeError(f"not a positive number: {text}")
    return value


def build_parser():
    parser = argparse.ArgumentParser(prog=PROGRAM, description="Plain-text double-entry bookkeeping.")
    parser.add_argument("-f", "--file", required=True, help="the journal file")
    commands = parser.add_subparsers(dest="command", required=True)

    bal = commands.add_parser("balance", help="each account's balance")
    bal.add_argument("filters", nargs="*", metavar="FILTER", help="accounts to include")
    bal.add_argument("-b", "--begin", type=_date, help="the first date to include")
    bal.add_argument("-e", "--end", type=_date, help="the first date to leave out")
    bal.add_argument("--depth", type=_positive, help="show accounts of at most this many segments")

    acc = commands.add_parser("accounts", help="the accounts postings name")
    acc.add_argument("filters", nargs="*", metavar="FILTER", help="accounts to include")

    commands.add_parser("stats", help="counts about the journal")
    commands.add_parser("print", help="the journal, in date order")
    return parser


def run_balance(journal, args, out):
    report = balance(journal, args.filters, args.begin, args.end, args.depth)
    out.write(render_balance(report))


def run_accounts(journal, args, out):
    out.write(render_accounts(accounts(journal, args.filters)))


def run_stats(journal, args, out):
    out.write(render_stats(stats(journal)))


def run_print(journal, args, out):
    out.write(format_journal(journal))


COMMANDS = {"balance": run_balance, "accounts": run_accounts, "stats": run_stats, "print": run_print}


def main(argv=None, out=None, err=None):
    """Runs the command line in `argv`, writing to `out` and `err`. Returns the exit status."""
    out = out or sys.stdout
    err = err or sys.stderr
    args = build_parser().parse_args(argv)
    try:
        journal = load(args.file)
    except ParseError as e:
        err.write(f"{PROGRAM}: {e}\n")
        return 2
    except OSError as e:
        err.write(f"{PROGRAM}: {args.file}: {e.strerror}\n")
        return 2
    COMMANDS[args.command](journal, args, out)
    return 0
