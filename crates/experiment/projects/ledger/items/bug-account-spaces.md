# Account names with a space are refused

**Kind:** fix.

This transaction:

```
2024-03-02 Pizza
    Expenses:Eating Out    18.50
    Assets:Checking       -18.50
```

fails with `ledger: books.journal:2: invalid amount: Out 18.50`. Account names with a space in a segment are common, and the spec allows them.
