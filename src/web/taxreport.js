//
// Annual Tax Report — a printable, per-year tax document (`custom:
// 'tax-report'` in config.js, dispatched from app.js's router). Pick a
// financial year and Generate; nothing runs until the button is pressed
// (unlike the price-valuation reports, which auto-run on load — this report
// is meant to be produced deliberately and archived). A Print / Save as PDF
// button calls window.print() over the new @media print rules in style.css.
//
// Deliberately NOT built on the shared filterableTable/dataTable machinery:
// this is a print document, not a data grid — the 50-row pager would
// silently print only the first page, and per-column filter inputs/sort
// indicators have no business on an archived document. Every table here is
// plain semantic HTML, formatted through the same numericDisplay/columnLabel
// helpers every other screen uses, so figures read identically to the rest
// of the app.
import {
  el, toast, setMain, api, numericDisplay, moneyText, cellText, fmtLocalTimestamp, columnLabel,
} from './util.js';
import { setActiveNav } from './nav.js';

// `moneyText` (util.js) is the prose form of the rounding `moneyEl`/`moneyTd`
// apply below — a plain string for the subtotal/total lines and the alert
// messages, which have no cell to hang a per-figure tooltip on. Never use
// cellText directly on a money amount: the underlying Decimal is
// exact-arithmetic, so a subtotal or a halved discount routinely carries
// three or more decimal places (e.g. "592.33850") that read as noise once
// printed.
function moneyEl(value) {
  const nd = numericDisplay(value, 'money');
  return el('span', { title: nd ? nd.tip : null }, nd ? nd.text : cellText(value));
}
function moneyTd(value, extraClass) {
  return el('td', { class: ['num', extraClass].filter(Boolean).join(' ') }, moneyEl(value));
}
// A cell holding one indivisible token — an ISO date, a quantity, a price —
// must never be broken across lines. The print rules let cells wrap so a wide
// table compresses onto the page, and both a hyphen and a decimal point are
// break opportunities, so without this a date splits as "2022-06" / "-28" and a
// price as "102.77" / "34" — a misread waiting to happen in a document whose
// whole purpose is hand-checking figures against source statements. (Money
// figures come from moneyTd, which marks them `num`; the CSS spares both.)
const ATOMIC_CELL = /^(\d{4}-\d{2}-\d{2}|-?[\d,]+(\.\d+)?)$/;
function td(value) {
  const text = cellText(value);
  return el('td', ATOMIC_CELL.test(text) ? { class: 'atomic' } : null, text);
}
function doc(headers, rows) {
  return el('table', { class: 'doc-table' }, [
    el('thead', null, el('tr', null, headers.map(function (h) { return el('th', null, h); }))),
    el('tbody', null, rows),
  ]);
}
function fyLabel(taxYear) {
  return 'FY' + (taxYear - 1) + '/' + String(taxYear).slice(-2);
}

// ---- completeness -------------------------------------------------------

// Units held at 30 June with nothing recorded for the fund-year at all. This
// is a question rather than a gap — an AMIT that attributed nothing owes no
// statement, so there would be nothing to enter to clear it — and it is kept
// out of the pass/fail badge for exactly that reason.
function nothingRecordedList(c) {
  const unrecorded = c.amma_nothing_recorded || [];
  if (!unrecorded.length) return null;
  return el('div', null, [
    el('p', { class: 'badge note' }, 'Worth checking — held across 30 June with no distribution recorded for the year. If the fund distributed nothing, there is nothing to enter and no statement to expect:'),
    el('ul', null, unrecorded.map(function (a) {
      return el('li', null, a.ticker + ' in account #' + a.holding_account_id + ': ' + a.units_held
        + ' units held at 30 June, but no distribution and no AMMA statement recorded for the year.');
    })),
  ]);
}

