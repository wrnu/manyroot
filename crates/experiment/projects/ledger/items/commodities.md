# Amounts with a commodity

**Kind:** refactor.

Everything assumes a single currency. I hold both USD and EUR, and the journal can't say which a posting is in. Amounts need a commodity, as spec.md describes:

- **Journal text:** an amount may be followed by a commodity, as in `42.10 USD`. Without one, the commodity is `""`.
- **A new type:** `ledger.Amount(quantity, commodity="")`, an immutable value with a Decimal `quantity`. `Posting.amount` becomes an Amount, and `parse_amount` returns one. Adding or subtracting different commodities is an error.
- **Formatting:** `format_amount(Amount)` shows the commodity after the number.
- **Report data:** every report keeps per-commodity totals in an amount map, `{commodity: Decimal}`, which never holds a zero.
  - `balance` rows become `{"account", "amounts"}`, and `total` becomes a map.
  - `stats`' `unbalanced` checks each commodity.
- **The balance text:** one line per account and commodity.
- Update the public tests to the new shapes.

This touches nearly every module. Anyone working on a report should expect amounts to carry a commodity.
