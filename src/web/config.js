//
// share-tracker frontend: the configuration the generic engine renders. Each
// domain entity is described once (API path, key, fields, columns), each
// report once (API path, method, price/as-of options), and each post-record
// action once (owner fetch, fields, POST endpoint, texts) — app.js's generic
// list/form/report/action code does the rest. Adding or changing an entity,
// report, or action means editing the matching entry here, not adding views.
//
import { describeTrade, tradeOrigin, apiUrl, confirmGeneratedAdjustments } from './util.js';
import { txt, dec, int, dt, bool, sel, fk, wireGstBrokerage, wireIncomeEntry, wireAmmaEntry } from './forms.js';

// Top menu bar order (nav.js's navModel groups ENTITIES/REPORTS by their
// `menu` field into this order). Reports additionally group into titled
// sections within its panel — see the comment above REPORTS.
export const MENUS = ['Activity', 'Reports', 'Reference Data', 'Jobs'];

// ---- entity configuration --------------------------------------------
export const ENTITIES = [
  {
    slug: 'exchanges', title: 'Exchanges', menu: 'Reference Data', api: '/exchanges',
    desc: 'Curated trading venues. Seeded with XASX (ASX) and XNYS (NYSE).',
    keyFields: [txt('mic', 'MIC', { required: true })],
    fields: [
      txt('name', 'Name', { required: true }),
      txt('country', 'Country', { required: true }),
      fk('currency', 'Default currency', 'currencies', { required: true, encode: 'string' }),
      txt('timezone', 'Timezone', { required: true, default: 'Australia/Sydney' }),
      int('settlement_days', 'Settlement days (T+N)', { required: true, default: '2' }),
      txt('close_time', 'Close time (HH:MM local)', { required: true, default: '16:00', hint: 'End of the regular session in the exchange timezone; closing prices are only collected after this.' }),
    ],
    columns: ['mic', 'name', 'country', 'currency', 'timezone', 'settlement_days', 'close_time'],
  },
  {
    slug: 'exchange_holidays', title: 'Exchange Holidays', menu: 'Reference Data', api: '/exchange_holidays',
    desc: 'Full-closure non-trading days; settlement skips these as well as weekends, and valuation reads the calendar live — so a change here re-values every stored snapshot from that date. Edits and deletions are recorded in Row History (the id column is the row id to look one up by).',
    keyFields: [fk('mic', 'Exchange', 'exchanges', { required: true, encode: 'string' }), dt('holiday_date', 'Date', { required: true })],
    fields: [txt('name', 'Name', { required: true })],
    columns: ['id', 'mic', 'holiday_date', 'name'],
  },
  {
    slug: 'listings', title: 'Listings', menu: 'Reference Data', api: '/listings',
    desc: 'Securities you trade, each on a curated exchange — except Crypto listings, which have no exchange (leave it blank), settle same-day, and need a recognised digital-token ticker (e.g. BTC). Only BTC and ETH are recognised until the ISO 24165 (DTIF) registry has been imported \u2014 run the currency-import job with its DTI credentials to widen the list. A renamed security keeps its listing — its id is what every trade, distribution and corporate action references — so never create a second one: once anything is recorded against a listing, a ticker or exchange change goes through the row’s Rename action (a dated, audited event), which is what the form’s refusal points at. Rename history shows the recorded chain and undoes the newest entry.',
    keyFields: [int('id', 'ID', { auto: true })],
    fields: [
      fk('exchange_mic', 'Exchange', 'exchanges', { optional: true, encode: 'string', hint: 'Required except for Crypto; must be blank for Crypto.' }),
      txt('ticker', 'Ticker', { required: true }),
      txt('name', 'Name', { required: true }),
      txt('isin', 'ISIN', { optional: true }),
      sel('security_type', 'Security type', ['Share', 'ETF', 'LIC', 'Trust', 'Crypto'], { required: true }),
      fk('currency', 'Currency', 'currencies', { required: true, encode: 'string' }),
      bool('amit', 'AMIT', { hint: 'An attribution managed investment trust. Not available on a Crypto listing — a crypto asset is not an interest in a trust.' }),
      dt('amit_from', 'AMIT from', { optional: true, hint: 'Only for a fund that converted: the 1 July its first AMIT financial year began. Earlier years stay ordinary trust income — their distributions keep their franking credits and tax-deferred amounts, and no AMMA statement is expected for them. Leave blank for a fund that has always been an AMIT.' }),
      dt('unpriced_from', 'Unpriced from', { optional: true, hint: 'The date the price provider stopped quoting the security — a delisting, or a suspension that can run for years. From it, collection stops fetching the listing, health stops reporting its errored/unpriced days, and valuation carries the last stored closing price forward instead of blocking the whole portfolio\u2019s snapshot for that date (the snapshot is flagged \u201ccarried price\u201d). Needs a stored closing price before it, and is refused while the provider has served one on or after it. Clear it when the security is quoted again \u2014 that marks every snapshot from the date on stale so they regenerate at real prices.' }),
      dt('unpriced_before', 'Unpriced before', { optional: true, hint: 'The date the price provider’s series *begins* for the security — before it nothing is obtainable at any price (the mirror of “Unpriced from”; a spun-off entity whose quoted history starts at the spin-off). Before it, collection never fetches the listing, health stops reporting its errored/unpriced days, and valuation leaves the holding out of that date’s portfolio totals rather than blocking them (the snapshot is flagged “excluded” and names what left). Nothing is substituted — the total is smaller by a real holding and the graph steps where the series begins. It supersedes any closing price already stored for those days, whatever its origin, which is what lets a span priced from another security’s series be retired. Must fall strictly before “Unpriced from” when both are set. Clear it (or move it back) when the price can be obtained after all — that marks every snapshot before the date stale so they regenerate with the holding back in.' }),
      bool('preference', 'Preference share (90-day franking holding period)'),
    ],
    columns: ['id', 'exchange_mic', 'ticker', 'name', 'isin', 'security_type', 'currency', 'amit', 'amit_from', 'unpriced_from', 'unpriced_before', 'preference'],
    // A ticker/exchange change is not a field edit: PUT refuses one on a
    // listing with recorded trades, income or prices (422 naming
    // POST /listings/:id/rename). Rename is that endpoint, and Rename
    // history is the recorded chain (GET /listings/:id/renames) with the
    // newest entry's undo.
    rowActions: function (row) {
      return [
        { label: 'Rename', href: '#/rename/' + row.id },
        { label: 'Rename history', href: '#/renames/' + row.id },
      ];
    },
  },
  {
    slug: 'holding_accounts', title: 'Holding Accounts', menu: 'Reference Data', api: '/holding_accounts',
    desc: 'Where holdings sit within the one taxpayer (e.g. an employer share-plan account vs a personal broker account). The same listing can be held in several accounts at once, each with its own DRP enrolment; move parcels between them with a Transfer. Account 1 is the seeded default every write falls back to.',
    keyFields: [int('id', 'ID', { auto: true })],
    fields: [txt('name', 'Name', { required: true })],
    columns: ['id', 'name'],
  },
  {
    slug: 'currencies', title: 'Currencies', menu: 'Reference Data', api: '/currencies', readonly: true,
    desc: 'Recognised ISO 4217 fiat and ISO 24165 token codes (import-managed).',
    columns: ['code', 'kind', 'numeric_code', 'name', 'short_name', 'minor_units', 'source'],
  },
  {
    slug: 'mic_registry', title: 'MIC Registry', menu: 'Reference Data', api: '/mic_registry', readonly: true,
    desc: 'ISO 10383 Market Identifier Codes (import-managed).',
    columns: ['mic', 'operating_mic', 'name', 'country_code', 'city', 'status', 'expiry_date'],
  },
  {
    slug: 'jobs', title: 'Jobs', menu: 'Jobs', api: '/jobs', custom: 'jobs',
    desc: 'Run scheduled maintenance jobs (backup, reference-data imports) on demand.',
  },
  {
    slug: 'closing_prices', title: 'Closing Prices', menu: 'Jobs', api: '/closing_prices', custom: 'prices',
    desc: 'Stored daily closing prices per held listing, collected by the price-import job.',
  },
  {
    slug: 'rba_fx_rates', title: 'RBA FX Rates', menu: 'Reference Data', api: '/rba_fx_rates', readonly: true,
    desc: 'Monthly RBA F11 rates (foreign units per AUD) used for ATO conversion (import-managed).',
    columns: ['id', 'currency', 'month', 'rate'],
  },
  {
    slug: 'trades', title: 'Trades', menu: 'Activity', api: '/trades',
    desc: 'Buy acquisitions. Sells are entered under Sells so they always carry parcel allocations; DRP acquisitions are created from the funding distribution under Income (Reinvest under DRP) so the shares stay linked to their dividend and residual chain.',
    keyFields: [int('id', 'ID', { auto: true })],
    fields: [
      sel('trade_type', 'Type', ['Buy'], { required: true }),
      dt('date', 'Trade date', { required: true }),
      dt('settlement_date', 'Settlement date', { optional: true, hint: 'Leave blank to auto-calculate (T+N business days, skipping weekends and holidays). An auto-calculated date is re-derived by the settlement-recompute job after you seed a missing holiday year; a date you enter is kept exactly as given.' }),
      fk('listing_id', 'Listing', 'listings', { required: true }),
      dec('average_price', 'Average price', { required: true, default: '' }),
      dec('quantity', 'Quantity', { required: true, default: '' }),
      fk('currency', 'Currency', 'currencies', { required: true, encode: 'string' }),
      dec('brokerage', 'Brokerage'),
      bool('brokerage_includes_gst', 'Brokerage includes GST', { hint: 'Tick when the statement quotes brokerage GST-inclusive; the GST component (1/11, rounded to the cent) is derived automatically.' }),
      dec('gst_on_brokerage', 'GST on brokerage'),
      fk('brokerage_currency', 'Brokerage currency', 'currencies', { required: true, encode: 'string' }),
      dec('fx_rate', 'Manual FX rate', { default: '1', hint: 'Foreign units per AUD; fallback used only when no ATO rate exists. 1 for AUD.' }),
      dec('spot_fx_rate', 'Spot FX rate override', { optional: true, default: '', hint: 'Optional deliberate transaction-date spot rate (foreign units per AUD): when set it wins over the monthly RBA rate everywhere this trade converts to AUD. Use for a one-off purchase/sale of a large foreign asset (QC 18020); leave blank for the monthly default. Non-AUD trades only.' }),
      txt('contract_note_ref', 'Contract note ref', { optional: true }),
      dec('statement_total', 'Statement total', { optional: true, default: '', hint: 'Optional cross-check in the brokerage currency: quantity × price + brokerage + GST. Rejected if it does not reconcile.' }),
      fk('holding_account_id', 'Holding account', 'holdingAccounts', { required: true, default: '1' }),
    ],
    columns: ['id', 'trade_type', 'origin', 'date', 'settlement_date', 'settlement_date_source', 'listing_id', 'average_price', 'quantity', 'currency', 'brokerage', 'statement_total', 'fx_rate', 'spot_fx_rate', 'holding_account_id'],
    listFilter: function (row) { return row.trade_type !== 'Sell'; },
    // Origin labels the operation that created the row (transfer-in, scrip
    // exchange, …) so a rollover Buy's cost-base-carrying brokerage figure
    // never reads as a real fee.
    deriveRow: function (row) { row.origin = tradeOrigin(row); },
    attachOwner: 'trade_id',
    wireForm: wireGstBrokerage,
  },
  {
    slug: 'sells', title: 'Sells', menu: 'Activity', api: '/sells', custom: 'sells',
    desc: 'Sell trades created atomically with their parcel allocations.',
  },
  {
    slug: 'transfers', title: 'Transfers', menu: 'Activity', api: '/transfers', custom: 'transfers',
    desc: 'Moves between holding accounts (e.g. vested plan shares to a personal account) — not a CGT event.',
  },
  {
    slug: 'inheritances', title: 'Inheritances', menu: 'Activity', api: '/inheritances',
    desc: 'Inherited parcels from a deceased estate — receiving them is not a CGT event. Recording one creates the parcel Buy at the date of death: the cost base per the chosen rule plus any legal-personal-representative (LPR) expenditure, with the 12-month discount clock per s 115-30. The parcel then flows through every report like a Buy; edit or delete it here (not under Trades) — refused while a sale or AMIT adjustment draws on it. The estate/LPR side (assets the executor sells) is not modelled.',
    keyFields: [int('id', 'ID', { auto: true })],
    fields: [
      sel('cost_base_rule', 'Cost base rule', ['DeceasedCostBase', 'MarketValueAtDeath'], { required: true }),
      fk('listing_id', 'Listing', 'listings', { required: true }),
      fk('holding_account_id', 'Holding account', 'holdingAccounts', { required: true, default: '1' }),
      dec('quantity', 'Units inherited', { required: true, default: '' }),
      dt('date_of_death', 'Date of death', { required: true }),
      dec('cost_base', 'Cost base', { required: true, default: '', hint: 'Your share of it: half the units carry half the deceased’s cost base. For a death on or after 21 September 1999, any indexation inside the deceased’s cost base must be recalculated out first (QC 66053).' }),
      dt('deceased_acquisition_date', 'Deceased’s acquisition date', { optional: true, default: '', hint: 'Starts the 12-month discount clock (s 115-30). Must be on or after 20 September 1985 — earlier means the asset was pre-CGT in their hands, so record it under Market value at death.' }),
      dec('lpr_expenditure', 'LPR expenditure', { optional: true, default: '', hint: 'Executor costs you can include — what the LPR incurred administering the estate: conveyancing on the transfer, legal costs of proving the will. Not anything billed before the death. Added to the parcel’s cost base; enter several as their total. AUD inheritances only — a foreign parcel converts at its acquisition month, which can predate the fee by decades.' }),
      dt('lpr_expenditure_date', 'LPR expenditure date', { optional: true, default: '', hint: 'When the LPR incurred it (on or after the death). Required with a non-zero expenditure.' }),
      fk('currency', 'Currency', 'currencies', { required: true, encode: 'string', default: 'AUD' }),
      dec('fx_rate', 'Manual FX rate', { default: '1', hint: 'Foreign units per AUD; fallback used only when no ATO rate exists for the month the cost base converts at — the deceased’s acquisition month, or the death for a pre-CGT asset. 1 for AUD; a non-AUD inheritance with no rate either way is refused rather than costed at parity.' }),
    ],
    typeField: 'cost_base_rule',
    fieldGroups: {
      DeceasedCostBase: ['deceased_acquisition_date'],
      MarketValueAtDeath: [],
    },
    typeDescs: {
      DeceasedCostBase: 'The deceased acquired the asset on or after 20 September 1985: your first-element cost base is the deceased’s cost base on the day they died, and the discount clock runs from the deceased’s acquisition date.',
      MarketValueAtDeath: 'The deceased acquired the asset before 20 September 1985 (pre-CGT in their hands): your first-element cost base is the asset’s market value on the day they died (you supply the valuation figure), and the discount clock runs from the date of death.',
    },
    typeLabels: {
      cost_base: {
        DeceasedCostBase: 'Deceased’s cost base at death',
        MarketValueAtDeath: 'Market value at death',
      },
    },
    columns: ['id', 'listing_id', 'holding_account_id', 'quantity', 'date_of_death', 'cost_base_rule', 'cost_base', 'lpr_expenditure', 'deceased_acquisition_date', 'currency'],
  },
  {
    slug: 'income', title: 'Income', menu: 'Activity', api: '/income',
    desc: 'Dividends and trust distributions. The form captures what the payment advice prints — amount, franking treatment, the per-share figures — and the advanced toggle reveals the full tax-component breakdown; a DRP statement’s reinvestment can be entered in the same form. A row can also record employment income (a dividend equivalent on unvested rights), which is reported as remuneration rather than as a dividend.',
    keyFields: [int('id', 'ID', { auto: true })],
    fields: [
      fk('listing_id', 'Listing', 'listings', { required: true }),
      dt('date_paid', 'Date paid', { required: true }),
      dec('amount_per_security', 'Amount per security', { optional: true, default: '', hint: 'Optional cross-check from the statement, supplied together with securities held: their product must equal the gross amount. Rejected if it does not reconcile.' }),
      dec('securities_held', 'Securities held', { optional: true, default: '' }),
      dt('ex_date', 'Ex date', { optional: true }),
      dec('franked_amount', 'Franked amount'),
      dec('unfranked_amount', 'Unfranked amount'),
      dec('foreign_source_income', 'Foreign source income'),
      dec('foreign_tax_paid', 'Foreign tax paid'),
      dec('tfn_withholding_tax', 'TFN withholding tax'),
      dec('franking_credits', 'Franking credits', { hint: 'Bounded by the franked amount above: a company can attach at most franked × 30/70 (its 30% tax rate; a base-rate entity’s 25% gives less), so a credit with no franked amount behind it, or above that maximum, is rejected — the usual cause is a transposed column or the wrong statement line. Trust distributions are exempt: the trust’s own deductions can reduce the franked component while the member still claims the full credit.' }),
      dec('lic_capital_gain_amount', 'LIC capital gain amount', { hint: 'A listed investment company’s dividend statement advises how much of the dividend is attributable to a LIC capital gain (the attributable part) — enter that figure as printed, not a share of it. An individual deducts 50% of it, and the Annual Tax Report and Tax Summary compute that halving for question D8 (Dividend deductions), so entering an already-halved figure claims half the deduction you are entitled to.' }),
      dec('conduit_foreign_income', 'Conduit foreign income', { hint: 'The part of the unfranked amount above that the payer declared to be conduit foreign income (CFI) — a memo figure, already included in that amount, not an extra payment. To an Australian resident an unfranked dividend declared to be CFI is assessable, so the unfranked amount must be the statement’s full figure with the CFI portion in it; a value larger than the unfranked amount is rejected.' }),
      bool('trust_income', 'Trust income'),
      sel('income_type', 'Income type', ['Dividend', 'EmploymentIncome', 'OtherIncome'], { default: 'Dividend', hint: 'Dividend = a payment of the holding (a dividend, trust distribution or buy-back dividend component) — what every row is unless you say otherwise. EmploymentIncome = a dividend equivalent paid on unvested rights, which is remuneration under s 6-5 and not a dividend in your hands (TD 2017/26): it reports on its own line, belongs at item 1/2 salary and wages (normally already prefilled from your employer’s STP reporting), and counts in no investment-income total. OtherIncome = ordinary income the holding produced but did not distribute — a crypto staking reward, or an airdrop of an established token, taken at the tokens’ market value on receipt (QC 69950); it reports at item 24, other income, which nothing prefills, so it does count in the assessable totals, and the tokens themselves are entered as a Buy at that same value. Both non-dividend kinds carry the cash as the unfranked amount and nothing else — franking, foreign-source, LIC, CFI, tax-deferred, ex/entitlement dates and the per-share figures are all rejected on such a row, and neither can be reinvested.' }),
      dt('entitlement_date', 'Entitlement date', { optional: true, hint: 'Trust distributions only: the date you became presently entitled — usually the distribution period’s end on the statement. Trust income is assessed in this date’s financial year even when the cash arrives later (a June distribution paid in July belongs to the year just ended). Leave empty to assess by the pay date.' }),
      dec('tax_deferred_amount', 'Tax-deferred amount', { optional: true, default: '', hint: 'Non-AMIT trust statements only: the statement’s tax-deferred amount — a CGT event E4 cost-base reduction. Recording it changes nothing by itself: enter the reduction as a Return of capital corporate action on the listing; the E4 cross-check report flags rows still missing one.' }),
      fk('currency', 'Currency', 'currencies', { required: true, encode: 'string', default: 'AUD' }),
      fk('holding_account_id', 'Holding account', 'holdingAccounts', { required: true, default: '1', hint: 'The account the distribution was paid to — decides whose DRP enrolment applies.' }),
    ],
    wireForm: wireIncomeEntry,
    columns: ['id', 'listing_id', 'date_paid', 'income_type', 'franked_amount', 'unfranked_amount', 'franking_credits', 'currency', 'holding_account_id', 'reinvestment_trade_id'],
    rowActions: function (row) {
      // Only a distribution is reinvestable — the API refuses any other kind
      // (422), so the action isn't offered for one.
      if (row.income_type && row.income_type !== 'Dividend') return [];
      return row.reinvestment_trade_id == null
        ? [{ label: 'Reinvest', href: '#/reinvest/' + row.id }]
        : [{
          label: 'Undo reinvest', del: '/income/' + row.id + '/reinvest',
          confirm: 'Undo this reinvestment? The linked DRP trade is deleted and the distribution can be reinvested again.',
        }];
    },
    attachOwner: 'income_id',
  },
  {
    slug: 'interest_income', title: 'Interest Income', menu: 'Activity', api: '/interest_income',
    desc: 'Interest income — bank, term-deposit, or broker-cash interest (it has no listing, so it isn’t an Income row). Enter the gross amount as the statement shows it, including any amount withheld. An Australian-source row reports as the year’s gross interest (question 10 label L, TFN withholding joining the withholding line, label M); a foreign-source row (e.g. a foreign broker’s cash or money-market sweep fund) reports as assessable foreign source income instead (question 20 label E, its foreign tax withheld joining the FITO line). Both count in gross assessable investment income.',
    keyFields: [int('id', 'ID', { auto: true })],
    fields: [
      dt('date_paid', 'Date credited', { required: true, hint: 'The date the interest was credited, received, or applied on your behalf — not the date the funds became reachable. A term deposit run to maturity is credited at maturity, so $500 credited 30 June and withdrawable 2 July is that June year’s interest. Sets the financial year the interest falls in and the ATO FX month for a non-AUD amount.' }),
      dec('amount', 'Gross amount', { required: true, hint: 'Include any amount withheld — the gross figure is declared; the withheld amount is entered below.' }),
      dec('tfn_withholding_tax', 'TFN withholding tax', { hint: 'Australian-source rows only — TFN amounts are withheld by Australian investment bodies.' }),
      bool('foreign_source', 'Foreign source', { hint: 'Tick for interest from a foreign payer (e.g. a US broker’s money-market fund): it reports at 20E assessable foreign source income, not 10L gross interest.' }),
      dec('foreign_tax_paid', 'Foreign tax paid', { hint: 'Foreign-source rows only — tax the foreign payer withheld; joins the FITO line (A$1,000 de-minimis applies).' }),
      fk('currency', 'Currency', 'currencies', { required: true, encode: 'string', default: 'AUD' }),
      txt('source', 'Source', { optional: true, default: '', hint: 'Where the interest came from, e.g. the bank account or term deposit.' }),
      fk('holding_account_id', 'Holding account', 'holdingAccounts', { optional: true, default: '', hint: 'Optional — leave blank for interest from outside the portfolio’s accounts (an ordinary bank account).' }),
    ],
    columns: ['id', 'date_paid', 'amount', 'tfn_withholding_tax', 'foreign_source', 'foreign_tax_paid', 'currency', 'source', 'holding_account_id'],
    attachOwner: 'interest_income_id',
  },
  {
    slug: 'investment_expenses', title: 'Investment Expenses', menu: 'Activity', api: '/investment_expenses',
    desc: 'Deductible investment expenses — the cost of earning assessable investment income: interest on money borrowed to buy income-producing shares, management/adviser fees, account-keeping fees, and subscriptions. Enter the amount as the deductible figure (post-apportionment — the portion you have determined is income-producing); the tax summary nets these against gross assessable investment income per financial year. Brokerage is not an expense here (it forms the CGT cost base on the trade) and the LIC capital gain deduction is its own income field. Each row is deducted in full in the financial year of its date, so an expense the ATO spreads across years — loan establishment fees and other borrowing costs over $100, or a prepayment covering more than 12 months — is entered as one row per year carrying that year’s apportioned share.',
    keyFields: [int('id', 'ID', { auto: true })],
    fields: [
      dt('date_incurred', 'Date incurred', { required: true, hint: 'Sets the financial year the deduction falls in and the ATO FX month for a non-AUD amount. One row is one year: an expense spread across years — borrowing expenses over $100 (5 years or the loan term, whichever is shorter) or a prepayment whose service period runs past 12 months (split by days) — is entered as one row per financial year carrying that year’s share.' }),
      sel('expense_type', 'Expense type', ['LoanInterest', 'ManagementFee', 'AdviceFee', 'AccountKeepingFee', 'Subscription', 'Other'], { required: true }),
      dec('amount', 'Deductible amount', { required: true, hint: 'Post-apportionment — the figure that goes on the return.' }),
      dec('gross_amount', 'Gross amount', { optional: true, default: '', hint: 'Optional provenance: the pre-apportionment expense. Supplied with the deductible %, gross × % must equal the deductible amount to the cent.' }),
      dec('deductible_percentage', 'Deductible %', { optional: true, default: '', hint: 'Optional provenance: the percentage you determined was deductible (0–100). Supplied with the gross amount, the two are cross-checked against the deductible amount.' }),
      fk('currency', 'Currency', 'currencies', { required: true, encode: 'string', default: 'AUD' }),
      txt('description', 'Description', { optional: true, default: '' }),
      fk('listing_id', 'Listing', 'listings', { optional: true, default: '', hint: 'Optional — leave blank for a portfolio-wide expense.' }),
      fk('holding_account_id', 'Holding account', 'holdingAccounts', { optional: true, default: '', hint: 'Optional — leave blank for a portfolio-wide expense.' }),
    ],
    columns: ['id', 'date_incurred', 'expense_type', 'amount', 'currency', 'description', 'listing_id', 'holding_account_id'],
  },
  {
    slug: 'amma_statements', title: 'AMMA Statements', menu: 'Activity', api: '/amma_statements',
    desc: 'Annual AMIT Member Annual statements.',
    keyFields: [int('id', 'ID', { auto: true })],
    fields: [
      fk('listing_id', 'Listing', 'listings', { required: true }),
      dt('tax_year_end_date', 'Tax year end', { required: true }),
      dec('units_held', 'Units held'),
      dt('date_received', 'Date received', { required: true }),
      dec('australian_interest', 'Australian interest'),
      dec('australian_dividends_unfranked', 'Australian dividends (unfranked)'),
      dec('franked_dividends', 'Franked dividends'),
      dec('franking_credits', 'Franking credits'),
      dec('net_rent', 'Net rent'),
      dec('foreign_income', 'Foreign income'),
      dec('foreign_tax_credits', 'Foreign tax credits', { hint: "Part C's foreign income tax offset on the statement's foreign INCOME — claimable in full." }),
      dec('foreign_tax_credits_capital_gains', 'Foreign tax credits (capital gains)', { hint: "Part C's foreign income tax offset applicable to the statement's CAPITAL GAINS, entered exactly as printed (trustees report it grossed up). The tax summary reduces it for the CGT discount before claiming it — the ATO requires that apportionment and the trustee does not do it for you. Leave 0 unless Part C separates this line." }),
      dec('other_income', 'Other income'),
      dec('cgt_discount_gains', 'CGT discount gains'),
      dec('cgt_indexation_gains', 'CGT indexation gains'),
      dec('cgt_other_gains', 'CGT other gains'),
      dec('capital_losses_applied', 'Capital losses applied'),
      dec('tax_deferred_amount', 'Tax-deferred amount'),
      dec('tax_free_amount', 'Tax-free amount'),
      dec('cost_base_adjustment', 'Cost base adjustment (per unit)'),
      dec('tfn_withholding_tax', 'TFN withholding tax'),
      fk('currency', 'Currency', 'currencies', { required: true, encode: 'string', default: 'AUD' }),
      fk('holding_account_id', 'Holding account', 'holdingAccounts', { required: true, default: '1', hint: 'The registry issues one statement per holder account.' }),
    ],
    wireForm: wireAmmaEntry,
    columns: ['id', 'listing_id', 'tax_year_end_date', 'units_held', 'cost_base_adjustment', 'currency', 'holding_account_id'],
    rowActions: function (row) {
      return [{ label: 'Generate adjustments', href: '#/generate-adjustments/' + row.id }];
    },
    attachOwner: 'amma_statement_id',
  },
  {
    slug: 'ess_statements', title: 'ESS Statements', menu: 'Activity', api: '/ess_statements',
    desc: 'Employee share scheme statements: the assessable discount on ESS interests (declared at Item 12 in the year of the taxing point), split by scheme type. The taxed-upfront-eligible discount is reduced by up to $1,000 per year in the tax summary (the ≤$180,000 income test is your responsibility). Use the row’s Vest action to create the cost-base-reset Buy for the vested shares — once vested the action is replaced by the linked Buy in the Vest trade column (delete the statement to redo it).',
    keyFields: [int('id', 'ID', { auto: true })],
    fields: [
      fk('listing_id', 'Listing', 'listings', { required: true }),
      fk('holding_account_id', 'Holding account', 'holdingAccounts', { required: true, default: '1', hint: 'The account the ESS interests vest into.' }),
      dt('taxing_point_date', 'Taxing point date', { required: true, hint: 'The deferred taxing point (or acquisition date for a taxed-upfront scheme): sets the assessable year and the vest Buy’s acquisition date. Must be on or after 20 September 1985 — the vest Buy is dated here, and a pre-CGT parcel is outside CGT and not modelled. 30-day rule: if you sell within 30 days after the taxing point, the taxing point becomes the sale date — the employer must issue an amended statement, and you enter it over this one (taxing point = the sale date, market value = what the sale realised) rather than as a second row.' }),
      dec('quantity', 'Quantity vested', { default: '0', hint: 'Shares that vest — drives the cost-base-reset Buy. Leave at 0 for an income-only statement (a discount with no vest recorded against it).' }),
      dec('market_value_per_share', 'Market value per share', { default: '0', hint: 'At the taxing point; the vest Buy’s price (the reset cost base). Quantity × this value is the ceiling on the discount labels below: the discount is market value less what you paid, so at most it equals the market value (an RSU acquired for nil consideration).' }),
      dec('taxed_upfront_eligible', 'Taxed-upfront eligible discount (D)', { hint: 'Discount from taxed-upfront schemes eligible for the $1,000 reduction.' }),
      dec('taxed_upfront_not_eligible', 'Taxed-upfront not-eligible discount (E)'),
      dec('deferral_discount', 'Deferral-scheme discount (F)', { hint: 'The RSU case.' }),
      dec('pre_2009_cessation_discount', 'Pre-2009 cessation discount (G)'),
      dec('foreign_source_discount', 'Foreign-source discount (A)', { hint: 'The foreign-sourced portion of the above discounts (already included in them, never added on top); for the foreign income tax offset. It cannot exceed D + E + F + G — a memo is part of what it memos.' }),
      dec('tfn_withholding', 'TFN amounts withheld (C)', { hint: 'Amounts the employer withheld from the discount. Positive, like every figure on this form — a negative one is refused.' }),
      fk('currency', 'Currency', 'currencies', { required: true, encode: 'string', default: 'AUD', hint: 'Must be the listing’s own currency: the per-share market value and the listed price are the same money, so a statement in another currency is rejected (convert it before entry, or pick the right listing).' }),
      dec('fx_rate', 'FX rate (foreign per AUD)', { optional: true, default: '', hint: 'Non-AUD statements only: the rate you are using for this statement (AUD = foreign ÷ rate), e.g. the release-date rate on the employer’s statement. It is a fallback — an imported RBA monthly rate for the taxing point’s month still wins — but without it, and without that month’s RBA rate, vesting is refused rather than costing the parcel at parity.' }),
      dec('aud_taxed_upfront_eligible', 'Statement AUD: taxed-upfront eligible (D)', { optional: true, default: '', hint: 'The employer statement’s AUD figure for this label (converted at the release-date spot rate — what the ATO prefill carries). When set, the tax summary reports it verbatim instead of RBA-converting the foreign amount. Non-AUD statements only.' }),
      dec('aud_taxed_upfront_not_eligible', 'Statement AUD: taxed-upfront not eligible (E)', { optional: true, default: '' }),
      dec('aud_deferral_discount', 'Statement AUD: deferral discount (F)', { optional: true, default: '', hint: 'The RSU case: the annual Employee share scheme statement’s label F figure.' }),
      dec('aud_pre_2009_cessation_discount', 'Statement AUD: pre-2009 cessation (G)', { optional: true, default: '' }),
      dec('aud_foreign_source_discount', 'Statement AUD: foreign-source (A)', { optional: true, default: '' }),
    ],
    columns: ['id', 'listing_id', 'holding_account_id', 'taxing_point_date', 'quantity', 'market_value_per_share', 'deferral_discount', 'aud_deferral_discount', 'taxed_upfront_eligible', 'currency', 'fx_rate', 'vest_trade_id'],
    // Vest only while unvested — a second vest is rejected by the API (422),
    // so a vested row (vest_trade_id set) shows the linked Buy instead.
    rowActions: function (row) {
      return row.vest_trade_id == null ? [{ label: 'Vest', href: '#/ess-vest/' + row.id }] : [];
    },
    attachOwner: 'ess_statement_id',
  },
  {
    slug: 'amit_adjustments', title: 'AMIT Adjustments', menu: 'Activity', api: '/amit_adjustments',
    desc: 'Links a purchase parcel (Buy/DRP trade) to an AMMA statement. Quantity is in the parcel’s as-acquired units, exactly as the trade records them; where a share split or bonus issue falls between the parcel’s acquisition and the statement’s year end it is re-based into that year’s units before the statement’s per-unit cost base adjustment is applied, so enter the fund’s figure as stated and scale nothing by hand.',
    keyFields: [int('id', 'ID', { auto: true })],
    fields: [
      fk('amma_statement_id', 'AMMA statement', 'amma', { required: true }),
      fk('trade_id', 'Trade (Buy/DRP)', 'buyParcels', { required: true }),
      dec('quantity', 'Quantity', { required: true, default: '' }),
    ],
    columns: ['id', 'amma_statement_id', 'trade_id', 'quantity'],
  },
  {
    slug: 'parcel_allocations', title: 'Parcel Allocations', menu: 'Activity', api: '/parcel_allocations', readonly: true,
    desc: 'Sell→purchase parcel links (read-only; managed via Sells).',
    columns: ['id', 'sale_trade_id', 'purchase_trade_id', 'quantity_allocated'],
  },
  {
    slug: 'rights_sales', title: 'Rights Sales', menu: 'Activity', api: '/rights_sales', deleteOnly: true,
    desc: 'Disposals of renounceable rights — sold on-market, lapsed, or compensated by a retail premium — recorded via a rights issue row’s Sell rights action (Corporate Actions). Each is a CGT event on the rights themselves, not the shares: the holding is untouched, free rights have a nil cost base (purchased rights carry their cost), and the gain/loss is anchored to the original parcels’ acquisition dates in the realised-gains and net-capital-gain reports. Rows are immutable — delete (freeing the entitlement) and re-enter to amend.',
    keyFields: [int('id', 'ID', { auto: true })],
    fields: [],
    columns: ['id', 'rights_action_id', 'date', 'units', 'proceeds_per_right', 'rights_cost', 'fx_rate', 'holding_account_id'],
  },
  {
    slug: 'drp_enrolments', title: 'DRP Enrolments', menu: 'Activity', api: '/drp_enrolments',
    desc: 'Dated DRP enrolment periods per (listing, holding account) — the same listing may be enrolled in one account and not another (blank unenrolment date = currently enrolled). Periods within an account must not overlap; unenrolling pays out the trailing carried residual.',
    keyFields: [int('id', 'ID', { auto: true })],
    fields: [
      fk('listing_id', 'Listing', 'listings', { required: true }),
      fk('holding_account_id', 'Holding account', 'holdingAccounts', { required: true, default: '1' }),
      dt('enrolment_date', 'Enrolment date', { required: true }),
      dt('unenrolment_date', 'Unenrolment date', { optional: true, hint: 'Leave blank while enrolled. Distributions with an ex date on or after this no longer reinvest.' }),
      sel('residual_handling', 'Residual handling', ['CarryForward', 'PayOut'], { required: true }),
    ],
    columns: ['id', 'listing_id', 'holding_account_id', 'enrolment_date', 'unenrolment_date', 'residual_handling'],
  },
  {
    slug: 'corporate_actions', title: 'Corporate Actions', menu: 'Activity', api: '/corporate_actions',
    desc: 'Capital events against a listing: return of capital, share splits/consolidations, bonus issues, rights issues, off-market buy-backs, scrip-for-scrip takeovers, demergers, and worthless/delisted shares. The form shows only the chosen action type’s fields; rights issues, buy-backs, scrip-for-scrip takeovers, demergers, and worthless shares are executed after recording via the row’s Exercise / Participate / Exchange / Demerge / Recognise action.',
    attachOwner: 'corporate_action_id',
    keyFields: [int('id', 'ID', { auto: true })],
    fields: [
      sel('action_type', 'Action type', ['ReturnOfCapital', 'ShareSplit', 'BonusIssue', 'RightsIssue', 'BuyBack', 'ScripForScrip', 'Demerger', 'WorthlessShares'], { required: true }),
      fk('listing_id', 'Listing', 'listings', { required: true }),
      dt('date', 'Date', { required: true }),
      dec('amount_per_unit', 'Amount per unit', { optional: true, default: '' }),
      fk('currency', 'Currency', 'currencies', { optional: true, encode: 'string', default: '', hint: 'Currency of the per-unit amount(s).' }),
      dt('record_date', 'Record date', { optional: true, hint: 'When entitlement to the payment was fixed. Parcels bought on or after it are ex-entitlement and are not reduced. Blank falls back to the payment date.' }),
      dec('split_new_units', 'Split: new units', { optional: true, default: '' }),
      dec('split_old_units', 'Split: old units', { optional: true, default: '' }),
      dec('bonus_units', 'Bonus: units issued', { optional: true, default: '' }),
      dec('bonus_held_units', 'Bonus: per units held', { optional: true, default: '' }),
      dec('rights_units', 'Rights: new units', { optional: true, default: '' }),
      dec('rights_held_units', 'Rights: per units held', { optional: true, default: '' }),
      dec('exercise_price', 'Rights: exercise price per unit', { optional: true, default: '' }),
      dec('buyback_price', 'Buy-back: price per unit', { optional: true, default: '' }),
      dec('buyback_dividend', 'Buy-back: dividend per unit', { optional: true, default: '', hint: '0 (or blank) when the price has no dividend component.' }),
      dec('buyback_franking_credit', 'Buy-back: franking credit per unit', { optional: true, default: '', hint: 'Needs a dividend.' }),
      dec('buyback_market_value', 'Buy-back: market value per unit', { optional: true, default: '', hint: 'Had the buy-back not been proposed. Blank if the price is at or above it.' }),
      fk('scrip_listing_id', 'Scrip: replacement listing', 'listings', { optional: true, default: '', hint: 'Must differ from the listing being taken over.' }),
      dec('scrip_new_units', 'Scrip: new units', { optional: true, default: '' }),
      dec('scrip_old_units', 'Scrip: per old units', { optional: true, default: '' }),
      dec('scrip_cash_per_unit', 'Scrip: cash per old unit', { optional: true, default: '', hint: 'Blank for an all-scrip exchange. With cash (a partial rollover), also give the market value and currency.' }),
      dec('scrip_market_value', 'Scrip: market value per new unit', { optional: true, default: '', hint: 'Value of one replacement share just after issue — apportions the cost base between cash and scrip.' }),
      fk('scrip_cash_currency', 'Scrip: cash currency', 'currencies', { optional: true, encode: 'string', default: '', hint: 'Currency of the cash and market value.' }),
      fk('demerger_listing_id', 'Demerger: demerged listing', 'listings', { optional: true, default: '', hint: 'Must differ from the head listing.' }),
      dec('demerger_new_units', 'Demerger: new units', { optional: true, default: '' }),
      dec('demerger_held_units', 'Demerger: per units held', { optional: true, default: '' }),
      dec('demerger_cost_base_pct', 'Demerger: cost base % to demerged entity', { optional: true, default: '', hint: 'The head-entity-advised percentage (0–100 exclusive), e.g. 5.063.' }),
      dt('demerger_close_date', 'Demerger: last pre-demerger trading day', { optional: true, hint: 'Leave the four “stated close” fields blank together, or fill all four. Must be before the demerger date.' }),
      dec('demerger_close_price', 'Demerger: actual close that day', { optional: true, default: '', hint: 'What the security really traded at, in the listing’s quote currency — the price provider restates its whole pre-demerger series for the spin-off, and this is what re-bases it back.' }),
      txt('demerger_close_sourced_from', 'Demerger: close sourced from', { optional: true, hint: 'Where the figure came from, e.g. “nyse.com daily close, retrieved 2026-08-20”.' }),
      txt('demerger_close_reason', 'Demerger: why it is stated', { optional: true, hint: 'Why the close had to be stated, e.g. “Yahoo adjusts the pre-demerger series by the spin-off factor”.' }),
      sel('worthless_event', 'Worthless: CGT event', ['G3Declaration', 'C2Cancellation'], { optional: true, default: '', hint: 'G3 = liquidator/administrator declaration; C2 = deregistration/cancellation.' }),
    ],
    // The form renders only the selected action_type's field group (plus the
    // common fields above that appear in no group); the unchosen groups'
    // fields submit as null, exactly as their blank inputs used to. The
    // matching typeDescs entry scopes the form's description to the type.
    typeField: 'action_type',
    fieldGroups: {
      ReturnOfCapital: ['amount_per_unit', 'currency', 'record_date'],
      ShareSplit: ['split_new_units', 'split_old_units'],
      BonusIssue: ['bonus_units', 'bonus_held_units'],
      RightsIssue: ['rights_units', 'rights_held_units', 'exercise_price', 'currency'],
      BuyBack: ['buyback_price', 'buyback_dividend', 'buyback_franking_credit', 'buyback_market_value', 'currency'],
      ScripForScrip: ['scrip_listing_id', 'scrip_new_units', 'scrip_old_units', 'scrip_cash_per_unit', 'scrip_market_value', 'scrip_cash_currency'],
      Demerger: ['demerger_listing_id', 'demerger_new_units', 'demerger_held_units', 'demerger_cost_base_pct', 'demerger_close_date', 'demerger_close_price', 'demerger_close_sourced_from', 'demerger_close_reason'],
      WorthlessShares: ['worthless_event'],
    },
    typeDescs: {
      ReturnOfCapital: 'Return-of-capital payment (CGT event G1): the per-unit amount reduces the cost base of the parcels entitled to it and still held on the payment date — entitlement is fixed at the record date, so parcels bought on or after it are untouched (leave the record date blank and the payment date decides instead); any excess over a parcel’s cost base is a capital gain in the Net Capital Gain report.',
      ShareSplit: 'Share split/consolidation (TD 2000/10): on the conversion date every “old units” become “new units” (2-for-1 split: new 2, old 1; 1-for-10 consolidation: new 1, old 10) — no CGT event, the parcels keep their total cost base and original acquisition date.',
      BonusIssue: 'Bonus issue (non-assessable): on the issue date every “held units” receive “bonus units” extra units (1-for-10 issue: bonus 1, held 10) — no CGT event, the cost base is apportioned over original + bonus shares and the acquisition date is preserved; bonus shares chosen in lieu of a dividend are a DRP trade, not entered here.',
      RightsIssue: 'Rights issue: units held before the record date earn “rights units” per “held units” at the exercise price (1-for-4 issue: rights 1, held 4) — recording the issue changes nothing; use the row’s Exercise action to create the new Buy parcel (acquired at the exercise date, cost base = exercise payment + any amount paid for the rights), or its Sell rights action to dispose of rights instead — sold, lapsed, or paid out as a retail premium (a CGT event on the rights themselves, anchored to the original parcels’ acquisition dates).',
      BuyBack: 'Off-market buy-back: record the per-unit buy-back price, the dividend component of that price and its franking credit (both 0 for a listed-company buy-back announced after 25 Oct 2022), and the market value had the buy-back not been proposed (blank if the price is at or above it); recording changes nothing — use the row’s Participate action to sell units into the buy-back, which creates the Sell at the capital proceeds (max(price, market value) − dividend) plus the dividend income row.',
      ScripForScrip: 'Scrip-for-scrip takeover (with rollover): on the exchange date every “old units” of this listing become “new units” of the replacement listing (1-for-1 merger: new 1, old 1), plus optionally cash per old unit (a partial rollover — also give the replacement share’s market value just after issue and the currency) — recording changes nothing; use the row’s Exchange action to substitute every open parcel: the scrip side’s gain is disregarded and each replacement parcel carries the consumed parcel’s remaining cost base (its market-value share when there is cash) and acquisition date (the combined period counts toward the 12-month discount), while the cash side is a capital gain assessed now in the realised-gains and net-capital-gain reports.',
      Demerger: 'Demerger (eligible, rollover chosen): on the demerger date every “held units” of this (head) listing receive “new units” of the demerged listing (BHP Steel’s 1-for-5: new 1, held 5), and the advised percentage of each parcel’s cost base moves to the new interests — recording changes nothing; use the row’s Demerge action to apportion every open parcel: any gain is disregarded, the head parcels keep the rest of the cost base and their acquisition dates, and the new parcels’ 12-month discount clock runs from the original acquisition. Separately, state what the security actually closed at on the last pre-demerger trading day: the price provider restates the whole pre-demerger series by its spin-off factor (a demerger moves no unit count here, so there is no ratio to read), and this stated close — divided by the provider’s own figure for that same day — is what re-bases the stored closing prices back into their own days. Leave it blank if no pre-demerger prices were fetched after the demerger; the Health report names any demerger that needs it.',
      WorthlessShares: 'Worthless / delisted shares (CGT events G3 and C2): a capital loss on a failed company without a sale — choose G3Declaration (a liquidator/administrator declared the shares worthless) or C2Cancellation (the company was deregistered). Recording changes nothing; use the row’s Recognise action to close every open parcel at nil proceeds: each parcel’s remaining reduced cost base becomes a capital loss (never income, never discounted) that flows through the realised-gains and net-capital-gain reports.',
    },
    // Per-type label for the common date field (generic 'Date' until a type
    // is chosen).
    typeLabels: {
      date: {
        ReturnOfCapital: 'Payment date',
        ShareSplit: 'Conversion date',
        BonusIssue: 'Issue date',
        RightsIssue: 'Record date',
        BuyBack: 'Buy-back date',
        ScripForScrip: 'Exchange date',
        Demerger: 'Demerger date',
        WorthlessShares: 'Event date',
      },
    },
    columns: ['id', 'action_type', 'listing_id', 'date', 'amount_per_unit', 'currency', 'record_date', 'split_new_units', 'split_old_units', 'bonus_units', 'bonus_held_units', 'rights_units', 'rights_held_units', 'exercise_price', 'buyback_price', 'buyback_dividend', 'buyback_franking_credit', 'buyback_market_value', 'scrip_listing_id', 'scrip_new_units', 'scrip_old_units', 'scrip_cash_per_unit', 'scrip_market_value', 'scrip_cash_currency', 'demerger_listing_id', 'demerger_new_units', 'demerger_held_units', 'demerger_cost_base_pct', 'demerger_close_date', 'demerger_close_price', 'demerger_close_sourced_from', 'demerger_close_reason', 'worthless_event'],
    rowActions: function (row) {
      if (row.action_type === 'RightsIssue') {
        return [
          { label: 'Exercise', href: '#/exercise/' + row.id },
          { label: 'Sell rights', href: '#/sell-rights/' + row.id },
        ];
      }
      if (row.action_type === 'BuyBack') return [{ label: 'Participate', href: '#/participate/' + row.id }];
      if (row.action_type === 'ScripForScrip') return [{ label: 'Exchange', href: '#/scrip-exchange/' + row.id }];
      if (row.action_type === 'Demerger') return [{ label: 'Demerge', href: '#/demerge/' + row.id }];
      if (row.action_type === 'WorthlessShares') return [{ label: 'Recognise', href: '#/recognise/' + row.id }];
      return [];
    },
  },
  {
    slug: 'cgt_settings', title: 'CGT Settings', menu: 'Activity', api: '/cgt_settings',
    desc: 'Opening carried-forward capital loss (pre-system loss years), applied as the starting balance in the Net Capital Gain report.',
    keyFields: [int('id', 'ID', { required: true, default: '1', hint: 'Singleton — always 1.' })],
    fields: [dec('opening_capital_loss', 'Opening capital loss carried forward', { required: true })],
    columns: ['id', 'opening_capital_loss'],
  },
  {
    slug: 'tax_year_settings', title: 'Tax Year Settings', menu: 'Activity', api: '/tax_year_settings',
    desc: 'Taxpayer facts answered year by year, rather than once. Only years that differ from the default need a row at all — an empty list means every year takes the default, which is what the system assumed before this screen existed.',
    keyFields: [int('tax_year', 'Financial year', { required: true, hint: 'By the calendar year of its 30 June end — FY2025/26 is 2026. From 1986 (CGT starts 20 September 1985).' })],
    fields: [
      bool('ess_taxed_upfront_reduction_eligible', '$1,000 ESS reduction: eligible', {
        default: true,
        hint: 'The $1,000 taxed-upfront ESS reduction applies only where your adjusted taxable income for the year was $180,000 or less — a test over income this tool cannot see, so it is recorded here. Untick to have the Tax Summary and the Annual Tax Report show the year’s taxed-upfront discount unreduced. Ticked (or no row at all) applies the reduction as before.',
      }),
    ],
    columns: ['tax_year', 'ess_taxed_upfront_reduction_eligible'],
  },
];