function completenessSection(c) {
  if (c.complete) {
    return el('div', { class: 'doc-section' }, [
      el('h3', null, 'Data completeness'),
      el('p', { class: 'badge ok' }, '✓ Verified — every AMIT fund known to have attributed for this year has a covering AMMA statement, and no cross-check gaps were found.'),
      nothingRecordedList(c),
    ].filter(Boolean));
  }
  const items = [];
  // Both AMMA-coverage checks are per holding account (a registry issues one
  // statement per holder account), so each line names the account it is
  // asking for — two accounts of one fund otherwise print as two identical
  // sentences.
  c.amma_missing.forEach(function (a) {
    items.push(el('li', null, 'No AMMA statement for ' + a.ticker + ' in account #' + a.holding_account_id
      + ' — the fund attributed for this year, so this account is owed one.'));
  });
  c.amit_cash_alerts.forEach(function (a) {
    items.push(el('li', null, a.ticker + ' (account #' + a.holding_account_id + '): ' + a.cash_rows
      + ' cash distribution row(s) totalling ' + moneyText(a.cash_total_aud) + ' AUD with no covering AMMA statement.'));
  });
  c.e4_alerts.forEach(function (a) {
    items.push(el('li', null, a.ticker + ' (' + a.date_paid + '): tax-deferred amount ' + moneyText(a.tax_deferred_amount) + ' recorded with no matching Return of Capital action this year.'));
  });
  // The per-parcel AMIT adjustment set: a gap here distorts the disposal
  // schedule's cost base, this document's central figure, so each statement's
  // own problems are printed rather than a bare "doesn't reconcile".
  c.amit_adjustment_alerts.forEach(function (a) {
    items.push(el('li', null, a.ticker + ' AMMA statement #' + a.amma_statement_id + ' (' + a.parcel_count
      + ' parcel adjustment(s) covering ' + a.units_adjusted + ' of ' + a.units_held + ' units held): '
      + a.problems.join(' ')));
  });
  // A rollover whose stored carried cost base has gone stale is the cost base
  // of every unit still descending from it, so these are listed whatever year
  // the operation itself was in.
  (c.rollover_alerts || []).forEach(function (a) {
    items.push(el('li', null, a.ticker + ' ' + a.kind + ' #' + a.group_id + ' (' + a.date + '): '
      + a.problems.join(' ')));
  });
  return el('div', { class: 'doc-section' }, [
    el('h3', null, 'Data completeness'),
    el('p', { class: 'badge warn' }, '⚠ Issues found for this year — this report may understate income or the cost base until they are resolved:'),
    el('ul', null, items),
    nothingRecordedList(c),
  ].filter(Boolean));
}

// ---- disposals ------------------------------------------------------------

function adjustmentRow(a) {
  return el('tr', { class: 'adjustment-row' }, [
    td(''), td(a.date), el('td', { colspan: 3 }, (a.capped ? '⚠ ' : '') + a.reference), moneyTd(a.amount), td(''), td(''), td(''), td(''), td(''), td(''), td(''),
  ]);
}

