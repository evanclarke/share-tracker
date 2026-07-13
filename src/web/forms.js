//
// share-tracker frontend: form building blocks. The field constructors the
// entity/action configs are written in, the generic field input builder and
// reader the engine renders them with, the entity-specific form wiring hooks
// (GST-inclusive brokerage, the income form's simple-first entry), and the
// shared parcel-allocation editor.
//
import {
  el, api, toast, loadOptions, listingNamer, describeTrade,
  addDecimalStrings, decParts, mulToCents, frankingCreditFor, decEq,
} from './util.js';

// ---- field constructors ----------------------------------------------
export function field(name, label, type, extra) { return Object.assign({ name: name, label: label, type: type }, extra || {}); }
export const txt = function (n, l, x) { return field(n, l, 'text', x); };
export const dec = function (n, l, x) { return field(n, l, 'decimal', Object.assign({ default: '0' }, x || {})); };
export const int = function (n, l, x) { return field(n, l, 'int', x); };
export const dt = function (n, l, x) { return field(n, l, 'date', x); };
export const bool = function (n, l, x) { return field(n, l, 'bool', x); };
export const sel = function (n, l, options, x) { return field(n, l, 'select', Object.assign({ options: options }, x || {})); };
export const fk = function (n, l, source, x) { return field(n, l, 'select', Object.assign({ source: source, encode: 'int' }, x || {})); };

// ---- generic field inputs ----------------------------------------------
export async function buildFieldInput(f, value, disabled) {
  const wrap = el('div', { class: 'field' });
  const id = 'f_' + f.name;
  wrap.appendChild(el('label', { for: id }, f.label + (f.required ? ' *' : '')));
  let input;
  if (f.type === 'select') {
    input = el('select', { id: id, name: f.name });
    const options = f.options
      ? f.options.map(function (o) { return typeof o === 'string' ? { value: o, label: o } : o; })
      : await loadOptions(f.source);
    const current = value != null ? value : f.default;
    if (f.optional || current == null) input.appendChild(el('option', { value: '' }, '—'));
    options.forEach(function (o) { input.appendChild(el('option', { value: o.value }, o.label)); });
    if (current != null) input.value = String(current);
    if (f.required) input.required = true;
  } else if (f.type === 'bool') {
    input = el('input', { id: id, name: f.name, type: 'checkbox' });
    if (value === true || (value == null && f.default === true)) input.checked = true;
  } else {
    input = el('input', { id: id, name: f.name, type: f.type === 'date' ? 'date' : 'text' });
    if (f.type === 'decimal') input.setAttribute('inputmode', 'decimal');
    if (f.type === 'int') input.setAttribute('inputmode', 'numeric');
    const v = value != null ? value : (f.default != null ? f.default : '');
    input.value = v == null ? '' : String(v);
    if (f.required) input.required = true;
  }
  if (disabled) input.disabled = true;
  wrap.appendChild(input);
  if (f.hint) wrap.appendChild(el('div', { class: 'hint' }, f.hint));
  return wrap;
}

export function readFieldValue(f, formEl) {
  const inp = formEl.querySelector('[name="' + f.name + '"]');
  if (f.type === 'bool') return inp.checked;
  const raw = (inp.value || '').trim();
  if (raw === '') {
    // Decimal fields with a default send that default; everything else empty
    // becomes null (a nullable/optional column, or an omitted optional date).
    if (f.type === 'decimal' && f.default != null && f.default !== '') return f.default;
    return null;
  }
  const encode = f.encode || (f.type === 'int' ? 'int' : 'string');
  if (encode === 'int') return Number(raw);
  return raw;
}

// ---- GST-inclusive brokerage wiring ------------------------------------
// Shared by the Buy/DRP trade form and the Sell form: ticking "Brokerage
// includes GST" hides the GST field (the server derives GST as 1/11 of the
// inclusive amount, rounded to the cent) and relabels brokerage. A flagged
// trade's `brokerage` already reads back from the API as the one
// GST-inclusive amount (the lossless round-trip contract — docs/API.md), so
// the generic field fill re-presents it with no client-side recombination.
export function wireGstBrokerage(form) {
  const flag = form.querySelector('[name="brokerage_includes_gst"]');
  const brokLabel = form.querySelector('label[for="f_brokerage"]');
  const gstWrap = form.querySelector('[name="gst_on_brokerage"]').closest('.field');
  function apply() {
    gstWrap.style.display = flag.checked ? 'none' : '';
    brokLabel.textContent = flag.checked ? 'Brokerage (GST-inclusive)' : 'Brokerage';
  }
  flag.addEventListener('change', apply);
  apply();
}

