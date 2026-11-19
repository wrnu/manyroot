# Totals by month, quarter or year

**Kind:** feature.

I want to see how spending moves over time: the selected postings totalled per calendar period. Add `periods`, as spec.md describes (sections 2 and 6).

- Library: `ledger.periods(journal, by, filters=(), begin=None, end=None)`, where `by` is `"month"`, `"quarter"` or `"year"`.
- Every period from the first selected posting's to the last's is listed, including empty ones.
- Labels: `YYYY-MM` for months and `YYYY` for years. Choose the quarter label from spec.md, section 5, and use it everywhere.
- CLI: `python -m ledger -f FILE periods --by month|quarter|year [FILTER...] [-b DATE] [-e DATE]`. It prints one line per period, with its label and its amounts, as `format_amount` shows them.
- An unknown `by` is an error, not an empty report.
