# Split the reports module

**Kind:** refactor.

`ledger/reports.py` holds every report and its text rendering. With the register, periods and budget reports coming, it will be unreadable. Make `ledger.reports` a package, with one module per report and one for the shared text layout:

- `ledger/reports/balance.py` holds `balance` and its rendering.
- `ledger/reports/accounts.py` holds `accounts`, `stats` and their rendering.
- `ledger/reports/journal.py` holds `format_journal`.
- `ledger/reports/render.py` holds the shared helpers for amount columns and tables. The contents of today's `ledger/text.py` move there, and `text.py` goes.
- New reports each get their own module: `register.py`, `periods.py` and `budget.py`.
- `ledger.reports` re-exports every report function, and `ledger` exports exactly what it does today. No behaviour changes.