function parcelRows(p) {
  const rows = [
    el('tr', null, [
      td(p.acquisition_date), td(p.acquisition_method + (p.trade_date && p.trade_date !== p.acquisition_date ? ' (traded ' + p.trade_date + ')' : '')),
      td(cellText(p.units)), td(p.buy_price != null ? cellText(p.buy_price) : '—'),
      moneyTd(p.initial_cost_base_aud), moneyTd(p.adjusted_cost_base_aud, 'bold'),
      td(p.sale_date), td(p.sale_price != null ? cellText(p.sale_price) : '—'),
      moneyTd(p.proceeds_aud), moneyTd(p.gain_loss_aud),
      td(p.discount_eligible ? 'yes (' + p.days_held + 'd)' : 'no (' + p.days_held + 'd)'),
      moneyTd(p.gain_after_discount_aud, 'bold'),
    ]),
  ];
  p.adjustments.forEach(function (a) { rows.push(adjustmentRow(a)); });
  const notes = [];
  // The other lawful method, named on the archived document rather than only
  // in a report the reader may never open. Nothing printed above changes: the
  // 50% discount is applied throughout (SCENARIOS AA-a).
  if (p.indexation_eligible && p.indexed_cost_base_aud != null) {
    notes.push('indexation was available on this parcel — indexed cost base '
      + moneyText(p.indexed_cost_base_aud) + ' AUD against the adjusted cost base shown; '
      + 'the 50% discount is applied here (see the Indexation Cross-Check report)');
  }
  if (p.buy_contract_note_ref) notes.push('buy note ' + p.buy_contract_note_ref);
  if (p.sale_contract_note_ref) notes.push('sale note ' + p.sale_contract_note_ref);
  if (p.currency !== 'AUD') {
    // Each rate is the one its own side's AUD figure was converted at — the
    // month's ATO rate, a deliberate spot override, or the record's fallback
    // where the month has none — so the printed arithmetic reconciles.
    notes.push(p.currency + ' — buy-side rate ' + (p.buy_month_fx_rate != null ? cellText(p.buy_month_fx_rate) : '?')
      + ', sale-side rate ' + (p.sell_month_fx_rate != null ? cellText(p.sell_month_fx_rate) : '?'));
  }
  if (notes.length) {
    rows.push(el('tr', { class: 'note-row' }, el('td', { colspan: 12 }, notes.join(' · '))));
  }
  return rows;
}

const DISPOSAL_HEADERS = [
  'Acquired', 'Method', 'Units', 'Buy price', 'Initial cost base (AUD)', 'Adjusted cost base (AUD)',
  'Sold', 'Sale price', 'Proceeds (AUD)', 'Gain / loss (AUD)', 'Discount eligible', 'Gain after discount (AUD)',
];

function disposalsSection(d) {
  if (d.listings.length === 0) {
    return el('div', { class: 'doc-section' }, [
      el('h3', null, 'Trading activity — gains and losses'),
      el('p', null, 'No disposals recorded this year.'),
    ]);
  }
  const groups = d.listings.map(function (g) {
    const rows = [];
    g.parcels.forEach(function (p) { rows.push.apply(rows, parcelRows(p)); });
    return el('div', { class: 'listing-group' }, [
      el('h4', null, g.ticker + ' — ' + g.listing_name),
      doc(DISPOSAL_HEADERS, rows),
      el('p', { class: 'subtotal' }, 'Subtotal: proceeds ' + moneyText(g.subtotal.proceeds_aud)
        + ', cost base ' + moneyText(g.subtotal.cost_base_aud)
        + ', gain/loss ' + moneyText(g.subtotal.gain_loss_aud)
        + ', gain after discount ' + moneyText(g.subtotal.gain_after_discount_aud)),
    ]);
  });
  return el('div', { class: 'doc-section' }, [
    el('h3', null, 'Trading activity — gains and losses'),
    el('div', null, groups),
    el('p', { class: 'total' }, 'Total: proceeds ' + moneyText(d.totals.proceeds_aud)
      + ', cost base ' + moneyText(d.totals.cost_base_aud)
      + ', gain/loss ' + moneyText(d.totals.gain_loss_aud)
      + ', gain after discount ' + moneyText(d.totals.gain_after_discount_aud)),
  ]);
}

// ---- CGT summary -------------------------------------------------------

function summaryRow(label, value, opts) {
  opts = opts || {};
  return el('tr', { class: opts.header ? 'summary-header' : (opts.total ? 'summary-total' : null) }, [
    el('td', { class: opts.indent ? 'indent' : null }, label),
    el('td', { class: 'num' }, value != null ? moneyEl(value) : ''),
  ]);
}

