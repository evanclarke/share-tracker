# TODO

Items are only marked done when a passing test exists for them.

Completed and decided (out-of-scope / not-reproducible) sections are archived in [DONE.md](DONE.md) to keep this list focused on active work. When a section here is fully done, move it to DONE.md rather than leaving it — see CLAUDE.md.




## FX conversion granularity — spot-rate override for one-off capital transactions (2026-06-12)

(REQUIREMENTS 2026-06-12: QC 18020 Examples 5/7 — an average rate is not a reasonable
approximation for a one-off purchase/sale of a large capital asset; today the monthly RBA rate is
compulsory because the per-trade `fx_rate` is fallback-only. Sources: `docs/ato/forex-average-rates.md`,
`docs/ato/forex-common-transactions.md`.)

- [ ] A trade (Buy, DRP, Sell) can carry an explicit spot-rate override that wins over the
      imported monthly RBA rate everywhere the trade's amounts convert to AUD (cost base,
      proceeds, every report and the snapshot pipeline). Design-open: promote `fx_rate` via an
      explicit flag, or a separate column — but entry must be deliberate; the silent fallback
      semantics of existing `fx_rate` rows must not flip
- [ ] Absent an override, behaviour is unchanged: monthly RBA rate first, `fx_rate` fallback,
      loud failure when neither exists (all pre-existing FX tests pass unmodified)
- [ ] Docs sync: `docs/API.md` FX conversion section states the rule honestly (monthly = the
      ATO-published convenience default, reasonable for recurring/small amounts; a one-off large
      foreign disposal should carry the transaction-date spot rate per QC 18020); `docs/SCHEMA.md`
      for any new column/flag; README FX bullet; web UI trade/Sell forms expose the override

## Settlement-window forex on foreign-currency trades — CGT events K10/K11 (2026-06-12)

(REQUIREMENTS 2026-06-12: under the default forex 12-month rule the contract-to-settlement
currency movement adjusts the cost base on an acquisition and is a separate non-discountable
K10 gain / K11 capital loss on a disposal — QC 17062, Art Ltd and Eleanor examples; the system
computes neither. Source: `docs/ato/forex-cgt-12-month-rule.md`. NEEDS DECISION: model it, or
resolve out of scope as a Known limitation.)

- [ ] Decide the scope explicitly: either model it — for a non-AUD trade, compute the forex
      movement between the trade-date and settlement-date translations of the consideration,
      folding it into the parcel's cost base on a Buy/DRP and surfacing it as a separate
      non-discountable K10 gain / K11 capital loss feeding the realised-gains and
      net-capital-gain reports on a Sell — or resolve it out of scope as a Known-limitations
      entry stating settlement-window forex outcomes are the taxpayer's manual adjustment
      (doc-only resolution is test-pinned via `src/doc_checks.rs`, citing
      `docs/ato/forex-cgt-12-month-rule.md`)
- [ ] The resolution notes the interaction with the spot-rate override above: with monthly rates
      and a same-rate-month T+2 settlement the component is nil by construction; per-leg spot
      rates are what make it visible
