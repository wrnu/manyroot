# Transactions on 29 February are refused

**Kind:** fix.

My journal has a transaction dated `2024-02-29`. `python -m ledger -f books.journal balance` stops with a traceback ending in `ValueError: invalid date: 2024-02-29 (no day 29 in that month)`. 2024 is a leap year, so that day exists. `balance -b 2024-02-29` is refused the same way.