// ---- income simple-entry wiring -----------------------------------------
// The tax components a registry payment advice doesn't print (or that the
// simple franking selector derives): hidden until the advanced toggle.
const INCOME_ADVANCED_FIELDS = [
  'ex_date', 'franked_amount', 'unfranked_amount', 'foreign_source_income',
  'foreign_tax_paid', 'tfn_withholding_tax', 'franking_credits',
  'lic_capital_gain_deduction', 'conduit_foreign_income', 'trust_income',
  'entitlement_date', 'tax_deferred_amount', 'currency', 'holding_account_id',
];

// Classify a stored row for the simple form: which franking-selector mode
// re-presents it losslessly, and the single amount to show. Returns null
// when only the advanced fields can represent it (the form then opens
// advanced so nothing is hidden) — any advanced-only field off its
// default, a partially franked split, or franking credits that aren't the
// derived amount × 30/70.
function incomeSimpleShape(existing) {
  if (!existing) return { mode: 'Unfranked', amount: '' };
  const nz = function (v) { return v != null && !decEq(String(v), '0'); };
  if (existing.ex_date != null || nz(existing.foreign_source_income) || nz(existing.foreign_tax_paid)
    || nz(existing.tfn_withholding_tax) || nz(existing.lic_capital_gain_deduction)
    || nz(existing.conduit_foreign_income) || existing.tax_deferred_amount != null
    || existing.currency !== 'AUD' || existing.holding_account_id !== 1) return null;
  const franked = nz(existing.franked_amount), credits = nz(existing.franking_credits);
  if (!franked && !credits) {
    return { mode: existing.trust_income ? 'Trust' : 'Unfranked', amount: String(existing.unfranked_amount) };
  }
  if (franked && !existing.trust_income && !nz(existing.unfranked_amount)
    && decEq(String(existing.franking_credits), frankingCreditFor(String(existing.franked_amount)) || '')) {
    return { mode: 'FullyFranked', amount: String(existing.franked_amount) };
  }
  return null;
}