function cgtSummarySection(s) {
  if (!s) {
    return el('div', { class: 'doc-section' }, [
      el('h3', null, 'Gain / loss summary'),
      el('p', null, 'No capital gains or losses activity recorded for this year.'),
    ]);
  }
  const rows = [
    summaryRow("Capital Gains on shares applicable for 'Other' method (short term gains)", null, { header: true }),
    summaryRow('Short Term Gains', s.short_term_gains, { indent: true }),
    summaryRow('less Capital losses available to be offset', s.losses_applied_other, { indent: true }),
    summaryRow('Net short term gain', s.net_other_gain, { indent: true, total: true }),
    summaryRow("Capital Gains on shares applicable for 'Discount' method (long term gains)", null, { header: true }),
    summaryRow('Long Term Gains', s.long_term_gains, { indent: true }),
    summaryRow('Discounted Capital Gain Distributions (Grossed Up)', s.amma_discount_gains_grossed_up, { indent: true }),
    summaryRow('less Capital losses available to be offset', s.losses_applied_discount, { indent: true }),
    summaryRow('less CGT Concession Amount @ 50%', s.cgt_concession_amount, { indent: true }),
    summaryRow('Net discount-eligible gain (after concession)', s.net_discount_eligible_gain, { indent: true, total: true }),
    summaryRow('Capital Gain', s.net_capital_gain, { total: true }),
  ];
  const lossRows = [
    summaryRow('Capital losses arising this year', s.capital_losses_this_year),
    summaryRow('Capital loss brought forward', s.capital_loss_brought_forward),
    summaryRow('Capital loss carried forward', s.capital_loss_carried_forward),
    summaryRow('CGT event E10 gain (informational — AMIT cost base exhausted)', s.cgt_event_e10_gain),
    summaryRow('CGT event G1 gain (informational — return of capital exceeded cost base)', s.cgt_event_g1_gain),
    summaryRow('CGT event C2 gain (informational — return of capital on units sold before payment)', s.cgt_event_c2_gain),
  ];
  return el('div', { class: 'doc-section' }, [
    el('h3', null, 'Gain / loss summary'),
    el('table', { class: 'doc-table summary-table' }, el('tbody', null, rows)),
    el('h4', null, 'Loss position'),
    el('table', { class: 'doc-table summary-table' }, el('tbody', null, lossRows)),
  ]);
}

// ---- income --------------------------------------------------------------

// Money columns not suffixed `_aud`: the AMMA/trust `tax_deferred_amount` and
// `tax_free_amount` fields are native-currency (informational, never
// AUD-converted — see entities::amma's doc comments) but are still cent
// figures that want the same rounding, not raw Decimal precision.
const EXTRA_MONEY_COLUMNS = ['tax_deferred_amount', 'tax_free_amount'];
function genericTable(rows, columns) {
  if (!rows || rows.length === 0) return el('p', null, 'None recorded.');
  const moneyCols = columns.filter(function (c) { return /_aud$/.test(c) || EXTRA_MONEY_COLUMNS.indexOf(c) !== -1; });
  return doc(
    columns.map(columnLabel),
    rows.map(function (r) {
      return el('tr', null, columns.map(function (c) {
        return moneyCols.indexOf(c) !== -1 ? moneyTd(r[c]) : td(r[c]);
      }));
    }),
  );
}

// The AMMA statement's income/CGT/non-assessable components, in the order the
// statement itself lists them. Deliberately NOT rendered one-row-per-statement
// like every other income table: fifteen money components plus the identifying
// columns need ~1400px of table, so the right-hand components were cut off the
// printed page entirely (an overflow box clips when printed — see the print
// rules in style.css). Transposed — components down the page, one column per
// statement — it fits any orientation, has room for every component, and reads
// the way the paper AMMA statement does, which is what hand-checking a figure
// against the source actually needs.
const AMMA_COMPONENTS = [
  'australian_interest_aud', 'australian_dividends_unfranked_aud', 'franked_dividends_aud',
  'franking_credits_aud', 'net_rent_aud', 'foreign_income_aud', 'foreign_tax_credits_aud',
  'foreign_tax_credits_capital_gains_aud',
  'other_income_aud', 'cgt_discount_gains_aud', 'cgt_indexation_gains_aud', 'cgt_other_gains_aud',
  'capital_losses_applied_aud', 'tfn_withholding_tax_aud', 'tax_deferred_amount', 'tax_free_amount',
];

