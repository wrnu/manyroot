# ledger

`ledger` is plain-text double-entry bookkeeping in Python 3.12, using only the standard library. It reads a journal of dated transactions and reports on it, both as a library (the `ledger` package) and as a command line (`python -m ledger`).

The seed repository already implements part of this spec. The items in the issue list describe what is missing or wrong. This spec describes the finished program, except where an item says otherwise. When this spec and an item disagree, the item wins. The public tests are in `tests/`. Run them with `python -m unittest discover -s tests` from the repository's top.

## 1. The journal format

A journal is UTF-8 text made of transactions, budget blocks, comments and blank lines.

```
; Opening the books
2024-01-01 Opening balance
    Assets:Checking        1000.00 USD
    Equity:Opening        -1000.00 USD

2024-01-05 Grocery store
    Expenses:Eating Out      42.10 USD
    Assets:Checking         -42.10 USD

budget monthly
    Expenses:Eating Out     300.00 USD
```

- **Comments:** a line whose first non-blank character is `;` or `#`. It is ignored wherever it appears.
- **Blank lines** end the transaction or budget block before them.
- **A transaction** is a header line at column 0, followed by its postings.
  - The header is a date in `YYYY-MM-DD` form, then optionally one or more spaces and a description, which runs to the end of the line with surrounding spaces removed.
  - The date must be a real day of the Gregorian calendar.
- **A posting** is an indented line (at least one space or tab) inside a transaction. It holds an account name, then a separator of two or more spaces or a tab, then an amount.
  - The amount is required. A posting with no amount is an error.
  - A transaction need not balance to be read. Checking balance is the `check` command's job.
- **An account name** is segments joined by `:`, such as `Expenses:Eating Out`.
  - Each segment is non-empty and has no leading or trailing space.
  - A segment may contain single spaces, but not two spaces in a row or a tab, since those separate the account from the amount.
- **An amount** is a number, then optionally one or more spaces and a commodity.
  - The number is an optional `-`, digits, and an optional `.` followed by digits.
  - The commodity is 1 to 10 ASCII letters, such as `USD` or `EUR`, and is case-sensitive. An amount without one has the empty commodity, `""`.
  - Amounts are exact decimals. The journal's text is never rounded.
- **A budget block** is a line `budget monthly` or `budget YYYY-MM` at column 0, followed by indented lines, each an account and an amount, separated as in a posting.
  - A block names what each account is budgeted per month: every month for `monthly`, or that one month.
  - Within a block, an account may appear once for each commodity.
  - Budget blocks are not transactions. No report but the budget report sees them.

Every error in journal text names the file and line where it is.

## 2. The library

Everything below is importable from the `ledger` package itself, such as `from ledger import balance`.

### Types

- `Amount(quantity, commodity="")`: an immutable value.
  - `quantity` is a `decimal.Decimal`, and `commodity` is a str.
  - Amounts compare equal when both fields are equal.
  - `+` and `-` work on two amounts of the same commodity, and unary `-` negates. Adding or subtracting amounts of different commodities raises `LedgerError`.
