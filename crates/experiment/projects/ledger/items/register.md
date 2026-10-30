# A register report

**Kind:** feature.

`ledger` can show balances, but not the postings behind them. Add a register: every selected posting in date order, with a running total, as `register` in spec.md (sections 2 and 6).

- Library: `ledger.register(journal, filters=(), begin=None, end=None)`, returning the rows spec.md describes.
- CLI: `python -m ledger -f FILE register [FILTER...] [-b DATE] [-e DATE]`. The text layout is yours to choose (spec.md, section 5), but each line shows the date, description, account, amount and running total.
- Transactions on the same date keep their file order, and postings within a transaction keep the order written.