function ammaStatementsTable(rows) {
  if (!rows || rows.length === 0) return el('p', null, 'None recorded.');
  const headers = ['Component'].concat(rows.map(function (r) {
    return r.ticker + ' — year ended ' + cellText(r.tax_year_end_date);
  }));
  const body = AMMA_COMPONENTS.map(function (c) {
    return el('tr', null, [el('td', null, columnLabel(c))].concat(rows.map(function (r) {
      return moneyTd(r[c]);
    })));
  });
  const table = doc(headers, body);
  table.classList.add('amma-table');
  return table;
}

// The conduit-foreign-income column is a memo *within* the unfranked amount,
// not an amount beside it — printed so a figure ties back to the statement's
// own CFI line, never added to a total. Shown only when the year actually has
// one, so the note doesn't clutter a report with no CFI in it. (For an
// Australian resident an unfranked dividend declared to be CFI is assessable;
// it is NANE only to a foreign resident.)
function cfiFootnote(inc) {
  const any = ['dividends', 'trust_income'].some(function (k) {
    return (inc[k] || []).some(function (r) { return Number(r.conduit_foreign_income_aud) !== 0; });
  });
  if (!any) return null;
  return el('p', { class: 'hint' },
    'Conduit foreign income is the part of the unfranked amount the payer declared to be CFI — '
    + 'it is included in the unfranked amount shown, not additional to it, and is assessable to an '
    + 'Australian resident.');
}

// The foreign-income table's three printed lines: what the taxpayer holds
// directly (question 20's gross), what an AMIT attributed, and the two
// together. The server strikes them, rounded to the cent so the printed column
// adds up; this only renders them. The ESS memo row is in none of the three —
// its amount is already inside the item 12 discount, and its own Kind cell
// says it is a memo.
function foreignIncomeTotals(inc) {
  if (!inc.foreign_income || inc.foreign_income.length === 0) return null;
  const t = inc.foreign_income_totals || {};
  function line(cls, label, part) {
    const s = part || {};
    return el('p', { class: cls }, label + ': ' + moneyText(s.amount_aud || 0)
      + ', foreign tax paid ' + moneyText(s.foreign_tax_paid_aud || 0));
  }
  return el('div', null, [
    line('subtotal', 'Subtotal, non-AMMA', t.non_amma),
    line('subtotal', 'Subtotal, AMMA', t.amma),
    line('total', 'Total foreign income', t.total),
  ]);
}

// The $1,000 taxed-upfront ESS reduction rests on a condition this system
// cannot check — the taxpayer's adjusted taxable income for the year being
// A$180,000 or less — and the printed document is the copy an accountant reads,
// where `ess_taxed_upfront_reduction` would otherwise be a bare line with an
// empty ATO label. Same call as `cfiFootnote`: print the condition only when a
// reduction was actually applied, and say where to record the other answer.
function essReductionFootnote(lines) {
  const line = (lines || []).find(function (l) { return l.field === 'ess_taxed_upfront_reduction'; });
  if (!line || Number(line.value) === 0) return null;
  return el('p', { class: 'hint' },
    'The $1,000 taxed-upfront reduction shown in the summary assumes adjusted taxable income of '
    + '$180,000 or less for this year — a test outside this system\u2019s data. If the year exceeds it, '
    + 'record the year as ineligible under Tax Year Settings and regenerate: the discount is then '
    + 'reported unreduced.');
}

// Where each deduction goes on the return. The deductible amount is one
// figure, but the question it is claimed at follows the income it was earning:
// a trust/AMIT distribution puts it at 13Y (interest on money borrowed to buy
// the units included), foreign-source income nets it into 20M — with a debt
// deduction moved to D15, which question 20's worksheet excludes — and
// everything else is the ordinary D7/D8 case.
function deductionDestinationFootnote(inc) {
  if (!inc.deductions || inc.deductions.length === 0) return null;
  return el('p', { class: 'hint' },
    'The ATO label column is where each deduction is claimed: 13Y for expenses of earning a trust '
    + 'or AMIT distribution (interest on money borrowed to buy the units included \u2014 question 13 '
    + 'takes debt deductions too), 20M for expenses of earning foreign-source income (question 20\u2019s '
    + 'other net foreign source income is the gross less those expenses), D15 for a debt deduction '
    + 'against foreign income, which the question 20 worksheet excludes, and D7/D8 for the ordinary '
    + 'Australian interest and dividend case. A portfolio-wide expense \u2014 one attributed to no '
    + 'listing \u2014 cannot be routed from what is recorded and is reported at D7/D8; split it per '
    + 'holding if it belongs elsewhere.');
}

