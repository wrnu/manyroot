# A `check` command

**Kind:** feature.

I'd like to check a journal before trusting its reports: transactions that don't balance, transactions with only one posting, and text that doesn't parse.

- Library: `ledger.check(text, filename="<string>")`, returning problems as spec.md section 2 describes. The kinds are `parse`, `unbalanced` and `too-few-postings`, sorted by line.
- A journal that doesn't parse gives one `parse` problem at the line of the error.
- CLI: `python -m ledger -f FILE check` prints one `FILE:LINE: ...` line per problem and exits 1 when there are problems, or prints nothing and exits 0 when there are none.
- A bad journal is a finding here, not an error, so `check` never exits 2 for it.
