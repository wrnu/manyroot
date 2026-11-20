# Import a bank's CSV export

**Kind:** feature.

Typing in every bank transaction by hand is tedious. My bank exports CSV. I'd like `ledger` to turn that export into journal text I can append to my journal, with rules that pick the counter-account from the description.

- Library: `ledger.import_csv(csv_text, rules_text, account, commodity="", date_format="%Y-%m-%d")`, which returns journal text, as spec.md section 4 describes.
- Rules are lines like `COFFEE -> Expenses:Coffee`. The first rule whose pattern occurs in the description wins. Unmatched rows go to `Expenses:Unknown` or `Income:Unknown`.
- Bank amounts look like `1,234.56`, `-12.00` or `(12.00)`.
- CLI: `python -m ledger import export.csv --rules bank.rules --account Assets:Checking [--commodity USD] [--date-format %d/%m/%Y]`. It writes the journal text to stdout, and doesn't need `-f`.
- Bad input raises `CsvError` with the line it's on.