function incomeSection(inc, summaryLines) {
  return el('div', { class: 'doc-section' }, [
    el('h3', null, 'Income'),
    el('h4', null, 'Trust income'),
    genericTable(inc.trust_income, ['date_paid', 'ticker', 'entitlement_date', 'franked_amount_aud', 'unfranked_amount_aud', 'conduit_foreign_income_aud', 'foreign_source_income_aud', 'franking_credits_aud', 'tax_deferred_amount']),
    inc.amma_statements.length ? el('p', { class: 'hint' }, 'AMMA statement components for the year:') : null,
    ammaStatementsTable(inc.amma_statements),
    el('h4', null, 'Dividend income'),
    genericTable(inc.dividends, ['date_paid', 'ticker', 'ex_date', 'franked_amount_aud', 'unfranked_amount_aud', 'conduit_foreign_income_aud', 'franking_credits_aud', 'lic_capital_gain_deduction_aud', 'franking_status']),
    cfiFootnote(inc),
    inc.employment_income && inc.employment_income.length
      ? el('h4', null, 'Employment income (not investment income)') : null,
    inc.employment_income && inc.employment_income.length
      ? genericTable(inc.employment_income, ['date_paid', 'ticker', 'amount_aud']) : null,
    inc.employment_income && inc.employment_income.length
      ? el('p', { class: 'hint' },
          'Cash recorded against a holding that is not income of that holding — a dividend '
          + 'equivalent paid on unvested rights is remuneration under s 6-5, not a dividend '
          + '(TD 2017/26). It belongs at item 1/2 salary and wages, which the employer normally '
          + 'reports through STP, so it is listed here for reconciliation only and is in none of '
          + 'the dividend or investment-income totals above.') : null,
    inc.other_income && inc.other_income.length
      ? el('h4', null, 'Other income (item 24)') : null,
    inc.other_income && inc.other_income.length
      ? genericTable(inc.other_income, ['date_paid', 'ticker', 'amount_aud']) : null,
    inc.other_income && inc.other_income.length
      ? el('p', { class: 'hint' },
          'Ordinary income produced by a holding but paid as no distribution of it \u2014 a crypto '
          + 'staking reward, or an airdrop of an established token, assessable at the tokens\u2019 '
          + 'market value when they were received (QC 69950). It belongs at item 24, other income, '
          + 'which nothing prefills, and is counted in the assessable investment income above; the '
          + 'tokens themselves are a parcel costed at that same value.') : null,
    el('h4', null, 'Foreign income'),
    genericTable(inc.foreign_income, ['kind', 'ticker', 'date', 'amount_aud', 'foreign_tax_paid_aud']),
    foreignIncomeTotals(inc),
    el('h4', null, 'Interest income'),
    genericTable(inc.interest, ['date_paid', 'source', 'amount_aud', 'foreign_source', 'foreign_tax_paid_aud', 'tfn_withholding_tax_aud']),
    el('h4', null, 'Employee share scheme income'),
    genericTable(inc.ess, ['taxing_point_date', 'ticker', 'taxed_upfront_eligible_aud', 'taxed_upfront_not_eligible_aud', 'deferral_discount_aud', 'pre_2009_cessation_discount_aud']),
    essReductionFootnote(summaryLines),
    el('h4', null, 'Deductions'),
    genericTable(inc.deductions, ['date_incurred', 'expense_type', 'ticker', 'amount_aud', 'ato_label', 'description']),
    deductionDestinationFootnote(inc),
  ]);
}