// Menu/section placement for the Reports mega-menu: which top-level menu an
// entry appears under (shared with ENTITIES' `menu` field — Reports, unlike
// Activity/Reference Data/Jobs, also groups into titled columns via
// `section`, since it holds far more entries) — see nav.js's navModel.
export const REPORTS = [
  {
    slug: 'overview', title: 'Portfolio Overview', api: '/portfolio/overview', method: 'POST', prices: true, performancePanel: true,
    menu: 'Reports', section: 'Portfolio',
    desc: 'Open holdings per listing and holding account, with optional market value, a market-value graph over a selectable date range, and a period performance summary (capital growth / FX movement / income).',
    // Shortcut buttons for the most common data-entry paths, shown above the
    // performance panel — this is the app's home screen (#/).
    shortcuts: [
      { label: '+ New trade', href: '#/e/trades/new', primary: true },
      { label: '+ New income', href: '#/e/income/new' },
      { label: '+ New sell', href: '#/sells/new' },
      { label: '+ New transfer', href: '#/transfers/new' },
    ],
  },
  { slug: 'open-parcels', title: 'Open Parcels', api: '/portfolio/open-parcels', method: 'GET', menu: 'Reports', section: 'Portfolio', desc: 'Every open parcel: acquisition date, original cost base, AMIT and return-of-capital reductions, remaining quantity and adjusted cost base (AUD).' },
  {
    slug: 'attachments', title: 'Attachments', api: '/reports/attachments', method: 'GET',
    menu: 'Reports', section: 'Portfolio',
    desc: 'Every stored document — the file, the activity it is attached to, and that activity’s listing. Download saves the file; View opens it in a new tab; Record opens the owning activity’s own attachments view, where a file can be deleted or another uploaded.',
    columns: ['id', 'filename', 'content_type', 'byte_size', 'uploaded_at', 'owner_type', 'listing_id', 'owner_description'],
    rowActions: function (row) {
      return [
        { label: 'Download', href: apiUrl('/attachments/' + row.id + '/content'), newTab: true },
        { label: 'View', href: apiUrl('/attachments/' + row.id + '/content?disposition=inline'), newTab: true },
        { label: 'Record', href: '#/attachments/' + row.owner_field + '/' + row.owner_id },
      ];
    },
  },
  {
    slug: 'activity', title: 'Listing Activity', api: '/portfolio/activity', method: 'POST',
    menu: 'Reports', section: 'Portfolio',
    desc: 'Everything ever recorded against one listing, in date order — trades labelled with the operation that created them, transfers, income, corporate actions, statements — with a running units-held balance, ending in the final holding summary per account (units held, cost base, market value).',
    params: [
      fk('listing_id', 'Listing', 'listings', { required: true }),
      dec('price', 'Current price per unit (AUD)', { optional: true, default: '', hint: 'Blank = the live price from the price source; a fetch failure leaves the summary unvalued (never zero).' }),
    ],
    tables: [
      { key: 'events', title: 'Activity' },
      { key: 'holdings', title: 'Holding summary' },
    ],
  },
  {
    slug: 'parcel-optimiser', title: 'Parcel Optimiser', api: '/portfolio/parcel-optimiser', method: 'POST',
    menu: 'Reports', section: 'Decision support',
    desc: 'Candidate parcel selections for a contemplated sale — which parcels a sale comes from is your choice, and it changes the tax outcome. Each strategy shows its per-parcel allocations and the resulting gross gain / discountable split. Nothing is recorded: enter the chosen allocations on the real Sell.',
    params: [
      fk('listing_id', 'Listing', 'listings', { required: true }),
      fk('holding_account_id', 'Holding account', 'holdingAccounts', { required: true, default: '1', hint: 'The account the Sell would happen in — a Sell may only consume its own account’s parcels.' }),
      dec('units', 'Units to sell', { required: true, default: '' }),
      dt('sale_date', 'Sale date', { optional: true, hint: 'Blank = today. Drives the 12-month discount clock, and the candidates are the parcels open on this date — the ones a Sell dated then could allocate.' }),
      dec('price', 'Price per unit (AUD)', { optional: true, default: '', hint: 'Blank = the live price from the price source.' }),
    ],
    tables: [
      {
        key: 'strategies', title: 'Strategies',
        // Each strategy row expands to its own per-parcel allocations (the
        // sibling `allocations` array, matched back by `strategy`) instead of
        // a separate flat table alongside it.
        expand: {
          from: 'allocations', matchOn: 'strategy',
          columns: ['purchase_trade_id', 'acquisition_date', 'units', 'cost_base', 'proceeds', 'capital_gain_loss', 'discount_eligible'],
        },
      },
    ],
  },
  { slug: 'unrealised-gains', title: 'Unrealised Gains', api: '/portfolio/unrealised-gains', method: 'POST', prices: true, asOfDate: true, menu: 'Reports', section: 'CGT & tax', desc: 'Per-holding (listing × holding account) unrealised gain/loss vs cost base.' },
  {
    slug: 'realised-gains', title: 'Realised Gains', api: '/portfolio/realised-gains', method: 'GET',
    menu: 'Reports', section: 'CGT & tax',
    desc: 'Per-disposal capital gain/loss split into CGT buckets — ordinary sales plus rights sales/lapses (source column). Expand a disposal for the individual parcels sold and each one’s own CGT outcome.',
    expand: {
      key: 'parcels',
      columns: ['purchase_trade_id', 'acquisition_date', 'units', 'cost_base', 'proceeds', 'capital_gain_loss', 'discount_eligible'],
    },
  },
  { slug: 'performance', title: 'Performance', api: '/portfolio/performance', method: 'POST', prices: true, asOfDate: true, menu: 'Reports', section: 'Portfolio', desc: 'Investment performance per holding and overall: total return, money-weighted return (% p.a.), trailing-12-month income yield.' },
  {
    slug: 'net-capital-gain', title: 'Net Capital Gain', api: '/portfolio/net-capital-gain', method: 'GET', export: true,
    menu: 'Reports', section: 'CGT & tax',
    desc: 'Assessable net capital gain per financial year. Expand a year for its realised disposals, and a disposal for its per-parcel breakdown.',
    expand: {
      key: 'disposals',
      columns: ['source', 'sale_trade_id', 'listing_id', 'sale_date', 'proceeds', 'cost_base', 'capital_gain_loss', 'discount_eligible_gain', 'non_discountable_gain', 'capital_loss'],
      expand: {
        key: 'parcels',
        columns: ['purchase_trade_id', 'acquisition_date', 'units', 'cost_base', 'proceeds', 'capital_gain_loss', 'discount_eligible'],
      },
    },
  },
  {
    slug: 'net-capital-gain-what-if', title: 'Pre-Sale What-If', api: '/portfolio/net-capital-gain/what-if', method: 'POST',
    menu: 'Reports', section: 'Decision support',
    desc: 'Dry-run a hypothetical disposal through the Net Capital Gain report: the disposal year’s figures with and without it, using a Parcel Optimiser strategy to pick the parcels (the API also accepts explicit allocations). Nothing is written, and the whole-of-income tax estimate is out of scope — this is the CGT-side delta only.',
    params: [
      fk('listing_id', 'Listing', 'listings', { required: true }),
      fk('holding_account_id', 'Holding account', 'holdingAccounts', { optional: true, default: '', hint: 'Blank = parcels from any account.' }),
      dec('units', 'Units to sell', { required: true, default: '' }),
      dec('proceeds', 'Total proceeds (AUD)', { required: true, default: '' }),
      dt('date', 'Sale date', { required: true, hint: 'The parcels drawn on are the ones open on this date — the ones a Sell dated then could allocate.' }),
      sel('strategy', 'Parcel-selection strategy', [
        { value: 'fifo', label: 'FIFO (oldest first)' },
        { value: 'min_gain', label: 'Minimise current-year gain' },
        { value: 'max_discount', label: 'Maximise discount-eligible proportion' },
        { value: 'harvest_losses', label: 'Harvest losses first' },
      ], { required: true, default: 'min_gain', hint: 'How the sold units are drawn from the open parcels — compare the candidates on the Parcel Optimiser screen.' }),
    ],
    tables: [
      {
        key: 'years', title: 'The year, without and with the disposal',
        // Explicit column list: `ScenarioYear` flattens in `NetCapitalGainYear`,
        // whose `disposals` drilldown is always empty here (it belongs to the
        // main report, not this hypothetical dry-run) — excluded rather than
        // shown as a permanently-blank column.
        columns: [
          'scenario', 'tax_year', 'discount_eligible_gains', 'other_gains', 'capital_losses',
          'capital_loss_brought_forward', 'net_discount_eligible_gain', 'net_other_gain',
          'cgt_discount', 'net_capital_gain', 'capital_loss_carried_forward',
          'cgt_event_e10_gain', 'cgt_event_g1_gain', 'cgt_event_c2_gain', 'taxpayer_basis',
        ],
      },
      {
        key: 'hypothetical', title: 'Hypothetical disposal',
        // The single hypothetical disposal's per-parcel allocations (the
        // sibling `allocations` array; `matchOn: null` = every row belongs to
        // this one disposal) expand inline instead of a separate flat table.
        expand: {
          from: 'allocations', matchOn: null,
          columns: ['purchase_trade_id', 'acquisition_date', 'units', 'cost_base', 'proceeds', 'capital_gain_loss', 'discount_eligible'],
        },
      },
    ],
  },
  { slug: 'tax-summary', title: 'Tax Summary', api: '/portfolio/tax-summary', method: 'GET', export: true, menu: 'Reports', section: 'CGT & tax', desc: 'Income aggregated by Australian financial year. Investment-expense deductions are cut two ways over the same total: by kind of expense (loan interest, management fee, \u2026) and by the question each is claimed at \u2014 13Y for expenses of earning a trust or AMIT distribution (interest on money borrowed to buy the units included), 20M for expenses of earning foreign-source income, D15 for a debt deduction against foreign income (question 20\u2019s worksheet excludes those), and D7/D8 for the ordinary Australian interest and dividend case. An expense attributed to no listing cannot be routed and is reported at D7/D8.' },
  {
    slug: 'tax-report', title: 'Annual Tax Report', custom: 'tax-report', api: '/reports/tax-report',
    menu: 'Reports', section: 'CGT & tax',
    desc: 'A printable, archivable tax document for one financial year — trading gains/losses with a full cost-base breakdown, the ATO gain/loss worksheet, income by category, and a data-completeness check. Print or Save as PDF to archive it.',
  },
  { slug: 'exchange-mic-validation', title: 'Exchange MIC Validation', api: '/reports/exchange_mic_validation', method: 'GET', statusField: 'registry_status', menu: 'Reports', section: 'Cross-checks & alerts', desc: 'Curated exchanges checked against the ISO MIC registry.' },
  { slug: 'fx-coverage', title: 'FX Coverage', api: '/reports/fx_coverage', method: 'GET', statusField: 'kind', menu: 'Reports', section: 'Cross-checks & alerts', desc: 'Amounts whose ATO monthly rate has not been imported (and what each currently converts at), plus the two documented FX simplifications where they actually bite: a settlement window crossing a rate month (CGT event K10/K11) and a cost-base reduction converted at the parcel acquisition month.' },
  { slug: 'settlement-holiday-coverage', title: 'Settlement Holiday Coverage', api: '/reports/settlement_holiday_coverage', method: 'GET', statusField: 'coverage_status', menu: 'Reports', section: 'Cross-checks & alerts', desc: 'Settlement dates worth a second look, on two independent questions: the trade\u2019s settlement window falls outside the seeded exchange-holiday calendars (so the date was computed skipping weekends only), or the stored settlement date is not a trading day on the listing\u2019s own calendar \u2014 a hand-entered weekend or public holiday. A row can carry both; neither is refused at write time, so a flagged trade stays editable.' },
  { slug: 'e4-cross-check', title: 'Tax-Deferred E4 Cross-Check', api: '/reports/e4_cross_check', method: 'GET', menu: 'Reports', section: 'Cross-checks & alerts', desc: 'Trust income rows whose statement reported a tax-deferred amount (a CGT event E4 cost-base reduction) with no Return of capital corporate action on the listing in the same financial year — enter the action to clear a row.' },
  { slug: 'amit-adjustment-cross-check', title: 'AMIT Adjustment Cross-Check', api: '/reports/amit_adjustment_cross_check', method: 'GET', menu: 'Reports', section: 'Cross-checks & alerts', desc: 'AMMA statements whose per-parcel AMIT adjustments don’t reconcile to the statement: none entered at all, adjusted units outside the band the year allows — the units held at its end, plus the units disposed of during it, so a statement for the year a holding was sold reconciles rather than reading as excess coverage (split-aware) — the same parcel adjusted twice, or a parcel that cannot have been held in the statement’s year. A missed parcel overstates its cost base; a duplicated one over-reduces it, and CGT event E10’s nil floor can turn that into a capital gain that was never made. Generate adjustments from the statement’s row to clear a row.' },
  { slug: 'rollover-consistency', title: 'Rollover Consistency', api: '/reports/rollover_consistency', method: 'GET', menu: 'Reports', section: 'Cross-checks & alerts', desc: 'Transfers, scrip-for-scrip exchanges and demergers whose stored figures no longer match today’s facts. Each of those operations writes the cost base (and units) its replacement parcels carry as a stored value, computed when it ran — so editing a source parcel, or correcting an AMMA statement behind it, moves the parcels the reports still walk while the frozen replacement figures stay put, and the same holding reports a different cost base depending on the order things were entered. Each row names what the operation carried, what the units it consumed are worth now, and the difference; the fix is to delete that operation and run it again. A partial-rollover scrip exchange (one with a cash component) is listed as not checked, because how much of each cost base went to the cash side is the exchange’s own apportionment. An empty report means every rollover still reconciles.' },
  { slug: 'amit-cash-cross-check', title: 'AMIT Cash Cross-Check', api: '/reports/amit_cash_cross_check', method: 'GET', menu: 'Reports', section: 'Cross-checks & alerts', desc: 'Financial years with AMIT cash distribution rows but no AMMA statement covering the year, per holding account — a registry issues one statement per holder account, so a fund held in two accounts needs two. AMIT cash rows fund DRP reinvestment only — the AMMA attribution is the assessable record the Tax Summary reports — so a missing AMMA would silently drop the year’s income from the return. Enter that account’s AMMA statement to clear a row; an AMMA year with no cash rows is fine.' },
  {
    slug: 'wash-sales', title: 'Wash Sales', api: '/reports/wash_sales', method: 'POST',
    menu: 'Reports', section: 'Decision support',
    desc: 'Loss-realising Sells with a Buy of the same listing within the window either side, across all holding accounts — the sell-and-repurchase pattern the ATO warns may have the loss cancelled under Part IVA (TR 2008/1). Advisory only: nothing is rejected and the loss still counts in every CGT report; whether a flag matters depends on the facts (a market-driven repurchase days later survived the ATO’s own example).',
    params: [
      int('window_days', 'Window (days either side)', { default: '30', hint: 'TR 2008/1 has no statutory window — 24 hours apart has failed and 3 days apart has passed. 30 days is a review convention; blank = 30.' }),
    ],
  },
  { slug: 'franking-at-risk', title: 'Franking At-Risk', api: '/reports/franking_at_risk', method: 'GET', statusField: 'status', menu: 'Reports', section: 'Cross-checks & alerts', desc: 'Each dividend whose shares fail the 45-day (90 for preference) at-risk holding-period walk: the failing qualification window, the entitled and disqualified units, and the credits denied — or shielded by the year’s under-$5,000 small-shareholder exemption. Denied rows are exactly what the Tax Summary subtracts as franking_credits_denied. A row marked untested_no_ex_date is a dividend the rule could not be applied to at all — no ex date (or trust entitlement date) was recorded to anchor the window — so record that date to resolve it; with none of these, every attached credit is claimable on the tests modelled here. The other two qualified-person conditions — the 30%-at-risk test (hedges, options, futures) and the related payments rule, which the small-shareholder exemption does not excuse — are not modelled and cannot be recorded, so an empty report assumes the holdings are unhedged and under no related-payment obligation.' },
  {
    slug: 'franking-what-if', title: 'Franking Sale What-If', api: '/reports/franking_at_risk/what-if', method: 'POST', statusField: 'status',
    menu: 'Reports', section: 'Decision support',
    desc: 'Before recording a Sell: which dividends’ franking credits the contemplated sale would put at risk under the 45-day rule. Each row shows the additional credits at stake and the qualification window end — selling after that date cannot disqualify the dividend. Nothing is written.',
    params: [
      fk('listing_id', 'Listing', 'listings', { required: true }),
      dt('sale_date', 'Contemplated sale date', { required: true }),
      dec('units', 'Units to sell', { required: true, default: '' }),
    ],
  },
  { slug: 'snapshots', title: 'Snapshots', custom: 'snapshots', api: '/report_snapshots', menu: 'Jobs', desc: 'Stored daily results of the price-dependent reports (portfolio overview, unrealised gains, performance), valued at the stored closing prices. A back-dated fact marks affected snapshots stale; regenerate them here. The market-value graph is on the Portfolio Overview screen.' },
  {
    slug: 'row-history', title: 'Row History', api: '/reports/row_history', method: 'POST',
    menu: 'Jobs',
    desc: 'The append-only audit trail: every past version of one record, newest first. Database triggers capture the prior values whenever an audited row is edited or deleted, so an accidental change to a historical fact can be noticed and reconstructed; entries are kept forever and nothing can rewrite them. No entries = the row has never been changed since the trail began.',
    params: [
      // Must list exactly the audited tables (reports::row_history::AUDITED_TABLES;
      // a web.rs test pins this select's options to that const).
      sel('table', 'Table', [
        'trades', 'parcel_allocations', 'income', 'interest_income',
        'amma_statements', 'amit_adjustments', 'ess_statements', 'transfers',
        'corporate_actions', 'inheritances', 'rights_sales',
        'rights_sale_allocations', 'investment_expenses', 'drp_enrolments',
        'cgt_settings', 'attachments', 'listings', 'listing_renames',
        'closing_prices', 'tax_year_settings', 'rba_fx_rates',
        'exchange_holidays',
      ], { required: true, default: 'trades' }),
      int('row_id', 'Row ID', { required: true, hint: "The record's id as shown in its entity list — for tax_year_settings, the financial year itself (e.g. 2026)." }),
    ],
  },
];

