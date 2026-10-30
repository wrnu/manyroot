"""The journal's types: postings, transactions and the journal holding them."""

from dataclasses import dataclass, field

from .amount import total


@dataclass
class Posting:
    """An amount posted to one account. `line` is where it is written, counting from 1."""

    account: str
    amount: object
    line: int = 0


@dataclass
class Transaction:
    """A dated transaction and its postings. `line` is where its header is written."""

    date: object
    description: str
    postings: list = field(default_factory=list)
    line: int = 0

    def total(self):
        """The sum of its postings' amounts: zero when it balances."""
        return total(p.amount for p in self.postings)

    def is_balanced(self):
        return self.total() == 0

    def accounts(self):
        """The accounts it posts to, in the order written, each once."""
        seen = []
        for posting in self.postings:
            if posting.account not in seen:
                seen.append(posting.account)
        return seen


@dataclass
class Journal:
    """Transactions in the order the file writes them, and the file's name."""

    transactions: list = field(default_factory=list)
    filename: str = "<string>"

    def __len__(self):
        return len(self.transactions)

    def __iter__(self):
        return iter(self.transactions)

    def sorted(self):
        """Its transactions in date order."""
        return sorted(self.transactions, key=lambda t: (t.date, t.description))

    def postings(self, begin=None, end=None):
        """(transaction, posting) for every posting, in date order, dated from `begin` up to but not
        including `end`."""
        from .dates import in_range

        for transaction in self.sorted():
            if not in_range(transaction.date, begin, end):
                continue
            for posting in transaction.postings:
                yield transaction, posting

    def accounts(self):
        """Every account any posting names, sorted."""
        return sorted({p.account for t in self.transactions for p in t.postings})

    def date_range(self):
        """The first and last transaction dates, or (None, None) for an empty journal."""
        if not self.transactions:
            return None, None
        dates = [t.date for t in self.transactions]
        return min(dates), max(dates)
