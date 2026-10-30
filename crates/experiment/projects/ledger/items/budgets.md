# Budgets

**Kind:** feature.

I'd like to set a monthly budget per account and see how each month went against it.

- Journal syntax: a `budget monthly` or `budget YYYY-MM` line at column 0, followed by indented `Account  amount` lines (spec.md, section 1).
  - A month's own block replaces the `monthly` entries for the accounts it names.
  - Budget blocks are not transactions, and no other report sees them.
- Library: `ledger.budget(journal, month)` with `month` as `"YYYY-MM"`, returning one row per budgeted account, as in spec.md section 2. A month that doesn't exist, such as `"2024-13"`, is an error.
  - `actual` includes postings to the account's descendants.
  - The sign of `difference` is yours to choose (spec.md, section 5), but use one rule everywhere.
- CLI: `python -m ledger -f FILE budget --month 2024-03`. It prints one line per budgeted account, with its name and its budget, actual and difference amounts, as `format_amount` shows them.
