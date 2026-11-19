"""Reading journal text into a Journal.

A journal is a sequence of transactions, each a header line and its postings:

    ; a comment
    2024-01-05 Grocery store
        Expenses:Food        42.10
        Assets:Checking     -42.10

See spec.md for the whole format.
"""

from .accounts import check_name
from .amount import parse_amount
from .dates import parse_date
from .journal import Journal, Posting, Transaction

COMMENT_STARTS = (";", "#")


class ParseError(ValueError):
    """Journal text that doesn't follow the format, with where it is."""

    def __init__(self, message, filename="<string>", line=0):
        super().__init__(f"{filename}:{line}: {message}")
        self.message = message
        self.filename = filename
        self.line = line


def is_comment(text):
    return text.lstrip().startswith(COMMENT_STARTS)


def is_blank(text):
    return not text.strip()


def is_indented(text):
    return text[:1] in (" ", "\t")


class _Reader:
    """The state of one parse: the journal so far and the transaction being read."""

    def __init__(self, filename):
        self.filename = filename
        self.journal = Journal(filename=filename)
        self.current = None

    def error(self, message, line):
        return ParseError(message, self.filename, line)

    def header(self, text, line):
        parts = text.split(None, 1)
        date = parse_date(parts[0])
        description = parts[1].strip() if len(parts) > 1 else ""
        self.current = Transaction(date=date, description=description, line=line)
        self.journal.transactions.append(self.current)

    def posting(self, text, line):
        if self.current is None:
            raise self.error("posting outside a transaction", line)
        parts = text.split()
        if len(parts) < 2:
            raise self.error("missing amount", line)
        account, amount_text = parts[0], " ".join(parts[1:])
        try:
            check_name(account)
            amount = parse_amount(amount_text)
        except ValueError as e:
            raise self.error(str(e), line) from e
        self.current.postings.append(Posting(account=account, amount=amount, line=line))

    def read(self, text, line):
        if is_blank(text):
            self.current = None
        elif is_comment(text):
            pass
        elif is_indented(text):
            self.posting(text.strip(), line)
        else:
            self.header(text, line)


def parse(text, filename="<string>"):
    """The journal `text` holds. Raises ParseError where it doesn't follow the format."""
    reader = _Reader(filename)
    for number, raw in enumerate(text.splitlines(), start=1):
        reader.read(raw.rstrip("\r\n"), number)
    return reader.journal


def load(path):
    """The journal in the file at `path`."""
    with open(path, encoding="utf-8") as f:
        return parse(f.read(), filename=str(path))
