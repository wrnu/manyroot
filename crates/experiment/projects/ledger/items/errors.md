# One error type

**Kind:** refactor.

Errors come out as a mix of `ParseError`, bare `ValueError`, and the odd `decimal.InvalidOperation`. The CLI catches some of them and prints a traceback for the rest. Bring them under one family, as spec.md section 2 describes:

- A new module `ledger/errors.py` holds:
  - `LedgerError(ValueError)`, the base;
  - `ParseError(message, filename="<string>", line=0)`, with `filename`, `line` and `message`, and a `str` of `FILE:LINE: message`;
  - `CsvError(message, line=0)`, with `line` and `message`;
  - `UsageError`, for bad arguments.
  - All four are exported from `ledger`.
- Library functions raise only these for bad input:
  - a bad date in the journal is a `ParseError` with its line;
  - `parse_amount` and `parse_date` raise a `LedgerError`.
- The CLI catches `LedgerError` once. It prints `ledger: MESSAGE` on one line to stderr and exits 2, never with a traceback.
