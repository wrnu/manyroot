# Change: a posting may leave its amount out

**Kind:** change. It revises `check`.

From now on, one posting in a transaction may omit its amount. It then takes the amount that makes the transaction balance:

```
2024-04-01 Groceries
    Expenses:Food     23.75 USD
    Assets:Checking
```

Here `Assets:Checking` gets `-23.75 USD`.

- **Errors:** each is a `ParseError`, at the line given here:
  - More than one posting in a transaction without an amount: at the line of the second posting without one.
  - An omitted amount where the other postings leave more than one commodity unbalanced: at the transaction's header line.
  - An omitted amount where the other postings already balance: at the transaction's header line.
- **Everything after parsing sees the filled-in amount:** every report, `print`, and `stats`.
- **`check`:** a transaction with one omitted amount is no longer a problem. A transaction that can't be completed is a `parse` problem, at the line the `ParseError` names.
- **Budget blocks** still need every amount.
- The public test that expects a missing amount to be an error must change.
