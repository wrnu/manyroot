# `print` reorders transactions on the same day

**Kind:** fix.

I write transactions in the order they happened. When two share a date, `python -m ledger -f books.journal print` sometimes swaps them: same-day transactions come out alphabetically by description, not in the order I wrote them. "Withdraw cash" then "Bakery" comes out as "Bakery" then "Withdraw cash".
