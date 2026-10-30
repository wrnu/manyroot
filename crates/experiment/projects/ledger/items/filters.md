# A real filter syntax

**Kind:** feature.

Today a filter selects any account whose name contains the text: `Food` also selects `Expenses:Seafood`. I want account patterns that follow the tree, wildcards, exclusions, and a way to filter by description, as spec.md section 3 describes.

- `Expenses:Food` selects that account and its descendants. Matching is case-insensitive.
- `*` matches within one segment: `*:Coffee`, `Exp*`.
- `not:PATTERN` leaves accounts out.
- `desc:TEXT` keeps postings whose transaction's description contains the text.
- Library: `ledger.matches(filters, account, description="")`. Every report that takes `filters` uses the same rules: `balance`, `accounts`, and the register and periods reports when they exist.
- The CLI's `FILTER` arguments use the same syntax.
- An empty term is an error.
