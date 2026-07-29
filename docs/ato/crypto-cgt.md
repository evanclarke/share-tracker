# Crypto assets — CGT treatment

> **Source:**
> https://www.ato.gov.au/individuals-and-families/investments-and-assets/crypto-asset-investments/how-to-work-out-and-report-cgt-on-crypto
> ("How to work out and report CGT on crypto", QC 69952, last updated 22 June 2026), and
> https://www.ato.gov.au/individuals-and-families/investments-and-assets/crypto-asset-investments/transactions-acquiring-and-disposing-of-crypto-assets/crypto-to-crypto-exchange-or-swap
> ("Crypto to crypto exchange or swap", QC 69949, last updated 22 June 2026)
> **Retrieved:** 2026-07-29
> The live ATO site is authoritative; this is a convenience mirror.

## When capital gains tax applies

The most common use of crypto is as an investment, in which case the crypto
asset is a **capital gains tax (CGT) asset**.

If you acquire a crypto asset as an investment, transactions such as disposal
or exchange or swap are a CGT event and you may make a:

- capital gain
- capital loss, which can reduce capital gains you make.

You can't deduct a net capital loss from your other income.

You may be able to reduce capital gains using the **CGT discount if you hold
your crypto asset for at least 12 months**.

If you hold the crypto asset as an investment, it will not be exempt from CGT
as a personal use asset.

To work out if you made a capital gain or capital loss from each CGT event,
keep records for each crypto asset and your transactions. You will make a
capital gain if the proceeds from the disposal of your crypto asset is more
than its cost base.

## Working out the timing of the CGT event

In general, a CGT event happens when you dispose of a CGT asset. For the
purposes of crypto assets, that may be when you:

- sell a crypto asset
- gift a crypto asset
- trade, exchange or swap one crypto asset for another
- convert a crypto asset to Australian or foreign currency
- buy goods or services with a crypto asset.

There are other CGT events, such as the loss or destruction of an asset, or
creating contractual or other rights.

## Calculating your CGT

As with other CGT assets, if your crypto assets are held as an investment, you
may pay tax on your **net capital gains** for the year. This is:

- your total capital gains
- less any capital losses
- less your entitlement to any CGT discount on your capital gains.

Before you calculate CGT on your crypto assets, you will need to:

- check you have records for your crypto assets and crypto transactions
- **convert the value of the crypto assets into Australian dollars**.

You need to keep details for **each crypto asset as they are separate CGT
assets**.

## Crypto to crypto exchange or swap

### Market value of new crypto asset at exchange or swap

When you exchange or swap one crypto asset for another crypto asset, you
dispose of one CGT asset and acquire another. Therefore, a CGT event happens
to your original crypto asset. Because you receive property instead of money,
you need to work out the **market value of the crypto asset in Australian
dollars**.

> **Example: market value of new asset determines old asset's disposal proceeds**
>
> Katrina acquires 100 Coin A for $15,000 on 5 July 2025.
>
> Katrina decides to exchange 20 Coin A for 100 Coin B through a reputable
> digital asset exchange on 15 November 2025.
>
> Using the exchange rates shown on the digital asset exchange at the time of
> the transaction, the market value of 100 Coin B was $6,000.
>
> Therefore, Katrina's capital proceeds are **$6,000** for the disposal of
> 20 Coin A. Katrina uses this amount to work out her capital gain for the
> CGT event.

### Market value of existing crypto asset at exchange or swap

If you can't determine the value of a crypto asset you receive in a crypto
asset exchange or swap, use the market value of the crypto asset you're
disposing of to work out the capital proceeds.

> **Example: market value of old crypto asset determines its disposal proceeds**
>
> Katrina acquires 100 Coin A for $15,000 on 5 July 2025.
>
> Katrina decides to exchange 20 Coin A for a new coin, Coin D, before it is
> listed on a digital exchange. Katrina acquires 100 Coin D in the exchange on
> 15 November 2025.
>
> At the time of the transaction, Coin D doesn't have a market value. Katrina
> uses the market value of Coin A on the digital asset exchange at the time of
> the transaction. The market value of 20 Coin A at the time of exchange was
> $5,000.
>
> Therefore, Katrina's capital proceeds are **$5,000** for the disposal of
> Coin A.

## Transferring crypto between wallets you own (and network fees)

> **Source:** [Crypto asset investments and tax](https://www.ato.gov.au/other-languages/information-in-other-languages/investing/crypto-asset-investments-and-tax)
> ("Crypto asset investments and tax", QC 67444, last updated 4 October 2022).
> **Retrieved:** 2026-06-08; both quoted sentences re-verified verbatim on this
> page 2026-07-29.
>
> Originally also cited [Crypto asset transactions](https://www.ato.gov.au/individuals-and-families/investments-and-assets/crypto-asset-investments/transactions-acquiring-and-disposing-of-crypto-assets/crypto-asset-transactions)
> (now QC 69948, last updated 22 June 2026). **That page no longer carries the
> wallet-transfer or network-fee sentences** — it was restructured; the
> guidance below now lives only on the QC 67444 page above. The rule itself is
> unchanged.

Moving a crypto asset between two wallets you own is **not a disposal** as long
as you keep ownership — no CGT event, exactly like a share transfer between two
of your own holding accounts. Verbatim:

> Transferring crypto assets from one digital wallet to another digital wallet
> is not considered as a disposal as long as you maintain ownership of it.
>
> **If your crypto holding reduces during a transfer to cover a network fee,
> the transaction fee is a disposal and has capital gain consequences.**

So an on-chain transfer fee paid **in the crypto** is itself a CGT event: the
fee units are disposed of at their market value (in Australian dollars) at the
transfer time, and the capital gain or loss is that AUD market value less the
fee units' share of the parcel's cost base — with the 12-month CGT discount
available if those units had been held for at least 12 months. (A transfer fee
charged in **fiat** by an exchange is not a crypto disposal; it is a
transaction cost, and like brokerage it is not separately deductible — see
[`investment-income-deductions.md`](investment-income-deductions.md).)

## How this maps to this project

A crypto asset held as an investment is a CGT asset whose gains are calculated
exactly like a share parcel's: AUD cost base and proceeds, losses netted
before the discount, and the 50% discount after 12 months. So a `Crypto`
listing (no exchange, same-day settlement, ticker = a recognised digital-token
code in `currencies`) flows through the existing parcel machinery — parcel
allocations, reduced cost base, transfers between holding accounts — with no
crypto-specific calculation code.

A holding-account transfer can carry an optional **network fee** paid in the
transferred crypto: the move stays a non-CGT event (transfer-out Sell +
transfer-in Buys carrying cost base), while the fee units are recorded as an
ordinary disposal Sell in the source account at the supplied AUD market value
— linked to the transfer (`transfers.fee_sale_trade_id`) so it is created and
removed atomically with it, but **counted by the gains reports** (with the
discount) because it is a real disposal. See `src/entities/transfer.rs`.

Out of scope (Known limitations in README): a crypto-to-crypto **swap is
entered manually** as a Sell at the market-value proceeds plus a Buy of the
acquired asset at the same value (per the Katrina examples above — reproduced
in `src/ato_examples.rs`); **staking rewards/airdrops** are entered manually
(an income row plus a Buy at receipt-date market value); chain splits/forks,
wrapping, the personal-use-asset exemption, and Div 775 foreign-currency
balances are not modelled.
