# JSON output for scripts

**Kind:** feature.

I want to feed `ledger`'s reports into other tools. Add a global `--json` option that prints the report's library data as JSON instead of text (spec.md, section 6).

- `python -m ledger -f FILE --json balance` (and `register`, `periods`, `budget`, `check`, `accounts`, `stats`).
- Decimals are strings, dates are `"YYYY-MM-DD"`, and None is `null`.
- The JSON holds exactly what the library function returns, so the two never disagree.