// ---- post-action configuration -----------------------------------------
// Follow-up actions reached from an owning row (a listing's Rename, a
// distribution's Reinvest, a corporate action's Exercise / Participate /
// Exchange / Demerge). Each is one config entry — owner fetch, fields,
// optional parcel allocations, POST endpoint, texts — rendered by the
// generic viewAction, mirroring how ENTITIES drives viewEntityForm. The
// confirm-only actions (scrip exchange, demerge) are the degenerate config
// with `fields: []`.
export const ACTIONS = [
  // Ticker / exchange change. A renamed security is the same security, so
  // this is a dated, audited event on the listing — not a field edit: PUT
  // refuses a ticker or exchange_mic change on a listing with any recorded
  // trades, income or prices, and its 422 names this very endpoint. The
  // recorded chain, and the undo, are the Rename history view
  // (#/renames/:id) the listing row also links to.
  {
    slug: 'rename', nav: 'listings', ownerApi: '/listings', cancel: '#/e/listings', submit: 'Rename',
    post: function (id) { return '/listings/' + id + '/rename'; },
    title: function (id, owner, listing) { return 'Rename listing ' + listing(id) + ' (#' + id + ')'; },
    desc: function (l, listing) {
      return 'Records a dated ticker or exchange change for ' + listing(l.id) + ' \u2014 the same security keeps its id, so every trade, distribution, parcel, cost base and acquisition date stays attached. '
        + 'The chain stores what was overwritten, read from the listing\u2019s own row, so the newest entry can be undone from Rename history. '
        + 'The request must change the ticker or the exchange (a no-op is refused), the effective date must be after the newest recorded rename and not after today (record an announced change on the day it takes effect \u2014 there is no pending state), '
        + 'and the new ticker must not already be held by another listing. A move to an exchange quoting a different currency is refused (a redenomination is a new listing plus a Transfer), and a Crypto listing takes no exchange and needs a recognised digital-token ticker. '
        + 'A relisting under a new entity after a takeover is not a rename \u2014 record that as a ScripForScrip corporate action.';
    },
    fields: function (l) {
      return [
        dt('effective_date', 'Effective date', { required: true, hint: 'The day the change took effect: after the newest recorded rename and not after today. Prices before it are still collected under the old symbol.' }),
        txt('ticker', 'New ticker', { required: true, hint: 'Currently ' + l.ticker + '. Enter it unchanged if only the exchange is moving.' }),
        fk('exchange_mic', 'New exchange', 'exchanges', { optional: true, encode: 'string', hint: 'Blank keeps the current exchange (' + (l.exchange_mic || 'none \u2014 Crypto') + '). Must quote the listing\u2019s own currency (' + l.currency + ').' }),
        txt('name', 'New name', { optional: true, hint: 'Blank keeps the current name (' + l.name + ').' }),
        txt('price_symbol', 'New price symbol', { optional: true, hint: 'Blank leaves the provider-symbol override exactly as it was (' + (l.price_symbol || 'none') + ') \u2014 it is not part of the rename chain, and an override that matched the old ticker rarely matches the new one.' }),
        txt('note', 'Note', { optional: true, hint: 'Optional: why the security was renamed (a company notice, a redomicile).' }),
      ];
    },
    toast: function (r) {
      const at = function (mic) { return mic ? mic + ':' : 'Crypto:'; };
      return 'Recorded rename #' + r.id + ': ' + at(r.old_exchange_mic) + r.old_ticker
        + ' \u2192 ' + at(r.new_exchange_mic) + r.new_ticker + ', effective ' + r.effective_date
        + '. Undo it from the listing\u2019s Rename history.';
    },
  },
  // DRP reinvestment: creates a DRP trade and links it to the distribution.
  {
    slug: 'reinvest', nav: 'income', ownerApi: '/income', cancel: '#/e/income', submit: 'Reinvest',
    post: function (id) { return '/income/' + id + '/reinvest'; },
    title: function (id, owner, listing) { return 'Reinvest ' + listing(owner.listing_id) + ' distribution #' + id; },
    desc: function (income, listing) { return 'Creates a DRP trade for ' + listing(income.listing_id) + ' and links it back to this distribution. The holding must be DRP-enrolled.'; },
    fields: function (income) {
      return [
        dec('reinvestment_price', 'Reinvestment price', { required: true, default: '' }),
        dec('units', 'Units allotted (as stated)', { default: '', hint: 'Leave blank to compute the units and the residual from the cash (a whole-share registry DRP). Or enter the statement’s exact figure — taken verbatim, cross-checked against the reinvestable cash. Stated to decimals (a broker’s fractional allotment) it spends the whole distribution and leaves no residual; stated as whole units, whatever the cash did not buy is carried or paid out like any leftover.' }),
        dec('fx_rate', 'FX rate', { default: '1', hint: 'Optional; defaults to 1.' }),
        dt('date', 'Trade date', { optional: true, hint: 'Optional; defaults to the distribution pay date (' + income.date_paid + ').' }),
      ];
    },
    toast: function (trade, listing) { return trade ? 'Reinvested into ' + describeTrade(trade, listing) + ' (trade #' + trade.id + ').' : 'Reinvested.'; },
  },
  // Rights exercise: creates the new Buy parcel (acquired at the exercise
  // date); the server caps cumulative exercised units at the entitlement.
  {
    slug: 'exercise', nav: 'corporate_actions', ownerApi: '/corporate_actions', cancel: '#/e/corporate_actions', submit: 'Exercise',
    post: function (id) { return '/corporate_actions/' + id + '/exercise'; },
    title: function (id, owner, listing) { return 'Exercise ' + listing(owner.listing_id) + ' rights issue #' + id; },
    desc: function (a, listing) { return 'Creates a Buy trade for ' + listing(a.listing_id) + ' at the exercise price (' + a.exercise_price + ' ' + a.currency + ' per unit): ' + a.rights_units + ' new unit(s) per ' + a.rights_held_units + ' held at the record date.'; },
    fields: function (a) {
      return [
        dt('date', 'Exercise date', { required: true, hint: 'The new parcel’s acquisition date; on or after the record date (' + a.date + ').' }),
        dec('units', 'Units acquired', { required: true, default: '' }),
        dec('rights_cost', 'Amount paid for the rights', { default: '0', hint: 'Total, in ' + a.currency + '. 0 for rights issued free.' }),
        dec('fx_rate', 'FX rate', { default: '1', hint: 'Optional; defaults to 1.' }),
        fk('holding_account_id', 'Holding account', 'holdingAccounts', { required: true, default: '1', hint: 'Where the exercised parcel lands.' }),
      ];
    },
    toast: function (trade, listing) { return trade ? 'Exercised into ' + describeTrade(trade, listing) + ' (trade #' + trade.id + ').' : 'Exercised.'; },
  },
  // Rights sale/lapse: disposes of the rights themselves — the share
  // holding is untouched — anchored to the original parcels whose
  // record-date units earned the rights (their acquisition dates drive the
  // 12-month CGT discount). The server shares the entitlement cap with the
  // Exercise action and caps each parcel's anchoring at what it earned.
  {
    slug: 'sell-rights', nav: 'corporate_actions', ownerApi: '/corporate_actions', cancel: '#/e/corporate_actions', submit: 'Sell rights',
    post: function (id) { return '/corporate_actions/' + id + '/sell_rights'; },
    title: function (id, owner, listing) { return 'Sell ' + listing(owner.listing_id) + ' rights from issue #' + id; },
    desc: function (a, listing) { return 'Records a disposal of the rights themselves — sold on-market, lapsed, or compensated by a retail premium under this renounceable offer (enter the premium as the proceeds per right). The ' + listing(a.listing_id) + ' holding is untouched. Free rights have a nil cost base and take each anchoring parcel’s acquisition date for the 12-month discount; rights you paid for carry that cost instead, so nil proceeds (a lapse) realise a capital loss. Together with exercises, sales may not exceed the record-date entitlement. Undo by deleting the row under Rights Sales.'; },
    fields: function (a) {
      return [
        dt('date', 'Sale / lapse date', { required: true, hint: 'On or after the record date (' + a.date + ').' }),
        dec('units', 'Rights sold or lapsed', { required: true, default: '' }),
        dec('proceeds_per_right', 'Proceeds per right', { default: '0', hint: 'In ' + a.currency + '. 0 for a lapse; a retail premium is entered per right.' }),
        dec('rights_cost', 'Amount paid for the rights', { default: '0', hint: 'Total, in ' + a.currency + '. 0 for rights issued free.' }),
        dec('fx_rate', 'FX rate', { default: '1', hint: 'Optional; defaults to 1.' }),
        fk('holding_account_id', 'Holding account', 'holdingAccounts', { required: true, default: '1', hint: 'The account the disposal is reported under.' }),
      ];
    },
    allocations: {
      heading: 'Anchoring parcels', parcelLabel: 'Original parcel', qtyLabel: 'Rights anchored',
      addLabel: '+ Add parcel', qtyField: 'units',
      hint: 'Which original parcels earned the sold rights — each anchors its rights to that parcel’s acquisition date for the CGT discount; no parcel units are consumed. Must sum exactly to the rights sold; each parcel is capped at the entitlement its record-date units earned.',
      // No remaining-quantity or holding-account constraint here — a parcel
      // fully sold since still earned rights it held at the record date. The
      // only things that make it a candidate: the action's own listing, and
      // having existed before the record date (the action's own date).
      filter: { source: 'buy', beforeOwnerDate: true },
    },
    toast: function (sale) { return sale ? 'Recorded rights sale #' + sale.id + ' (' + sale.units + ' right(s)).' : 'Recorded rights sale.'; },
  },
  // Buy-back participation: atomically creates the Sell at the capital
  // proceeds per unit (max(price, market value) − dividend) with the chosen
  // parcel allocations, plus the dividend-component income row if any.
  {
    slug: 'participate', nav: 'corporate_actions', ownerApi: '/corporate_actions', cancel: '#/e/corporate_actions', submit: 'Participate',
    post: function (id) { return '/corporate_actions/' + id + '/participate'; },
    title: function (id, owner, listing) { return 'Participate in ' + listing(owner.listing_id) + ' buy-back #' + id; },
    desc: function (a, listing) { return 'Creates a Sell trade for ' + listing(a.listing_id) + ' at the capital proceeds per unit — max(price ' + a.buyback_price + ', market value ' + (a.buyback_market_value || a.buyback_price) + ') − dividend ' + a.buyback_dividend + ' ' + a.currency + ' — plus the dividend-component income row when there is one.'; },
    fields: function (a) {
      return [
        dt('date', 'Participation date', { required: true, hint: 'The CGT event (acceptance) date; on or after the buy-back date (' + a.date + '). Also the dividend component’s pay date.' }),
        dec('units', 'Units sold into the buy-back', { required: true, default: '' }),
        dec('fx_rate', 'FX rate', { default: '1', hint: 'Optional; defaults to 1.' }),
        fk('holding_account_id', 'Holding account', 'holdingAccounts', { required: true, default: '1', hint: 'The participating account: allocations may only consume its parcels.' }),
      ];
    },
    allocations: {
      hint: 'Allocations must sum exactly to the units sold. Each parcel must be a Buy/DRP with enough remaining units.',
      // Same two things the server itself requires: the action's own
      // listing, and the chosen holding account (its parcels only).
      filter: { source: 'open', accountField: 'holding_account_id' },
    },
    toast: function (r, listing) {
      const t = r && r.trade;
      return 'Sold into the buy-back: ' + describeTrade(t, listing) + (t ? ' (trade #' + t.id + ')' : '')
        + (r && r.income ? ', plus dividend income for ' + listing(r.income.listing_id) + ' (income #' + r.income.id + ')' : '') + '.';
    },
  },
  // Scrip-for-scrip exchange (confirm-only): POST takes no parameters — the
  // action's terms and the holdings at its date determine everything. It
  // atomically closes every open parcel of the original listing (the
  // rollover disregards the gain; with a cash component the cash side's
  // apportioned gain is assessed now) and creates the replacement parcels
  // carrying each consumed parcel's remaining rolled-over cost base and
  // acquisition date.
  {
    slug: 'scrip-exchange', nav: 'corporate_actions', ownerApi: '/corporate_actions', cancel: '#/e/corporate_actions', submit: 'Exchange',
    post: function (id) { return '/corporate_actions/' + id + '/exchange'; },
    title: function (id, owner, listing) { return 'Exchange ' + listing(owner.listing_id) + ' scrip-for-scrip takeover #' + id; },
    desc: function (a, listing) {
      const cash = a.scrip_cash_per_unit
        ? ' Plus ' + a.scrip_cash_per_unit + ' ' + a.scrip_cash_currency + ' cash per old unit (partial rollover): the cash side’s market-value share of each parcel’s cost base is assessed as a capital gain now, and only the scrip side’s share rolls over.'
        : ' The rollover disregards the capital gain.';
      return 'Substitutes every open parcel of ' + listing(a.listing_id) + ' held at ' + a.date + ' with ' + a.scrip_new_units + ' unit(s) of ' + listing(a.scrip_listing_id) + ' per ' + a.scrip_old_units + ' held.' + cash + ' Each replacement parcel carries its consumed parcel’s remaining (rolled-over) cost base and acquisition date (the combined holding period counts toward the 12-month discount). Undo by deleting the closing Sell from the Sells view.';
    },
    fields: [],
    toast: function (r, listing, a) {
      const n = r && r.replacements ? r.replacements.length : 0;
      return 'Exchanged ' + listing(a.listing_id) + ' into ' + n + ' parcel(s) of ' + listing(a.scrip_listing_id)
        + ' (closing sell #' + (r && r.sell ? r.sell.id : '?') + ').';
    },
  },
  // Demerger (confirm-only): POST takes no parameters. It atomically closes
  // every open parcel of the head listing (the rollover disregards any gain)
  // and recreates each as a head replacement parcel plus a demerged-entity
  // parcel splitting the cost base by the advised percentage, both keeping
  // the consumed parcel's acquisition date.
  {
    slug: 'demerge', nav: 'corporate_actions', ownerApi: '/corporate_actions', cancel: '#/e/corporate_actions', submit: 'Demerge',
    post: function (id) { return '/corporate_actions/' + id + '/demerge'; },
    title: function (id, owner, listing) { return 'Demerge ' + listing(owner.listing_id) + ' #' + id; },
    desc: function (a, listing) { return 'Apportions every open parcel of head listing ' + listing(a.listing_id) + ' held at ' + a.date + ': ' + a.demerger_cost_base_pct + '% of each parcel’s cost base moves to ' + a.demerger_new_units + ' unit(s) of demerged listing ' + listing(a.demerger_listing_id) + ' per ' + a.demerger_held_units + ' held; the head parcels keep the rest. Any gain is disregarded and both sides keep the original acquisition date (the 12-month discount clock). Undo by deleting the closing Sell from the Sells view.'; },
    fields: [],
    toast: function (r, listing, a) {
      const n = r && r.demerged_replacements ? r.demerged_replacements.length : 0;
      return 'Demerged ' + listing(a.listing_id) + ' into ' + n + ' parcel(s) of ' + listing(a.demerger_listing_id)
        + ' (closing sell #' + (r && r.sell ? r.sell.id : '?') + ').';
    },
  },
  // Worthless-shares recognise (confirm-only): POST takes no parameters. It
  // atomically closes every open parcel of the listing at nil proceeds,
  // recognising each parcel's remaining reduced cost base as a capital loss
  // (unlike the rollover closing Sells, this one reaches the gains reports).
  {
    slug: 'recognise', nav: 'corporate_actions', ownerApi: '/corporate_actions', cancel: '#/e/corporate_actions', submit: 'Recognise',
    post: function (id) { return '/corporate_actions/' + id + '/recognise'; },
    title: function (id, owner, listing) { return 'Recognise worthless ' + listing(owner.listing_id) + ' #' + id; },
    desc: function (a, listing) { return 'Closes every open parcel of ' + listing(a.listing_id) + ' held at ' + a.date + ' through a single Sell at nil proceeds (' + a.worthless_event + '). Each parcel’s remaining reduced cost base becomes a capital loss — never income, never discounted — that flows through the realised-gains and net-capital-gain reports. Undo by deleting the closing Sell from the Sells view, which restores the holding.'; },
    fields: [],
    toast: function (r, listing, a) {
      return 'Recognised worthless ' + listing(a.listing_id) + ' as a capital loss'
        + ' (closing sell #' + (r && r.sell ? r.sell.id : '?') + ').';
    },
  },
  // AMIT adjustment generation: derives one amit_adjustment per parcel open
  // at the statement's tax year end, in its own listing and holding account.
  // The standing counterpart to the AMMA form's chain-after-save tick — this
  // is the path for generating later, or re-running with `replace` after
  // correcting a missed trade (a missing parcel usually means a trade was
  // entered after the statement). `confirm` previews the set and its total
  // against the statement's units held before anything is written.
  {
    slug: 'generate-adjustments', nav: 'amma_statements', ownerApi: '/amma_statements', cancel: '#/e/amma_statements', submit: 'Generate',
    post: function (id) { return '/amma_statements/' + id + '/generate_adjustments'; },
    title: function (id, owner, listing) { return 'Generate AMIT adjustments for ' + listing(owner.listing_id) + ' statement #' + id; },
    desc: function (s, listing) { return 'Creates one AMIT adjustment per ' + listing(s.listing_id) + ' parcel held in this statement’s holding account at ' + s.tax_year_end_date + ', so the statement’s ' + s.cost_base_adjustment + ' per-unit cost base adjustment reaches every affected parcel. You confirm the parcels and their total against the statement’s units held (' + s.units_held + ') before anything is written; a mismatch is surfaced but never blocks, and the AMIT Adjustment Cross-Check keeps the statement flagged until it is resolved. A holding sold before the year end has no parcels to generate from — enter that statement’s rows by hand under AMIT Adjustments, against the parcels the units came from — and one transferred, exchanged or demerged away before it is entered by hand against the replacement parcels that now hold those units, which is accepted because the units trace back to this account. A move that happened *after* the year end needs no workaround: generation follows it and writes the row against the replacement parcel itself. Undo by deleting the rows under AMIT Adjustments.'; },
    fields: [
      bool('replace', 'Replace existing adjustments', { hint: 'Required when the statement already has adjustments: they are deleted and regenerated in one transaction.' }),
    ],
    confirm: confirmGeneratedAdjustments,
    toast: function (r) {
      return 'Generated ' + r.created.length + ' AMIT adjustment(s) covering ' + r.units_adjusted
        + ' unit(s) against ' + r.units_held + ' units held.';
    },
  },
  // ESS vest (confirm-only): POST takes no parameters — the statement's
  // quantity, per-share market value, and taxing-point date determine the
  // cost-base-reset Buy. The discount (income side) is already on the
  // statement; this creates the CGT parcel and links it back.
  {
    slug: 'ess-vest', nav: 'ess_statements', ownerApi: '/ess_statements', cancel: '#/e/ess_statements', submit: 'Vest',
    post: function (id) { return '/ess_statements/' + id + '/vest'; },
    title: function (id, owner, listing) { return 'Vest ' + listing(owner.listing_id) + ' ESS statement #' + id; },
    desc: function (s, listing) { return 'Creates the cost-base-reset Buy for ' + s.quantity + ' share(s) of ' + listing(s.listing_id) + ' at the taxing-point market value (' + s.market_value_per_share + ' ' + s.currency + ' per share), acquired ' + s.taxing_point_date + '. Undo by deleting this ESS statement, which removes the vest Buy.'; },
    fields: [],
    toast: function (trade, listing) { return trade ? 'Vested into ' + describeTrade(trade, listing) + ' (trade #' + trade.id + ').' : 'Vested.'; },
  },
];
