# Wrapping crypto tokens (and DeFi rewards)

> **Source:**
> https://www.ato.gov.au/individuals-and-families/investments-and-assets/crypto-asset-investments/decentralised-finance-and-wrapping-crypto
> ("Decentralised finance and wrapping crypto", QC 73649, last updated
> 19 August 2026). Mirrored here are the page's **Wrapped tokens** section and
> its **DeFi interest and rewards** paragraph — the two parts this system's
> crypto entry paths rest on. The page's DeFi lending / liquidity-pool
> analysis (which CGT event a given protocol triggers) is a question answered
> before entry and is not mirrored.
> **Retrieved:** 2026-10-08 (re-captured: the Wrapped tokens section was rewritten
> on 19 August 2026 alongside Draft Taxation Determination TD 2026/D2 — the
> conclusion is unchanged, but the event is now named as **CGT event C2** and the
> single BTC → WBTC example became a wrap example and an unwrap example)
> The live ATO site is authoritative; this is a convenience mirror.

## Wrapped tokens

A wrapped token is a crypto asset that is tied to the value of another crypto
asset. You can typically unwrap it to receive an equivalent amount of the
original type of crypto asset.

Wrapped tokens can allow:

- the value from one blockchain (for example, Bitcoin) to be used on another
  blockchain (for example, Ethereum)
- the value of a crypto asset of a certain standard to be represented as a
  different standard, such as creating a version of ETH that can be used with
  DeFi platforms.

There are different ways to wrap and unwrap crypto assets. This guidance focuses
on wrapping and unwrapping using a smart contract. A smart contract is a
blockchain program that automatically carries out a set of steps. In this
context, it can be used to exchange a crypto asset for its wrapped version.

### Wrapping crypto assets using a smart contract

In a wrapping arrangement that uses a smart contract, you send the original
crypto asset from your wallet to the smart contract's address. The smart
contract creates a wrapped version of the crypto asset, which can be used in
DeFi protocols or traded. The original crypto asset is locked at the smart
contract's address.

If you wrap a crypto asset, such as ETH, using a smart contract, **CGT event C2
happens when you send the original crypto asset to the smart contract
address**. This is because you no longer control that crypto asset through your
private key.

**The capital proceeds from this event are the market value of the wrapped
crypto asset you receive.** This will usually correspond to the market value of
the original crypto asset at the time it is sent to the smart contract.

**The first element of the cost base of your wrapped crypto asset is the market
value of the original crypto asset at the time it was sent to the smart
contract.**

> **Example 5: CGT treatment when wrapping contracts using a smart contract**
>
> Kal bought 5 ETH for \$10,000. Four years later, Kal decides to wrap the ETH
> to receive WETH. Kal wraps 5 ETH by sending them to a smart contract address.
> The smart contract creates 5 WETH and sends them to Kal's wallet address.
>
> The market value of ETH at the time of wrapping was \$30,000. CGT event C2
> happens when the ETH is sent to the smart contract. The capital proceeds are
> \$30,000, being the market value of the WETH received. The cost base of the
> 5 ETH is \$10,000
>
> Kal will have a **capital gain of \$20,000 (\$30,000 − \$10,000)** from
> exchanging ETH for WETH.
>
> The 5 WETH Kal now holds have a **cost base of \$30,000**. This example
> excludes any gas or platform fees.

### Unwrapping crypto assets using a smart contract

CGT event C2 happens if you unwrap a wrapped crypto asset, such as WETH, to
receive an equivalent amount of the original type of crypto asset, such as ETH.

The CGT event happens when the wrapped crypto asset is burnt under the smart
contract.

The capital proceeds are the market value of new crypto asset you receive from
the smart contract. The first element of the cost base of your new crypto asset
is the market value of the wrapped crypto asset at the time it was burnt.

> **Example 6: CGT treatment when unwrapping crypto assets using a smart contract**
>
> After a few months, Kal stops using the 5 WETH and unwraps it to receive ETH.
> He does this using the smart contract. The 5 WETH are burnt, and 5 ETH valued
> at \$28,000 are sent to Kal's wallet address.
>
> CGT event C2 happens when the ETH are burnt under the smart contract. The
> capital proceeds from the event are \$28,000, being the market value of the
> ETH received at that time. The reduced cost base of Kal's 5 WETH is \$30,000.
> As a result, Kal makes a **capital loss of \$2,000 (\$30,000 − \$28,000)**.
> The cost base of the 5 ETH that Kal receives is **\$28,000**. This example
> excludes any gas or platform fees.

For more information, see Draft Taxation Determination TD 2026/D2 *Income tax:
capital gains tax consequences of using a smart contract to wrap and unwrap crypto
assets*.

> **TD 2026/D2** (https://www.ato.gov.au/law/view/print?DocID=DXT%2FTD2026D2%2FNAT%2FATO%2F00001&PiT=99991231235958,
> retrieved 2026-10-08), dated 19 August 2026, a draft: its Example 1 (Finella) carries
> the same figures as Kal's wrap above, and its date of effect reads "When the final
> Determination is issued, it is proposed to apply both before and after its date of
> issue." Its explanation is why a wrap is a disposal at all: "Crypto asset A (for
> example, ETH) and any subsequent holding of the corresponding crypto asset B (for
> example, WETH), which is minted upon wrapping crypto asset A, are each separate CGT
> assets despite being tied in value."

## Crypto asset DeFi interest and rewards

DeFi platforms may pay you a reward that is a type of return or yield for
crypto assets that are placed in the DeFi platform accounts. If you receive
periodic rewards in the form of a crypto asset from a DeFi platform you must
report the **market value of the crypto asset reward at the time of receipt as
assessable income** in your tax return. The rewards of crypto assets are taxed
similarly to interest income.

> **Example 4: crypto asset reward from DeFi platform**
>
> Craig 'lends' 100 stablecoin tokens valued at \$10 per token through the DeFi
> platform Compound Finance. The DeFi platform pays a rate of return of 1% in
> the form of newly issued stablecoin tokens.
>
> Craig will need to declare the market value of the newly issued tokens he
> earns as assessable income in his tax return. The income amount Craig
> declares is \$10. The cost base of the newly issued tokens is their market
> value at the time Craig acquires them.

---

## Superseded capture (retrieved 2026-08-18, page last updated 22 June 2026)

> The Wrapped tokens section's conclusion and example as they read before the
> 19 August 2026 rewrite. The conclusion is the same as today's; kept so the
> earlier citations of "Kal's BTC → WBTC" can be followed.

> **When you wrap or unwrap a crypto asset, you exchange one crypto asset for
> another and a CGT event happens. The capital proceeds for the CGT event equal
> the market value of the wrapped token at the time of the exchange.**
>
> > **Example: CGT treatment when exchanging wrapped tokens**
> >
> > Kal bought 1 BTC for \$165,000 and then wrapped it through a smart contract
> > for 1 WBTC a few months later.
> >
> > The market value of BTC at the time of exchange was \$180,000. A CGT event
> > happens when the BTC is wrapped through that smart contract. Kal will have a
> > **capital gain of \$15,000** due to exchanging BTC for WBTC.
> >
> > The 1 WBTC Kal now holds has a market value acquisition amount of **\$180,000**
> > (the market value of BTC at time of wrapping) that will form part of its cost
> > base when it is later sold or otherwise disposed of.
