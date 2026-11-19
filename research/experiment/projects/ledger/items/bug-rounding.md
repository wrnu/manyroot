# Amounts come out slightly wrong

**Kind:** fix.

I track fuel by the litre with prices to three decimals. A posting of `0.125` shows up as `0.12` in the balance report, and three of them add up to `0.36` instead of `0.375`. Worse, this transaction balances, yet the report's total is `-0.02` instead of `0`:

```
2024-01-01 Fuel
    Expenses:Fuel    0.125
    Expenses:Fuel    0.125
    Expenses:Fuel    0.125
    Assets          -0.375
```

A very large amount, `12345678901234567.89`, also comes out different from what I typed.