- `Posting`: `account` (str), `amount` (Amount) and `line` (int, counting from 1).
- `Transaction`: `date` (`datetime.date`), `description` (str), `postings` (a list of Posting, in the order written) and `line` (the header's line).
- `Journal`: `transactions` (a list of Transaction, in file order) and `filename` (str).

### Reading

- `parse(text, filename="<string>")` returns the Journal that `text` holds. It raises `ParseError` where the text doesn't follow section 1.
- `load(path)` returns `parse` of the file at `path`, with `filename` set to `path`.
- `parse_amount(text)` returns the Amount written as `text`.
- `parse_date(text)` returns the `datetime.date` written as `text`.
- `format_amount(amount)` returns the amount as reports show it:
  - the quantity with at least two decimal places, and more when its exact value needs them (`0.125` shows as `0.125`, `3` as `3.00`);
  - a leading `-` when negative, and never `-0.00`;
  - then a space and the commodity, if it has one, such as `-42.10 USD`.

### Reports

Reports return plain data: dicts, lists, str, int, `Decimal`, `datetime.date` and None, never library objects. These conventions hold in every report:

- **Amount maps:** an amount in a report that may hold several commodities is a dict from commodity to `Decimal` quantity. It never holds a zero quantity, so a total of zero is `{}`.
- **Date ranges:** `begin` and `end` are `datetime.date` or None. A posting is included when its transaction's date is on or after `begin` and before `end`. None means no limit.
- **Date order:** transactions are taken in date order. Transactions with the same date keep their file order.
- **Filters:** `filters` is a sequence of filter terms, as in section 3, selecting postings.

The reports:

- `balance(journal, filters=(), begin=None, end=None, depth=None)` returns `{"accounts": [{"account": str, "amounts": {...}}, ...], "total": {...}}`.
  - The rows are every account a selected posting names, and all their ancestors, sorted by name. Each holds the sum of the selected postings to it and its descendants.
  - With `depth`, only accounts with at most that many segments are listed. Their sums still include deeper accounts.
  - `total` is the sum of every selected posting.
- `register(journal, filters=(), begin=None, end=None)` returns one row per selected posting, in date order and within a transaction in the order written:
  ```
  {"date": date, "description": str, "account": str, "quantity": Decimal, "commodity": str, "running": {...}}
  ```
  `running` is the sum of this posting and every row before it.
- `periods(journal, by, filters=(), begin=None, end=None)` totals the selected postings per calendar period. `by` is `"month"`, `"quarter"` or `"year"`.
  - It returns `[{"period": label, "amounts": {...}}, ...]`, for every period from the one holding the first selected posting to the one holding the last, in order, including periods with none (`{}`).
  - With no selected postings it returns `[]`.
  - Labels are `YYYY-MM` for months and `YYYY` for years. For quarters, see section 5.
- `budget(journal, month)` compares a month's budget with what was spent. `month` is a `YYYY-MM` string naming a real month (01 to 12). Any other value raises `UsageError`.
  - It returns one row per account that has a budget that month, sorted by account:
    ```
    {"account": str, "budget": {...}, "actual": {...}, "difference": {...}}
    ```
  - A `budget YYYY-MM` block's entries for an account replace that account's `budget monthly` entries for that month. If several blocks give the same account and commodity for one month, the last in the file wins.
  - `actual` is the sum of that month's postings to the account and its descendants.
  - `difference` compares the two; see section 5.
- `accounts(journal, filters=())` lists every account the selected postings name, sorted:
  ```
  [{"account": str, "postings": int, "first": date, "last": date}, ...]
  ```
- `stats(journal)` returns `{"transactions": int, "postings": int, "accounts": int, "first": date or None, "last": date or None, "unbalanced": int}`. `unbalanced` counts the transactions whose postings don't sum to zero in every commodity.

### Other functions

- `check(text, filename="<string>")` lists what is wrong with a journal, as `[{"line": int, "kind": str, "message": str}, ...]` sorted by line. It returns `[]` for a good journal. The kinds:
  - `"parse"`: the text doesn't parse. This is the only problem listed then, at the error's line.
  - `"unbalanced"`: a transaction's postings don't sum to zero in some commodity. Listed at the header's line.
  - `"too-few-postings"`: a transaction has fewer than two postings. Listed at the header's line.
- `import_csv(csv_text, rules_text, account, commodity="", date_format="%Y-%m-%d")` turns a bank's CSV export into journal text. See section 4.
- `matches(filters, account, description="")` says whether filter terms select a posting to `account` in a transaction described as `description`. See section 3.
- `format_journal(journal)` returns the journal as text that `parse` reads back to the same transactions.
  - It writes transactions in date order, with a blank line between them, and each posting's amount in `format_amount`'s form.
  - Each transaction's header line is its date, one space and its description, or the date alone when the description is empty, such as `2024-01-05 Grocery store`.
  - It doesn't write budget blocks.

### Errors

- `LedgerError`, a subclass of `ValueError`, is the base of every error the library raises for bad input. No other exception type escapes it for bad journal text, bad arguments or bad CSV.
- `ParseError(LedgerError)` is constructed as `ParseError(message, filename="<string>", line=0)`. It has `filename`, `line` and `message`, and its `str` is `FILE:LINE: message`.
- `CsvError(LedgerError)` is constructed as `CsvError(message, line=0)`. It has `line`, the CSV or rules line, and `message`.
- `UsageError(LedgerError)` is raised for a bad argument to a library function:
  - an unknown `by`;
  - a month that isn't a real month in `YYYY-MM` form, such as `2024-13`, `2024-00` or `March`;
  - an empty filter term.
- `parse_amount` and `parse_date` raise a `LedgerError` for text that isn't an amount or a date.

## 3. Filters

A filter term is one of:

- **An account pattern**, such as `Expenses:Food` or `*:Coffee`: segments joined by `:`.
  - It selects an account when each of its segments matches the account's segment at the same position, so it selects that account and its descendants.
  - Matching is case-insensitive. A `*` in a pattern segment matches any run of characters within one segment, including none.
  - `Exp*` selects `Expenses` and everything under it. `*:Food` selects `Expenses:Food` and `Income:Food:Stall`, but not `Food`.
- **`not:` followed by an account pattern** leaves out the accounts the pattern selects.
- **`desc:` followed by text** selects postings whose transaction's description contains the text, case-insensitively.

A posting is selected when all of these hold:

- its account matches at least one account pattern, or there is none;
- it matches no `not:` pattern;
- its description contains every `desc:` text.

No terms at all select everything.

## 4. CSV import

`import_csv` reads `csv_text` with the `csv` module's default dialect.

- **The header:** the first row is a header. Its column names are matched case-insensitively and with surrounding spaces removed.
  - `date`, `description` and `amount` are required.
  - `commodity` is optional. A non-empty cell in it overrides the `commodity` argument for that row.
- **Amount cells:** an optional `-`, digits with optional `,` thousands separators, and an optional fraction. Parentheses around the whole, such as `(12.50)`, mean negative.
  - A row whose amount cell is empty is skipped.
- **Date cells** are read with `date_format`, in `datetime.datetime.strptime`'s codes.
- **Rules:** each line of `rules_text` is `PATTERN -> ACCOUNT`. Blank lines and lines starting with `#` or `;` are ignored.
  - A row's counter-account is the first rule whose pattern occurs in the row's description, case-insensitively.
  - With no matching rule, the counter-account is `Expenses:Unknown` for a negative amount, and `Income:Unknown` otherwise.
- **The result:** each row becomes a transaction, in the CSV's order. It has the row's date and description (with surrounding spaces removed), a posting to `account` of the row's amount, and a posting to the counter-account of its negation.
- **Errors:** a missing column, a bad date, a bad amount or a malformed rules line raise `CsvError`. Its `line` is the line in the CSV (the header is line 1) or in the rules.

## 5. Open choices

The spec leaves these open. Any of the listed answers is right, but the library, the text output and the JSON must all agree.

- **Quarter labels** are `2024-Q1`, `2024Q1` or `Q1 2024`.
- **A budget row's `difference`** is either budget minus actual, or actual minus budget, per commodity, with zeros left out as in every amount map. One rule for every row.
- **The layout of the register's text output** is free, as long as each line shows the date, description, account, amount and running total.

## 6. The command line

`python -m ledger [-f FILE] [--json] COMMAND [options]`

- `-f FILE` names the journal. Every command but `import` needs it.
- `--json` prints the command's library data as JSON instead of text:
  - `Decimal` as a string, such as `"42.10"`;
  - dates as `"YYYY-MM-DD"`;
  - None as `null`.
  - It applies to `balance`, `register`, `periods`, `budget`, `check`, `accounts` and `stats`.

The commands are each the library function of the same name. Every text form follows these rules:

- An amount is shown in `format_amount`'s form.
- An amount map with nothing in it, such as a zero total, is shown as `0`. This is the one exception to the rule above.
- A date is shown as `YYYY-MM-DD`, such as `2024-01-05`. That holds in `register`, `accounts`, `stats`, `print` and `import`.

- `balance [FILTER...] [-b DATE] [-e DATE] [--depth N]`
  - `N` is a whole number of at least 1. Anything else, such as `0`, is a bad argument.
  - The text form has one line per account and commodity: the amount right-aligned in 16 characters, two spaces, then the full account name.
  - An account with several commodities takes a line for each, sorted by commodity.
  - Then 16 `-` characters, then the total, one line per commodity. A zero total is one line with `0` right-aligned, not `0.00`: the empty-map exception above.
- `register [FILTER...] [-b DATE] [-e DATE]`
  - The text form has one line per row, with its date, description, account, amount and running total. The layout is yours (section 5).
- `periods --by month|quarter|year [FILTER...] [-b DATE] [-e DATE]`
  - The text form has one line per period, with its label and its amounts.
- `budget --month YYYY-MM`
  - The text form has one line per budgeted account, holding its name and its budget, actual and difference amounts.
  - It may also have a header line.
- `check`
  - Prints one line per problem, beginning `FILE:LINE: ` (FILE as given to `-f`), and exits 1 if there is any.
  - Prints nothing and exits 0 for a good journal.
  - A journal that doesn't parse is a problem here, not an error.
- `accounts [FILTER...]`
  - The text form has one line per account: its name, its number of postings, and its first and last dates, separated by spaces.
- `stats`
- `print`: `format_journal`'s text.
- `import CSV --rules RULES --account ACCOUNT [--commodity C] [--date-format F]`: `import_csv`'s text, on stdout.

Exit status:

- 0 on success.
- 1 for `check` with problems.
- 2 for any error: a `LedgerError`, a file that can't be read, or bad arguments, such as an unknown command, a missing required option or a value an option doesn't take. The error goes to stderr as one line beginning `ledger: `, with nothing on stdout and no traceback.