// The income form's simple-first behaviour: a payment-amount input plus a
// franking selector stand in for the component fields (mapped onto the
// body at submit), the per-share pair shows its computed product as a
// live hint, and a "Reinvested under DRP" tick chains the existing
// POST /income/:id/reinvest after the save (a reinvest failure leaves the
// saved income standing — the row's Reinvest action is the retry path).
export function wireIncomeEntry(form, existing) {
  const shape = incomeSimpleShape(existing);

  // Advanced toggle, at the top of the form.
  const advFlag = el('input', { id: 'f_simple_advanced', name: 'simple_advanced', type: 'checkbox' });
  advFlag.checked = shape == null;
  form.insertBefore(
    el('div', { class: 'field' }, [el('label', { for: 'f_simple_advanced' }, 'Show advanced fields'), advFlag]),
    form.firstChild
  );

  // Simple section: amount + franking selector, between the date and the
  // per-share pair.
  const amountInput = el('input', { id: 'f_simple_amount', name: 'simple_amount', type: 'text' });
  amountInput.setAttribute('inputmode', 'decimal');
  const frankSel = el('select', { id: 'f_simple_franking', name: 'simple_franking' });
  [
    { value: 'Unfranked', label: 'Unfranked' },
    { value: 'FullyFranked', label: 'Fully franked (30%)' },
    { value: 'Trust', label: 'Trust distribution' },
  ].forEach(function (o) { frankSel.appendChild(el('option', { value: o.value }, o.label)); });
  if (shape) { amountInput.value = shape.amount; frankSel.value = shape.mode; }
  const frankHint = el('div', { class: 'hint' });
  const simpleSection = el('div', null, [
    el('div', { class: 'field' }, [el('label', { for: 'f_simple_amount' }, 'Amount'), amountInput,
      el('div', { class: 'hint' }, 'The statement’s gross payment in AUD.')]),
    el('div', { class: 'field' }, [el('label', { for: 'f_simple_franking' }, 'Franking'), frankSel, frankHint]),
  ]);
  const apsWrap = form.querySelector('[name="amount_per_security"]').closest('.field');
  form.insertBefore(simpleSection, apsWrap);

  function updateFrankHint() {
    const credit = frankSel.value === 'FullyFranked' ? frankingCreditFor(amountInput.value) : null;
    frankHint.textContent = frankSel.value === 'FullyFranked'
      ? 'Franking credits of ' + (credit || 'amount × 30/70') + ' will be recorded (amount × 30/70). Partially franked or non-30%-rate dividends: use the advanced fields.'
      : (frankSel.value === 'Trust' ? 'Recorded as unfranked trust income; the component breakdown arrives with the AMMA statement for AMIT funds.' : '');
  }
  amountInput.addEventListener('input', updateFrankHint);
  frankSel.addEventListener('change', updateFrankHint);
  updateFrankHint();

  // Trust distributions are assessed by present entitlement, not payment
  // (docs/ato/trust-income-timing.md): selecting Trust reveals the
  // entitlement-date field in simple mode too, prefilled with the pay date.
  const entitlementInput = form.querySelector('[name="entitlement_date"]');
  const datePaidInput = form.querySelector('[name="date_paid"]');
  function applyEntitlement() {
    if (advFlag.checked) return; // advanced mode shows every field
    const isTrust = frankSel.value === 'Trust';
    entitlementInput.closest('.field').style.display = isTrust ? '' : 'none';
    if (isTrust && !entitlementInput.value) entitlementInput.value = datePaidInput.value || '';
  }
  frankSel.addEventListener('change', applyEntitlement);
  datePaidInput.addEventListener('change', applyEntitlement);

  // Live product hint for the per-share cross-check pair.
  const apsInput = form.querySelector('[name="amount_per_security"]');
  const heldInput = form.querySelector('[name="securities_held"]');
  const productHint = el('div', { class: 'hint' });
  heldInput.closest('.field').appendChild(productHint);
  function grossEntered() {
    if (!advFlag.checked) return amountInput.value.trim();
    const f = form.querySelector('[name="franked_amount"]').value.trim() || '0';
    const u = form.querySelector('[name="unfranked_amount"]').value.trim() || '0';
    const fo = form.querySelector('[name="foreign_source_income"]').value.trim() || '0';
    if (!decParts(f) || !decParts(u) || !decParts(fo)) return '';
    return addDecimalStrings(addDecimalStrings(f, u), fo);
  }
  function updateProductHint() {
    const aps = apsInput.value.trim(), held = heldInput.value.trim();
    const product = aps && held ? mulToCents(aps, held) : null;
    if (!product) { productHint.textContent = ''; productHint.className = 'hint'; return; }
    const gross = grossEntered();
    const matches = gross !== '' && decParts(gross) != null && decEq(product, gross);
    productHint.textContent = aps + ' × ' + held + ' = ' + product
      + (matches ? ' — matches the gross amount.' : ' — does not match the gross amount entered.');
    productHint.className = matches ? 'hint' : 'hint warn';
  }
  form.addEventListener('input', updateProductHint);
  updateProductHint();

  // DRP tick: only for a distribution not yet reinvested.
  let drpFlag = null, priceInput = null, drpUnitsInput = null, drpDateInput = null, drpFxInput = null;
  if (!existing || existing.reinvestment_trade_id == null) {
    drpFlag = el('input', { id: 'f_simple_drp', name: 'simple_drp', type: 'checkbox' });
    priceInput = el('input', { id: 'f_drp_price', name: 'drp_price', type: 'text' });
    priceInput.setAttribute('inputmode', 'decimal');
    drpUnitsInput = el('input', { id: 'f_drp_units', name: 'drp_units', type: 'text' });
    drpUnitsInput.setAttribute('inputmode', 'decimal');
    drpDateInput = el('input', { id: 'f_drp_date', name: 'drp_date', type: 'date' });
    drpFxInput = el('input', { id: 'f_drp_fx', name: 'drp_fx', type: 'text', value: '1' });
    drpFxInput.setAttribute('inputmode', 'decimal');
    const drpFields = el('div', null, [
      el('div', { class: 'field' }, [el('label', { for: 'f_drp_price' }, 'Reinvestment price *'), priceInput,
        el('div', { class: 'hint' }, 'Per-unit DRP price from the statement; whole units and the carried residual are computed server-side.')]),
      el('div', { class: 'field' }, [el('label', { for: 'f_drp_units' }, 'Units allotted (fractional plans)'), drpUnitsInput,
        el('div', { class: 'hint' }, 'Leave blank for a whole-share registry DRP. For a broker plan that allots fractional shares, enter the statement’s exact units — taken verbatim, cross-checked against the reinvestable cash, no residual.')]),
      el('div', { class: 'field' }, [el('label', { for: 'f_drp_date' }, 'Reinvestment trade date'), drpDateInput,
        el('div', { class: 'hint' }, 'Optional; defaults to the pay date.')]),
      el('div', { class: 'field' }, [el('label', { for: 'f_drp_fx' }, 'Reinvestment FX rate'), drpFxInput]),
    ]);
    form.appendChild(el('div', { class: 'field' }, [el('label', { for: 'f_simple_drp' }, 'Reinvested under DRP'), drpFlag,
      el('div', { class: 'hint' }, 'Creates the linked DRP trade after saving (the holding must be DRP-enrolled).')]));
    form.appendChild(drpFields);
    function applyDrp() {
      drpFields.style.display = drpFlag.checked ? '' : 'none';
      priceInput.required = drpFlag.checked;
    }
    drpFlag.addEventListener('change', applyDrp);
    applyDrp();
  }

  function applyMode() {
    const adv = advFlag.checked;
    simpleSection.style.display = adv ? 'none' : '';
    amountInput.required = !adv;
    INCOME_ADVANCED_FIELDS.forEach(function (n) {
      const inp = form.querySelector('[name="' + n + '"]');
      if (inp) inp.closest('.field').style.display = adv ? '' : 'none';
    });
    applyEntitlement();
    updateProductHint();
  }
  advFlag.addEventListener('change', applyMode);
  applyMode();

  return {
    // Simple mode maps the amount through the franking selector onto the
    // component fields; advanced mode submits the fields as entered.
    transformBody: function (body) {
      if (advFlag.checked) return;
      const amount = amountInput.value.trim();
      if (!decParts(amount)) throw new Error('Amount must be a decimal number.');
      const mode = frankSel.value;
      body.franked_amount = mode === 'FullyFranked' ? amount : '0';
      body.unfranked_amount = mode === 'FullyFranked' ? '0' : amount;
      body.franking_credits = mode === 'FullyFranked' ? frankingCreditFor(amount) : '0';
      body.trust_income = mode === 'Trust';
      // Only a trust row may carry an entitlement date or tax-deferred
      // amount (the server rejects them otherwise) — clear values left
      // behind by switching modes.
      if (mode !== 'Trust') { body.entitlement_date = null; body.tax_deferred_amount = null; }
    },
    afterSave: async function (id) {
      if (!drpFlag || !drpFlag.checked) return null;
      const body = { reinvestment_price: priceInput.value.trim() };
      if (drpUnitsInput.value.trim() !== '') body.units = drpUnitsInput.value.trim();
      if (drpDateInput.value) body.date = drpDateInput.value;
      if (drpFxInput.value.trim() !== '') body.fx_rate = drpFxInput.value.trim();
      try {
        const trade = await api('POST', '/income/' + id + '/reinvest', body);
        const listingName = await listingNamer();
        return trade ? 'Saved and reinvested into ' + describeTrade(trade, listingName) + ' (trade #' + trade.id + ').' : 'Saved and reinvested.';
      } catch (e) {
        toast('Income saved, but the reinvestment failed — ' + e.message + '. Retry from the row’s Reinvest action.', true);
        return '';
      }
    },
  };
}