// ---- overall tax summary ------------------------------------------------

function taxSummarySection(lines) {
  if (!lines || lines.length === 0) {
    return el('div', { class: 'doc-section' }, [
      el('h3', null, 'Overall tax summary'),
      el('p', null, 'No assessable income, AMMA, or deduction activity recorded for this year.'),
    ]);
  }
  const rows = lines
    .filter(function (l) { return l.field !== 'tax_year' && l.field !== 'taxpayer_basis'; })
    .map(function (l) {
      const isMoney = typeof l.value === 'string' && /^-?\d+(\.\d+)?$/.test(l.value);
      return el('tr', null, [
        td(columnLabel(l.field)),
        td(l.ato_label || '—'),
        isMoney ? moneyTd(l.value) : td(l.value),
      ]);
    });
  return el('div', { class: 'doc-section' }, [
    el('h3', null, 'Overall tax summary'),
    doc(['Field', 'ATO label', 'Value'], rows),
  ]);
}

// ---- top level ------------------------------------------------------------

function renderReport(report) {
  const m = report.meta;
  const wrap = el('div', { class: 'tax-report-doc' }, [
    el('h1', null, 'Annual Tax Report — ' + fyLabel(m.tax_year) + ' (' + m.period_start + ' – ' + m.period_end + ')'),
    el('p', { class: 'hint' }, 'Produced ' + fmtLocalTimestamp(m.generated_at) + ' · ' + m.taxpayer_basis),
    completenessSection(report.completeness),
    disposalsSection(report.disposals),
    cgtSummarySection(report.cgt_summary),
    incomeSection(report.income, report.tax_summary),
    taxSummarySection(report.tax_summary),
  ]);
  return wrap;
}

export async function viewTaxReport() {
  setActiveNav('r:tax-report');
  const years = await api('GET', '/reports/tax-report/years');
  const header = el('div', null, [
    el('h2', null, 'Annual Tax Report'),
    el('p', { class: 'view-desc' },
      'A printable, archivable tax document for one Australian financial year — enough detail to hand-check every ' +
      'figure against the source contract notes and statements. Every figure is sourced from the existing reports; ' +
      'nothing here is a second calculation. Pick a year and Generate, then Print / Save as PDF to archive it.'),
  ]);

  const yearSelect = el('select', null, years.slice().reverse().map(function (y) {
    return el('option', { value: y }, fyLabel(y));
  }));
  if (years.length) yearSelect.value = String(years[years.length - 1]);

  const generateBtn = el('button', { type: 'button', class: 'primary' }, 'Generate report');
  const printBtn = el('button', { type: 'button', hidden: true }, 'Print / Save as PDF');
  // The stylesheet asks for A4 landscape via @page, which Chrome and Firefox
  // honour; WebKit implements neither the size nor the margin descriptor, so
  // Safari prints at whatever the dialog is set to. Say so rather than leave it
  // as folklore — the document is sized to fit portrait as well, so a
  // forgotten setting costs density, never a clipped column.
  const printHint = el('span', { class: 'hint', hidden: true },
    'Prints landscape automatically in Chrome and Firefox. Safari ignores the page-size rule — '
    + 'choose Landscape in its print dialog for the roomiest result (portrait still fits every column).');
  const toolbar = el('div', { class: 'toolbar tax-report-toolbar' }, [
    el('label', null, ['Tax year ', yearSelect]),
    generateBtn,
    printBtn,
    printHint,
  ]);
  const result = el('div');

  generateBtn.addEventListener('click', async function () {
    if (!years.length) { toast('No tax year has any recorded data yet.', true); return; }
    try {
      const report = await api('POST', '/reports/tax-report', { tax_year: Number(yearSelect.value) });
      result.innerHTML = '';
      result.appendChild(renderReport(report));
      printBtn.hidden = false;
      printHint.hidden = false;
    } catch (e) {
      toast(e.message, true);
    }
  });
  printBtn.addEventListener('click', function () { window.print(); });

  setMain(el('div', null, [header, toolbar, result]));
}