// ---- allocation editor --------------------------------------------------
// Shared parcel-allocation row builder used by the Sell form, the Transfer
// form, and the buy-back Participate action: a list of (purchase-parcel
// select, decimal quantity) rows with add/remove buttons. Returns the
// section element to append to the form and a `read()` harvesting the rows
// as [{ purchase_trade_id, quantity_allocated }] (blank rows skipped; the
// quantity key is `labels.qtyField` when a caller's API names it differently,
// e.g. the sell-rights action's `units`). The callers differ only in labels
// and hint text.
export function allocationEditor(parcelOptions, existingAllocs, labels) {
  labels = Object.assign({
    heading: 'Parcel allocations',
    parcelLabel: 'Purchase parcel',
    qtyLabel: 'Quantity allocated',
    addLabel: '+ Add allocation',
  }, labels);
  const list = el('div');
  function addRow(alloc) {
    const purchaseSel = el('select', { name: 'alloc_purchase' });
    parcelOptions.forEach(function (o) { purchaseSel.appendChild(el('option', { value: o.value }, o.label)); });
    if (alloc) purchaseSel.value = String(alloc.purchase_trade_id);
    const qtyInput = el('input', { type: 'text', inputmode: 'decimal', name: 'alloc_qty', value: alloc ? String(alloc.quantity_allocated) : '' });
    const row = el('div', { class: 'alloc-row' }, [
      el('div', { class: 'field' }, [el('label', null, labels.parcelLabel), purchaseSel]),
      el('div', { class: 'field' }, [el('label', null, labels.qtyLabel), qtyInput]),
      el('button', { type: 'button', class: 'small danger', onclick: function () { list.removeChild(row); } }, 'Remove'),
    ]);
    list.appendChild(row);
  }
  if (existingAllocs && existingAllocs.length) existingAllocs.forEach(addRow); else addRow(null);
  const section = el('div', null, [
    el('h3', null, labels.heading),
    el('p', { class: 'hint' }, labels.hint),
    list,
    el('button', { type: 'button', class: 'small', onclick: function () { addRow(null); } }, labels.addLabel),
  ]);
  function read() {
    const allocs = [];
    list.querySelectorAll('.alloc-row').forEach(function (r) {
      const pid = r.querySelector('[name="alloc_purchase"]').value;
      const qty = (r.querySelector('[name="alloc_qty"]').value || '').trim();
      if (pid !== '' && qty !== '') {
        const a = { purchase_trade_id: Number(pid) };
        a[labels.qtyField || 'quantity_allocated'] = qty;
        allocs.push(a);
      }
    });
    return allocs;
  }
  return { section: section, read: read };
}
