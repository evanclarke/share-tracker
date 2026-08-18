//
// share-tracker single-page frontend: the rendering engine and entry point.
//
// No build step, no framework — native ES modules loaded straight by the
// browser (<script type="module">). This module holds the generic machinery:
// the filterable/sortable/paginated table every data view renders through,
// the config-driven entity list/form views, the report runner, the
// post-record action view, the bespoke Sells/Transfers flows (written
// atomically with their parcel allocations), and the hash router. What it
// renders is described in config.js (ENTITIES / REPORTS / ACTIONS), built
// from the field constructors and form wiring in forms.js, over the shared
// utilities in util.js.
//
import {
  el, toast, setMain, looksNumeric, isTimestamp, fmtLocalTimestamp, utcTooltip,
  cellText, numericDisplay, columnKinds, columnLabel, columnLabelMaps,
  fkLabelMaps, api, apiUrl, pathSeg, nextId, loadOptions, listingNamer, describeTrade, tradeOrigin,
  periodReturnPct, holdingHasActivity, loadPref, savePref,
} from './util.js';
import {
  field, txt, dec, dt, bool, fk,
  buildFieldInput, readFieldValue, wireGstBrokerage, allocationEditor,
} from './forms.js';
import { ENTITIES, REPORTS, ACTIONS } from './config.js';
import { seriesChart, presetRange, sliceSeries } from './chart.js';
import { buildNav, setActiveNav } from './nav.js';
import { viewTaxReport } from './taxreport.js';

const entityBySlug = {};
ENTITIES.forEach(function (e) { entityBySlug[e.slug] = e; });
const reportBySlug = {};
REPORTS.forEach(function (r) { reportBySlug[r.slug] = r; });
const actionBySlug = {};
ACTIONS.forEach(function (a) { actionBySlug[a.slug] = a; });

// ---- generic table ----------------------------------------------------
// Default page size: tables with this many rows or fewer show no pager; a
// larger filtered result set is paged so only one page is in the DOM at once.
const PAGE_SIZE = 50;

// Every data table in the app — entity lists, the Sells list, and report
// tables — goes through this one renderer so they are uniformly filterable,
// sortable, and paginated. Each column has its own filter input (substring
// match on that column's text); the filters AND together, so you can e.g.
// filter currency to "USD" and date to "2024" at once. Click a column header
// to sort it (toggling ascending/descending). Filtering and sorting apply to
// the whole result set; only the current page's slice (PAGE_SIZE rows) is put
// in the DOM, with a prev/next pager + "showing m–n of total" count that
// appears only when the filtered total exceeds one page. `opts.actions`, if
// given, renders a trailing non-sortable, non-filtered Actions cell per row;
// `opts.statusField` renders that column as a status badge; `opts.labels`
// ({col: {id → label}}, from fkLabelMaps) shows the label instead of the raw
// foreign-key id — filtering and sorting follow the label, and the id moves to
// the tooltip. `opts.expand`, if given, is a synchronous `row => childSpec`
// (`{ rows, cols, opts }`, or a falsy value/empty `rows` for a childless row):
// a leading toggle column shows ▸/▾ for a row with children, and an expanded
// row's children render as a nested `filterableTable` in a full-width detail
// row underneath — `childSpec.opts` may itself carry `expand`, so a report
// with a two-level breakdown (year → disposal → parcel) nests by recursion.
// An "Expand all"/"Collapse all" pair sits above the table whenever any
// caller supplies `expand`.
function filterableTable(rows, cols, opts) {
  opts = opts || {};
  const statusField = opts.statusField;
  const actions = opts.actions;
  const expand = opts.expand;
  const labels = opts.labels || {};
  // Display kinds are derived from the column names alone (COLUMN_KINDS), so
  // every caller of filterableTable gets money rounding / rate precision with
  // no per-call wiring; numeric sorting still uses the raw underlying value.
  const kinds = columnKinds(cols);
  function displayText(row, c) {
    const map = labels[c];
    if (map && row[c] != null && map[row[c]] !== undefined) return map[row[c]];
    const nd = numericDisplay(row[c], kinds[c]);
    if (nd) return nd.text;
    return cellText(row[c]);
  }
  // A column is numeric if any row has a numeric value there — used for
  // right-alignment and numeric (not lexicographic) sorting. A labelled
  // foreign-key column displays names, so it is never numeric.
  const numeric = {};
  cols.forEach(function (c) { numeric[c] = !labels[c] && rows.some(function (r) { return looksNumeric(r[c]); }); });

  let sortCol = null;
  let sortDir = 1; // 1 = ascending, -1 = descending
  const filters = {}; // column → lowercased substring; absent/empty = no filter
  let page = 0; // zero-based current page within the filtered/sorted result set
  // Rows currently shown expanded (row object identity is stable across
  // sort/filter/page re-renders, so a plain Set works as the key). Only
  // meaningful when `expand` is set.
  const expandedRows = new Set();

  const container = el('div');

  if (expand) {
    container.appendChild(el('div', { class: 'expand-all-bar' }, [
      el('button', {
        type: 'button', class: 'small link',
        onclick: function () {
          rows.forEach(function (row) {
            const spec = expand(row);
            if (spec && spec.rows && spec.rows.length) expandedRows.add(row);
          });
          renderBody();
        },
      }, 'Expand all'),
      el('button', {
        type: 'button', class: 'small link',
        onclick: function () { expandedRows.clear(); renderBody(); },
      }, 'Collapse all'),
    ]));
  }

  // Header row: click-to-sort column titles, shown as friendly labels
  // (columnLabel) while sorting/filtering stay keyed by the raw column name.
  // `colHeadCells` holds only the real sortable columns — the click handler's
  // indicator loop must iterate this, not `headCells` below, which gains a
  // non-sortable expand/Actions th with no `_ind`/`_col` of its own.
  const colHeadCells = cols.map(function (c) {
    const indicator = el('span', { class: 'sort-ind' }, '');
    const th = el('th', { class: (numeric[c] ? 'num ' : '') + 'sortable' }, [columnLabel(c), indicator]);
    th._col = c;
    th._ind = indicator;
    th.addEventListener('click', function () {
      if (sortCol === c) sortDir = -sortDir; else { sortCol = c; sortDir = 1; }
      colHeadCells.forEach(function (h) {
        h._ind.textContent = h._col === sortCol ? (sortDir === 1 ? ' ▲' : ' ▼') : '';
      });
      renderBody();
    });
    return th;
  });
  const headCells = colHeadCells.slice();
  if (expand) headCells.unshift(el('th', { class: 'expand-col' }, ''));
  if (actions) headCells.push(el('th', null, 'Actions'));

  // Filter row: one input per column, AND-combined.
  const filterCells = cols.map(function (c) {
    const input = el('input', {
      type: 'search', class: 'table-filter', placeholder: 'Filter ' + columnLabel(c) + '…',
      oninput: function () {
        const v = this.value.trim().toLowerCase();
        if (v === '') delete filters[c]; else filters[c] = v;
        page = 0; // a changed filter re-pages from the first page
        renderBody();
      },
    });
    return el('th', { class: 'filter-cell' }, input);
  });
  if (expand) filterCells.unshift(el('th', { class: 'filter-cell' }));
  if (actions) filterCells.push(el('th', { class: 'filter-cell' }));

  const tbody = el('tbody');
  const thead = el('thead', null, [
    el('tr', null, headCells),
    el('tr', { class: 'filter-row' }, filterCells),
  ]);
  container.appendChild(el('table', null, [thead, tbody]));

  // Pager: a "showing m–n of total" count flanked by prev/next. Hidden when
  // the filtered total fits one page; updatePager toggles it per render.
  const pagerInfo = el('span', { class: 'pager-info' });
  const prevBtn = el('button', {
    type: 'button', class: 'small', onclick: function () { if (page > 0) { page--; renderBody(); } },
  }, '‹ Prev');
  const nextBtn = el('button', {
    type: 'button', class: 'small', onclick: function () { page++; renderBody(); },
  }, 'Next ›');
  const pager = el('div', { class: 'pager', hidden: true }, [prevBtn, pagerInfo, nextBtn]);
  container.appendChild(pager);

  // The whole filtered/sorted result set, paged only at render time so the
  // count and sort order always reflect the full set, never the visible page.
  function visibleRows() {
    let out = rows;
    const active = Object.keys(filters);
    if (active.length) {
      out = out.filter(function (row) {
        return active.every(function (c) { return displayText(row, c).toLowerCase().indexOf(filters[c]) !== -1; });
      });
    }
    if (sortCol != null) {
      out = out.slice().sort(function (a, b) {
        const av = a[sortCol], bv = b[sortCol];
        let cmp;
        if (numeric[sortCol] && looksNumeric(av) && looksNumeric(bv)) cmp = Number(av) - Number(bv);
        else cmp = displayText(a, sortCol).localeCompare(displayText(b, sortCol));
        return cmp * sortDir;
      });
    }
    return out;
  }

  // Show the pager (and set its count) only when the filtered total spills
  // past one page; `start` is the zero-based index of the first shown row.
  function updatePager(total, start, shown) {
    if (total <= PAGE_SIZE) { pager.hidden = true; return; }
    pager.hidden = false;
    pagerInfo.textContent = 'showing ' + (start + 1) + '–' + (start + shown) + ' of ' + total;
    prevBtn.disabled = start === 0;
    nextBtn.disabled = start + shown >= total;
  }

  function renderBody() {
    tbody.innerHTML = '';
    const vr = visibleRows();
    if (vr.length === 0) {
      const span = cols.length + (expand ? 1 : 0) + (actions ? 1 : 0);
      const filtered = Object.keys(filters).length > 0;
      tbody.appendChild(el('tr', null, el('td', { colspan: span, class: 'empty' },
        filtered ? 'No matching records.' : 'No records.')));
      updatePager(0, 0, 0);
      return;
    }
    // Clamp the page into range (e.g. after filtering shrinks the result set),
    // then put only that page's slice in the DOM.
    const lastPage = Math.ceil(vr.length / PAGE_SIZE) - 1;
    if (page > lastPage) page = lastPage;
    if (page < 0) page = 0;
    const start = page * PAGE_SIZE;
    const pageRows = vr.slice(start, start + PAGE_SIZE);
    updatePager(vr.length, start, pageRows.length);
    pageRows.forEach(function (row) {
      const tds = cols.map(function (c) {
        const v = row[c];
        if (statusField && c === statusField) {
          return el('td', null, el('span', { class: 'badge ' + cellText(v) }, cellText(v)));
        }
        if (isTimestamp(v)) {
          return el('td', { title: utcTooltip(v) }, cellText(v));
        }
        const text = displayText(row, c);
        // A labelled fk cell keeps its raw id reachable on the tooltip; a
        // money cell rounded for display keeps its full value there.
        let title = null;
        if (labels[c] && text !== cellText(v)) {
          title = 'id ' + cellText(v);
        } else {
          const nd = numericDisplay(v, kinds[c]);
          if (nd && nd.tip) title = nd.tip;
        }
        return el('td', { class: numeric[c] ? 'num' : null, title: title }, text);
      });

      let childSpec = null;
      if (expand) {
        childSpec = expand(row);
        const hasChildren = !!(childSpec && childSpec.rows && childSpec.rows.length);
        const toggle = hasChildren
          ? el('button', {
            type: 'button', class: 'expand-toggle',
            onclick: function () {
              if (expandedRows.has(row)) expandedRows.delete(row); else expandedRows.add(row);
              renderBody();
            },
          }, expandedRows.has(row) ? '▾' : '▸')
          : null;
        tds.unshift(el('td', { class: 'expand-col' }, toggle));
      }
      if (actions) tds.push(actions(row) || el('td'));
      tbody.appendChild(el('tr', null, tds));

      if (expand && childSpec && childSpec.rows && childSpec.rows.length && expandedRows.has(row)) {
        const span = cols.length + 1 + (actions ? 1 : 0);
        const childTable = filterableTable(childSpec.rows, childSpec.cols, childSpec.opts || {});
        tbody.appendChild(el('tr', { class: 'detail-row' },
          el('td', { colspan: span, class: 'detail-cell' }, childTable)));
      }
    });
  }

  renderBody();
  return container;
}

// Collects every `columns` list declared across an `expand` config chain
// (parent's own columns are not included — the caller already has those),
// so `dataTable` can fetch every descendant level's FK label maps up front:
// `filterableTable`'s `opts.expand` callback is synchronous, so no report
// table can await a label fetch lazily when a row first expands.
function expandColumns(expandCfg) {
  if (!expandCfg) return [];
  return (expandCfg.columns || []).concat(expandColumns(expandCfg.expand));
}

// Builds the synchronous `row => childSpec` `filterableTable` expects from a
// declarative `expand` config (see config.js REPORTS entries): `key` reads a
// nested array field on the row itself (e.g. a disposal's `parcels`); `from`
// + `matchOn` instead reads a sibling array from the same multi-`tables`
// response object (`context`), filtered to the rows matching this row's
// `matchOn` field (`matchOn: null` = every sibling row belongs to the single
// parent, as the what-if's one hypothetical disposal does). `labels` is the
// FK-label-map set already fetched for every level, shared unchanged down
// the recursion — every column name maps to the same source regardless of
// nesting depth.
function buildExpand(expandCfg, labels, context) {
  return function (row) {
    let childRows;
    if (expandCfg.key) {
      childRows = row[expandCfg.key] || [];
    } else if (expandCfg.from) {
      const all = (context && context[expandCfg.from]) || [];
      childRows = expandCfg.matchOn
        ? all.filter(function (r) { return r[expandCfg.matchOn] === row[expandCfg.matchOn]; })
        : all;
    } else {
      childRows = [];
    }
    if (!childRows.length) return null;
    const childOpts = { statusField: expandCfg.statusField, labels: labels };
    if (expandCfg.expand) childOpts.expand = buildExpand(expandCfg.expand, labels, context);
    return { rows: childRows, cols: expandCfg.columns || Object.keys(childRows[0]), opts: childOpts };
  };
}

// A row action rendered as a plain link + button — shared by the entity
// list's Actions column and a report's `rowActions` (below). `newTab` opens
// the link in a new tab/window (e.g. a report's Download/View links onto
// /attachments/:id/content) rather than navigating the app away.
function rowActionLink(a) {
  return el('a', { href: a.href, target: a.newTab ? '_blank' : null },
    el('button', { class: 'link small' }, a.label));
}

// Report tables: read-only — no per-row Edit/Delete — but a report may still
// declare `rowActions` (config.js) for link-only actions (e.g. the
// Attachments report's Download/View/Record links); `dataTable` renders them
// the same way the entity list's Actions column does, via `rowActionLink`.
// Foreign-key id columns render the referenced row's name (per
// FK_COLUMN_SOURCES), same as the entity lists. `expandCfg` (a REPORTS
// `expand` entry) turns each row into a expand-to-a-child-table row (see
// `buildExpand`); `context` is the whole multi-`tables` response object,
// needed only for an expand config's `from` sibling lookup.
async function dataTable(rows, columns, statusField, expandCfg, context, rowActions) {
  if (!rows || rows.length === 0) return el('div', { class: 'empty' }, 'No records.');
  // A `key` expand reads its children from a nested field on the row itself
  // (e.g. `parcels`) — excluded from the parent's own columns, whether
  // `columns` was explicit or (as every report passes) auto-derived from the
  // first row's keys.
  let cols = columns || Object.keys(rows[0]);
  if (expandCfg && expandCfg.key) cols = cols.filter(function (c) { return c !== expandCfg.key; });
  const labels = await columnLabelMaps(cols.concat(expandColumns(expandCfg)));
  const opts = { statusField: statusField, labels: labels };
  if (expandCfg) opts.expand = buildExpand(expandCfg, labels, context);
  if (rowActions) {
    opts.actions = function (row) {
      return el('td', { class: 'actions' }, rowActions(row).map(rowActionLink));
    };
  }
  return filterableTable(rows, cols, opts);
}

// ---- entity list view -------------------------------------------------
async function viewEntityList(entity) {
  setActiveNav(entity.slug);
  let rows = await api('GET', entity.api);
  if (entity.listFilter) rows = rows.filter(entity.listFilter);
  // Display-only derived columns (e.g. the trades list's Origin) are computed
  // client-side from fields the API already returns on the row.
  if (entity.deriveRow) rows.forEach(entity.deriveRow);

  const header = el('div', null, [
    el('h2', null, entity.title),
    el('p', { class: 'view-desc' }, entity.desc),
  ]);

  const toolbar = el('div', { class: 'toolbar' });
  // deleteOnly: rows are created by an operation and immutable — no New or
  // Edit, but Delete stays as the undo path (e.g. rights sales).
  if (!entity.readonly && !entity.deleteOnly) {
    toolbar.appendChild(el('a', { href: '#/e/' + entity.slug + '/new' },
      el('button', { class: 'primary' }, '+ New ' + entity.title.replace(/s$/, ''))));
  }

  const cols = entity.columns || (rows[0] ? Object.keys(rows[0]) : entity.keyFields.concat(entity.fields).map(function (f) { return f.name; }));
  let table;
  if (rows.length === 0) {
    table = el('div', { class: 'empty' }, 'No records yet.');
  } else {
    // Foreign-key id columns render the referenced row's name, not the raw
    // id — resolved by column name, so a column with no editable field
    // (e.g. income's reinvestment_trade_id) is named too.
    const labels = await columnLabelMaps(cols);
    const actions = entity.readonly ? null : function (row) {
      const keyPath = entity.keyFields.map(function (kf) { return row[kf.name]; }).join('/');
      const td = el('td', { class: 'actions' });
      // A row action is a link (`href`) or an API DELETE (`del` + `confirm`)
      // — e.g. income's Undo reinvest, which drives DELETE /income/:id/reinvest.
      (entity.rowActions ? entity.rowActions(row) : []).forEach(function (a) {
        if (a.del) {
          td.appendChild(el('button', {
            class: 'link small danger',
            onclick: async function () {
              if (!confirm(a.confirm)) return;
              try {
                await api('DELETE', a.del);
                toast(a.label + ': done.');
                viewEntityList(entity);
              } catch (e) {
                toast(e.message, true);
              }
            },
          }, a.label));
        } else {
          td.appendChild(rowActionLink(a));
        }
      });
      if (entity.attachOwner) {
        td.appendChild(el('a', { href: '#/attachments/' + entity.attachOwner + '/' + row.id },
          el('button', { class: 'link small' }, 'Attachments')));
      }
      if (!entity.deleteOnly) {
        td.appendChild(el('a', { href: '#/e/' + entity.slug + '/edit/' + keyPath },
          el('button', { class: 'link small' }, 'Edit')));
      }
      td.appendChild(el('button', {
        class: 'link small danger',
        onclick: function () { deleteEntity(entity, keyPath, row); },
      }, 'Delete'));
      return td;
    };
    table = filterableTable(rows, cols, { actions: actions, labels: labels });
  }

  setMain(el('div', null, [header, toolbar, table]));
}

async function deleteEntity(entity, keyPath, row) {
  if (!confirm('Delete this ' + entity.title.replace(/s$/, '') + '?')) return;
  try {
    await api('DELETE', entity.api + '/' + keyPath);
    toast('Deleted.');
    viewEntityList(entity);
  } catch (e) {
    toast(e.message, true);
  }
}

// ---- entity form view -------------------------------------------------
async function viewEntityForm(entity, keyParts) {
  setActiveNav(entity.slug);
  const editing = keyParts != null;
  // Each key part is encoded, not the joined string: the '/' between the parts
  // of a composite key (closing_prices is listing_id + date) is real path
  // structure, while the parts themselves came from the hash route.
  const existing = editing
    ? await api('GET', entity.api + '/' + keyParts.map(pathSeg).join('/'))
    : null;

  // Per-type field grouping: an entity with `fieldGroups` (keyed by the
  // `typeField` select's value) renders only the chosen type's group after
  // the common fields (those in no group); the group re-renders on type
  // change and the matching `typeDescs` entry scopes the description.
  const grouped = {};
  Object.keys(entity.fieldGroups || {}).forEach(function (t) {
    entity.fieldGroups[t].forEach(function (n) { grouped[n] = true; });
  });

  const form = el('form');
  // Key fields: editable on create (unless auto), disabled on edit.
  for (const kf of entity.keyFields) {
    if (kf.auto) continue;
    const val = existing ? existing[kf.name] : null;
    form.appendChild(await buildFieldInput(kf, val, editing));
  }
  for (const f of entity.fields) {
    if (grouped[f.name]) continue;
    const val = existing ? existing[f.name] : null;
    form.appendChild(await buildFieldInput(f, val, false));
  }

  if (entity.fieldGroups) {
    const fieldByName = {};
    entity.fields.forEach(function (f) { fieldByName[f.name] = f; });
    const typeDesc = el('p', { class: 'hint' });
    const typeWarn = el('p', { class: 'hint warn' });
    const typeSection = el('div');
    form.appendChild(typeDesc);
    form.appendChild(typeWarn);
    form.appendChild(typeSection);
    const typeSel = form.querySelector('[name="' + entity.typeField + '"]');
    // Values typed into a group survive flipping the type away and back:
    // each re-render first harvests the outgoing inputs into `draft`, which
    // wins over the stored row when the group renders again.
    const draft = {};
    // Renders are async (fk fields fetch their options); the sequence number
    // makes a stale render abandon itself instead of appending its fields
    // after a newer selection's.
    let renderSeq = 0;
    async function renderTypeFields() {
      typeSection.querySelectorAll('[name]').forEach(function (inp) {
        draft[inp.name] = inp.type === 'checkbox' ? inp.checked : inp.value;
      });
      const t = typeSel.value;
      typeDesc.textContent = (entity.typeDescs && entity.typeDescs[t])
        || (t ? '' : 'Choose an action type above to see its fields.');
      typeWarn.textContent = editing && t && t !== existing[entity.typeField]
        ? 'Saving as ' + t + ' clears the saved ' + existing[entity.typeField] + ' fields.'
        : '';
      // Common fields with a per-type label (e.g. the date) take it here.
      Object.keys(entity.typeLabels || {}).forEach(function (n) {
        const label = entity.typeLabels[n][t] || fieldByName[n].label;
        form.querySelector('label[for="f_' + n + '"]').textContent = label + (fieldByName[n].required ? ' *' : '');
      });
      const seq = ++renderSeq;
      const inputs = [];
      for (const n of entity.fieldGroups[t] || []) {
        const val = draft[n] !== undefined ? draft[n] : (existing ? existing[n] : null);
        inputs.push(await buildFieldInput(fieldByName[n], val, false));
      }
      if (seq !== renderSeq) return; // a newer selection rendered meanwhile
      typeSection.innerHTML = '';
      inputs.forEach(function (node) { typeSection.appendChild(node); });
    }
    typeSel.addEventListener('change', renderTypeFields);
    await renderTypeFields();
  }

  // Entity-specific form behaviour beyond what the field configs express
  // (e.g. the trades form's GST-inclusive brokerage toggle). A hook may
  // return submit-time extensions: transformBody(body) maps UI-only
  // controls onto the entity body before the PUT, and afterSave(idPath)
  // chains a follow-up call, returning the success toast text (null = the
  // default 'Saved.', '' = it already toasted, e.g. its own failure).
  const wired = entity.wireForm ? entity.wireForm(form, existing) : null;

  const actions = el('div', { class: 'form-actions' });
  actions.appendChild(el('button', { type: 'submit', class: 'primary' }, editing ? 'Save' : 'Create'));
  actions.appendChild(el('a', { href: '#/e/' + entity.slug }, el('button', { type: 'button' }, 'Cancel')));
  form.appendChild(actions);

  form.addEventListener('submit', async function (ev) {
    ev.preventDefault();
    try {
      // Resolve the key path.
      let keyVals;
      if (editing) {
        keyVals = entity.keyFields.map(function (kf) { return existing[kf.name]; });
      } else {
        keyVals = [];
        for (const kf of entity.keyFields) {
          keyVals.push(kf.auto ? await nextId(entity.api) : readFieldValue(kf, form));
        }
      }
      const body = {};
      entity.fields.forEach(function (f) {
        // A field whose type group is not selected has no input; it submits
        // as null, exactly as its blank input used to.
        body[f.name] = form.querySelector('[name="' + f.name + '"]') ? readFieldValue(f, form) : null;
      });
      if (wired && wired.transformBody) wired.transformBody(body);
      await api('PUT', entity.api + '/' + keyVals.join('/'), body);
      const msg = wired && wired.afterSave ? await wired.afterSave(keyVals.join('/')) : null;
      if (msg !== '') toast(msg || 'Saved.');
      location.hash = '#/e/' + entity.slug;
    } catch (e) {
      toast(e.message, true);
    }
  });

  setMain(el('div', null, [
    el('h2', null, (editing ? 'Edit ' : 'New ') + entity.title.replace(/s$/, '')),
    el('p', { class: 'view-desc' }, entity.desc),
    el('div', { class: 'card' }, form),
  ]));
}

// ---- Sells (trade + allocations, atomic) ------------------------------
const SELL_FIELDS = [
  dt('date', 'Trade date', { required: true }),
  dt('settlement_date', 'Settlement date', { optional: true, hint: 'Leave blank to auto-calculate.' }),
  fk('listing_id', 'Listing', 'listings', { required: true }),
  dec('average_price', 'Average price', { required: true, default: '' }),
  dec('quantity', 'Quantity', { required: true, default: '' }),
  fk('currency', 'Currency', 'currencies', { required: true, encode: 'string' }),
  dec('brokerage', 'Brokerage'),
  bool('brokerage_includes_gst', 'Brokerage includes GST', { hint: 'Tick when the statement quotes brokerage GST-inclusive; the GST component (1/11, rounded to the cent) is derived automatically.' }),
  dec('gst_on_brokerage', 'GST on brokerage'),
  fk('brokerage_currency', 'Brokerage currency', 'currencies', { required: true, encode: 'string' }),
  dec('fx_rate', 'Manual FX rate', { default: '1' }),
  dec('spot_fx_rate', 'Spot FX rate override', { optional: true, default: '', hint: 'Optional deliberate transaction-date spot rate (foreign units per AUD): when set it wins over the monthly RBA rate everywhere this trade converts to AUD. Use for a one-off purchase/sale of a large foreign asset (QC 18020); leave blank for the monthly default. Non-AUD trades only.' }),
  txt('contract_note_ref', 'Contract note ref', { optional: true }),
  dec('statement_total', 'Statement total', { optional: true, default: '', hint: 'Optional cross-check in the brokerage currency: quantity × price − brokerage − GST (net proceeds). Rejected if it does not reconcile.' }),
  fk('holding_account_id', 'Holding account', 'holdingAccounts', { required: true, default: '1', hint: 'Allocations may only consume parcels held in this account.' }),
];

async function viewSellsList() {
  setActiveNav('sells');
  const sells = (await api('GET', '/trades')).filter(function (t) { return t.trade_type === 'Sell'; });
  // Transfer-out and other operation-created Sells are labelled like the
  // Trades list, so a price-0 transfer leg never reads as a real disposal.
  sells.forEach(function (t) { t.origin = tradeOrigin(t); });
  const cols = ['id', 'origin', 'date', 'settlement_date', 'listing_id', 'average_price', 'quantity', 'currency', 'statement_total', 'holding_account_id'];
  const toolbar = el('div', { class: 'toolbar' }, [
    el('a', { href: '#/sells/new' }, el('button', { class: 'primary' }, '+ New Sell')),
  ]);
  let table;
  if (sells.length === 0) {
    table = el('div', { class: 'empty' }, 'No sell trades yet.');
  } else {
    table = filterableTable(sells, cols, {
      labels: await columnLabelMaps(cols),
      actions: function (row) {
        return el('td', { class: 'actions' }, [
          el('a', { href: '#/attachments/trade_id/' + row.id }, el('button', { class: 'link small' }, 'Attachments')),
          el('a', { href: '#/sells/edit/' + row.id }, el('button', { class: 'link small' }, 'Edit')),
          el('button', {
            class: 'link small danger',
            onclick: async function () {
              if (!confirm('Delete this Sell and its allocations?')) return;
              try { await api('DELETE', '/sells/' + row.id); toast('Deleted.'); viewSellsList(); }
              catch (e) { toast(e.message, true); }
            },
          }, 'Delete'),
        ]);
      },
    });
  }
  setMain(el('div', null, [
    el('h2', null, 'Sells'),
    el('p', { class: 'view-desc' }, 'Sell trades, each persisted atomically with parcel allocations that must sum exactly to the sell quantity.'),
    sellForesightLinks(),
    toolbar, table,
  ]));
}

// Foresight reports surfaced in the Sell flow: what a contemplated sale
// would do to franking credits, and the wash-sale pattern a loss Sell plus
// nearby Buy creates.
function sellForesightLinks() {
  return el('p', { class: 'hint' }, [
    'Before selling: ',
    el('a', { href: '#/r/franking-what-if' }, 'check the franking credits a sale would put at risk'),
    ' · ',
    el('a', { href: '#/r/wash-sales' }, 'review wash-sale flags'),
    ' · ',
    el('a', { href: '#/r/parcel-optimiser' }, 'compare parcel selections'),
    '.',
  ]);
}

async function viewSellForm(id) {
  setActiveNav('sells');
  const editing = id != null;
  const existing = editing ? await api('GET', '/trades/' + pathSeg(id)) : null;
  let existingAllocs = [];
  if (editing) {
    existingAllocs = (await api('GET', '/parcel_allocations')).filter(function (a) { return a.sale_trade_id === Number(id); });
  }

  const form = el('form');
  for (const f of SELL_FIELDS) {
    form.appendChild(await buildFieldInput(f, existing ? existing[f.name] : null, false));
  }
  wireGstBrokerage(form);

  // Allocations: the shared editor, pre-filled with the existing rows and
  // narrowed to open parcels matching the chosen listing + holding account —
  // the two things the server itself requires a parcel to match
  // (`entities::sell`) — re-filtered live as either field changes.
  const openParcels = await loadOptions('openParcels');
  function validParcelOptions() {
    const listingId = form.querySelector('[name="listing_id"]').value;
    const accountId = form.querySelector('[name="holding_account_id"]').value;
    return openParcels.filter(function (p) {
      return String(p.listing_id) === listingId && String(p.holding_account_id) === accountId;
    });
  }
  const allocEditor = allocationEditor(validParcelOptions(), existingAllocs, {
    hint: 'Allocations must sum exactly to the sell quantity. Each parcel must be a Buy/DRP with enough remaining units, in the chosen listing and holding account.',
  });
  form.querySelector('[name="listing_id"]').addEventListener('change', function () { allocEditor.setOptions(validParcelOptions()); });
  form.querySelector('[name="holding_account_id"]').addEventListener('change', function () { allocEditor.setOptions(validParcelOptions()); });

  const actions = el('div', { class: 'form-actions' }, [
    el('button', { type: 'submit', class: 'primary' }, editing ? 'Save Sell' : 'Create Sell'),
    el('a', { href: '#/sells' }, el('button', { type: 'button' }, 'Cancel')),
  ]);

  form.appendChild(allocEditor.section);
  form.appendChild(actions);
  form.addEventListener('submit', async function (ev) {
    ev.preventDefault();
    try {
      const body = {};
      SELL_FIELDS.forEach(function (f) { body[f.name] = readFieldValue(f, form); });
      body.allocations = allocEditor.read();
      const sellId = editing ? Number(id) : await nextId('/trades');
      await api('PUT', '/sells/' + sellId, body);
      toast('Sell saved.');
      location.hash = '#/sells';
    } catch (e) {
      toast(e.message, true);
    }
  });

  setMain(el('div', null, [
    el('h2', null, editing ? 'Edit Sell' : 'New Sell'),
    sellForesightLinks(),
    el('div', { class: 'card' }, form),
  ]));
}

// ---- Transfers between holding accounts --------------------------------
// A transfer moves units of one listing between two holding accounts of the
// same owner — not a CGT event. PUT /transfers/:id records and executes it
// atomically (a price-0 transfer-out Sell consuming the chosen parcels in
// the source account plus transfer-in Buys in the destination carrying each
// parcel's remaining cost base and acquisition date); DELETE removes the
// whole group and restores the pre-transfer holding. A transfer is
// immutable — delete and re-transfer to change it.
async function viewTransfersList() {
  setActiveNav('transfers');
  const rows = await api('GET', '/transfers');
  const cols = ['id', 'listing_id', 'date', 'from_account_id', 'to_account_id'];
  const toolbar = el('div', { class: 'toolbar' }, [
    el('a', { href: '#/transfers/new' }, el('button', { class: 'primary' }, '+ New Transfer')),
  ]);
  let table;
  if (rows.length === 0) {
    table = el('div', { class: 'empty' }, 'No transfers yet.');
  } else {
    table = filterableTable(rows, cols, {
      labels: await columnLabelMaps(cols),
      actions: function (row) {
        return el('td', { class: 'actions' }, [
          el('button', {
            class: 'link small danger',
            onclick: async function () {
              if (!confirm('Delete this transfer and restore the pre-transfer holding?')) return;
              try { await api('DELETE', '/transfers/' + row.id); toast('Deleted.'); viewTransfersList(); }
              catch (e) { toast(e.message, true); }
            },
          }, 'Delete'),
        ]);
      },
    });
  }
  setMain(el('div', null, [
    el('h2', null, 'Transfers'),
    el('p', { class: 'view-desc' }, entityBySlug.transfers.desc + ' Each parcel keeps its cost base and acquisition date; deleting a transfer restores the pre-transfer holding.'),
    toolbar, table,
  ]));
}

async function viewTransferForm() {
  setActiveNav('transfers');
  const form = el('form');
  const fields = [
    fk('listing_id', 'Listing', 'listings', { required: true }),
    dt('date', 'Transfer date', { required: true }),
    fk('from_account_id', 'From holding account', 'holdingAccounts', { required: true }),
    fk('to_account_id', 'To holding account', 'holdingAccounts', { required: true }),
  ];
  for (const f of fields) form.appendChild(await buildFieldInput(f, null, false));

  // The parcels to move — the same shape as a Sell's allocations (the shared
  // editor); partial parcels allowed. Narrowed to open parcels matching the
  // chosen listing + source account — the two things the server itself
  // requires a parcel to match (`entities::transfer`) — re-filtered live as
  // either field changes.
  const openParcels = await loadOptions('openParcels');
  function validParcelOptions() {
    const listingId = form.querySelector('[name="listing_id"]').value;
    const accountId = form.querySelector('[name="from_account_id"]').value;
    return openParcels.filter(function (p) {
      return String(p.listing_id) === listingId && String(p.holding_account_id) === accountId;
    });
  }
  const allocEditor = allocationEditor(validParcelOptions(), null, {
    heading: 'Parcels to move',
    hint: 'Each parcel must be a Buy/DRP of the chosen listing held in the source account, with enough remaining units. Moved units keep their cost base and acquisition date.',
    parcelLabel: 'Parcel to move',
    qtyLabel: 'Units to move',
    addLabel: '+ Add parcel',
  });
  form.appendChild(allocEditor.section);

  // The optional crypto network fee: the source parcels disposed of to cover
  // an on-chain transfer fee. Unlike the move, these units are a CGT event
  // (ATO: a holding reducing to cover a network fee is a disposal) — they
  // show up in the gains reports at the per-unit market value below.
  const feeEditor = allocationEditor(validParcelOptions(), null, {
    heading: 'Network fee (optional)',
    hint: 'Leave empty for no fee. Otherwise: the source parcels disposed of to pay the on-chain network fee. These units are a CGT disposal (not moved) at the per-unit market value below — they surface a capital gain/loss in the realised-gains report.',
    parcelLabel: 'Fee parcel',
    qtyLabel: 'Fee units',
    addLabel: '+ Add fee parcel',
  });
  form.appendChild(feeEditor.section);
  form.querySelector('[name="listing_id"]').addEventListener('change', function () {
    allocEditor.setOptions(validParcelOptions());
    feeEditor.setOptions(validParcelOptions());
  });
  form.querySelector('[name="from_account_id"]').addEventListener('change', function () {
    allocEditor.setOptions(validParcelOptions());
    feeEditor.setOptions(validParcelOptions());
  });
  const feePrice = field('fee_market_price', 'Fee market value per unit (AUD)', 'decimal',
    { required: false, hint: "The fee crypto's market value per unit at the transfer date — the disposal's capital proceeds. Required only when a fee parcel is set." });
  form.appendChild(await buildFieldInput(feePrice, null, false));

  form.appendChild(el('div', { class: 'form-actions' }, [
    el('button', { type: 'submit', class: 'primary' }, 'Transfer'),
    el('a', { href: '#/transfers' }, el('button', { type: 'button' }, 'Cancel')),
  ]));
  form.addEventListener('submit', async function (ev) {
    ev.preventDefault();
    try {
      const body = {};
      fields.forEach(function (f) { body[f.name] = readFieldValue(f, form); });
      body.allocations = allocEditor.read();
      const feeAllocs = feeEditor.read();
      if (feeAllocs.length) {
        body.fee_allocations = feeAllocs;
        body.fee_market_price = readFieldValue(feePrice, form);
      }
      const result = await api('PUT', '/transfers/' + await nextId('/transfers'), body);
      const n = result && result.transfer_ins ? result.transfer_ins.length : 0;
      const listingName = await listingNamer();
      const acct = (await fkLabelMaps({ a: 'holdingAccounts' })).a || {};
      const acctName = function (id) { return acct[id] || ('account ' + id); };
      const feeNote = result && result.fee_sale
        ? ' Network fee disposed as sell #' + result.fee_sale.id + ' (a CGT event).' : '';
      toast('Transferred ' + n + ' parcel(s) of ' + listingName(body.listing_id)
        + ' from ' + acctName(body.from_account_id) + ' to ' + acctName(body.to_account_id)
        + ' (transfer-out sell #' + (result && result.sell ? result.sell.id : '?') + ').' + feeNote);
      location.hash = '#/transfers';
    } catch (e) {
      toast(e.message, true);
    }
  });
  setMain(el('div', null, [
    el('h2', null, 'New Transfer'),
    el('p', { class: 'view-desc' }, 'Moves units between two holding accounts of the same owner — not a CGT event: nothing appears in the gains reports and each moved parcel keeps its cost base and acquisition date. An optional crypto network fee, paid in the transferred crypto, is the exception: those units are disposed of at market value and do surface a capital gain/loss.'),
    el('div', { class: 'card' }, form),
  ]));
}

// ---- generic post-action view ------------------------------------------
// Renders one ACTIONS entry: fetch the owning record, render the action's
// fields (and the shared allocation editor when it takes parcel
// allocations), POST the body to the action endpoint, toast the result, and
// return to the owner's list. Null field values are omitted so server-side
// defaults apply; the no-field confirm-only actions POST without a body.
async function viewAction(action, id) {
  setActiveNav(action.nav);
  const owner = await api('GET', action.ownerApi + '/' + pathSeg(id));
  // Action descriptions name the listings they touch rather than printing
  // raw ids; unknown/null ids fall back to the old "listing N" wording.
  const listingName = await listingNamer();
  const fields = typeof action.fields === 'function' ? action.fields(owner) : action.fields;
  const form = el('form');
  for (const f of fields) form.appendChild(await buildFieldInput(f, null, false));
  // Parcel choices, when the action takes allocations: narrowed by its
  // `allocations.filter` (listing fixed to the owning record unless
  // `listingField` names a field to read live; `accountField`/`beforeOwnerDate`
  // add the other constraints the server itself enforces for that action) —
  // re-filtered live as any watched field changes.
  let allocEditor = null;
  if (action.allocations) {
    const filter = action.allocations.filter || {};
    const parcels = await loadOptions(filter.source === 'buy' ? 'buyParcels' : 'openParcels');
    function currentParcelOptions() {
      const listingId = filter.listingField
        ? form.querySelector('[name="' + filter.listingField + '"]').value
        : String(owner.listing_id);
      return parcels.filter(function (p) {
        if (String(p.listing_id) !== listingId) return false;
        if (filter.accountField
          && String(p.holding_account_id) !== form.querySelector('[name="' + filter.accountField + '"]').value) return false;
        if (filter.beforeOwnerDate && !(p.date < owner.date)) return false;
        return true;
      });
    }
    allocEditor = allocationEditor(currentParcelOptions(), null, action.allocations);
    form.appendChild(allocEditor.section);
    [filter.listingField, filter.accountField].forEach(function (name) {
      if (!name) return;
      form.querySelector('[name="' + name + '"]').addEventListener('change', function () { allocEditor.setOptions(currentParcelOptions()); });
    });
  }
  form.appendChild(el('div', { class: 'form-actions' }, [
    el('button', { type: 'submit', class: 'primary' }, action.submit),
    el('a', { href: action.cancel }, el('button', { type: 'button' }, 'Cancel')),
  ]));
  form.addEventListener('submit', async function (ev) {
    ev.preventDefault();
    try {
      let body;
      if (fields.length || allocEditor) {
        body = {};
        fields.forEach(function (f) {
          const v = readFieldValue(f, form);
          if (v != null) body[f.name] = v;
        });
        if (allocEditor) body.allocations = allocEditor.read();
      }
      // An action may gate its POST behind a confirmation built from the
      // assembled body — e.g. the AMMA generation's preview, which asks the
      // server what it would create and shows it before anything is written.
      // Answering false cancels the submit, leaving the form as it was.
      if (action.confirm && !await action.confirm(action.post(id), body)) return;
      const result = await api('POST', action.post(id), body);
      toast(action.toast(result, listingName, owner));
      location.hash = action.cancel;
    } catch (e) {
      toast(e.message, true);
    }
  });
  setMain(el('div', null, [
    el('h2', null, action.title(id, owner, listingName)),
    el('p', { class: 'view-desc' }, action.desc(owner, listingName)),
    el('div', { class: 'card' }, form),
  ]));
}

// ---- document attachments ---------------------------------------------
// Reached from a Trade / Income / AMMA / ESS statement / Interest income
// row's "Attachments" action. Lists the activity's attachments (metadata only
// — never the blob), uploads a new file via multipart/form-data
// (POST /attachments), and links each row to its download
// (GET /attachments/:id/content). The owner field name (trade_id / income_id /
// amma_statement_id / ess_statement_id / interest_income_id) is carried in
// the route.
const ATTACH_OWNER = {
  trade_id: { noun: 'trade', api: '/trades', name: function (o, listing) { return describeTrade(o, listing); } },
  income_id: { noun: 'distribution', api: '/income', name: function (o, listing) { return listing(o.listing_id) + ' on ' + o.date_paid; } },
  amma_statement_id: { noun: 'AMMA statement', api: '/amma_statements', name: function (o, listing) { return listing(o.listing_id) + ' FY' + o.tax_year_end_date; } },
  ess_statement_id: { noun: 'ESS statement', api: '/ess_statements', name: function (o, listing) { return listing(o.listing_id) + ' taxing point ' + o.taxing_point_date; } },
  interest_income_id: { noun: 'interest income', api: '/interest_income', name: function (o) { return (o.source ? o.source + ' ' : '') + 'on ' + o.date_paid; } },
  corporate_action_id: { noun: 'corporate action', api: '/corporate_actions', name: function (o, listing) { return o.action_type + ' ' + listing(o.listing_id) + ' on ' + o.date; } },
};

async function viewAttachments(ownerField, ownerId) {
  // A trade's view also lists the documents of the record the trade was
  // created from (a DRP trade's funding distribution, a buy-back Sell's
  // dividend income row, an ESS vest Buy's annual statement): the server
  // traverses the provenance link when include_linked is set. Ownership is
  // unchanged — a linked row is labelled with its owning record, downloads
  // from here, and is deleted from that record's own Attachments view.
  const isTrade = ownerField === 'trade_id';
  const rows = await api('GET', '/attachments?' + ownerField + '=' + encodeURIComponent(ownerId)
    + (isTrade ? '&include_linked=true' : ''));
  // Name the owning activity (e.g. "DRP 45 XASX:VDHG on 2024-12-20"), not a
  // bare "trade #5"; the id stays as secondary detail.
  const ownerSpec = ATTACH_OWNER[ownerField];
  let ownerName = (ownerSpec ? ownerSpec.noun : 'activity') + ' #' + ownerId;
  if (ownerSpec) {
    try {
      const owner = await api('GET', ownerSpec.api + '/' + pathSeg(ownerId));
      ownerName = ownerSpec.noun + ' ' + ownerSpec.name(owner, await listingNamer()) + ' (#' + ownerId + ')';
    } catch (e) { /* fall back to the noun + id wording */ }
  }

  // On a trade's view, a row whose trade_id is null is a linked document —
  // find which owner field carries it.
  function linkedOwner(row) {
    if (!isTrade || row.trade_id !== null) return null;
    for (const field in ATTACH_OWNER) {
      if (field !== 'trade_id' && row[field] !== null && row[field] !== undefined) {
        return { field: field, id: row[field] };
      }
    }
    return null;
  }
  const anyLinked = rows.some(function (r) { return linkedOwner(r) !== null; });
  rows.forEach(function (r) {
    const link = linkedOwner(r);
    r.attached_to = link ? ATTACH_OWNER[link.field].noun + ' #' + link.id + ' (linked)' : 'this trade';
  });

  // checksum is stored integrity metadata, not user-facing — not a column
  // here. The attached-to column appears only when a linked document is
  // present, so the plain single-owner view stays uncluttered.
  const cols = ['id', 'filename', 'content_type', 'byte_size', 'uploaded_at']
    .concat(anyLinked ? ['attached_to'] : []);

  const container = el('div');
  function refresh() { viewAttachments(ownerField, ownerId); }

  let table;
  if (rows.length === 0) {
    table = el('div', { class: 'empty' }, 'No attachments yet.');
  } else {
    table = filterableTable(rows, cols, {
      actions: function (row) {
        const link = linkedOwner(row);
        const actions = [
          el('a', { href: apiUrl('/attachments/' + row.id + '/content'), target: '_blank' },
            el('button', { class: 'link small' }, 'Download')),
        ];
        if (link) {
          // Delete stays on the owning record's view — link to it instead.
          actions.push(el('a', { href: '#/attachments/' + link.field + '/' + link.id },
            el('button', { class: 'link small' }, "Owner's attachments")));
        } else {
          actions.push(el('button', {
            class: 'link small danger',
            onclick: async function () {
              if (!confirm('Delete this attachment?')) return;
              try { await api('DELETE', '/attachments/' + row.id); toast('Deleted.'); refresh(); }
              catch (e) { toast(e.message, true); }
            },
          }, 'Delete'));
        }
        return el('td', { class: 'actions' }, actions);
      },
    });
  }

  // Upload form: a single file input posted as multipart/form-data. The
  // browser sets the multipart boundary and the part's Content-Type; the
  // server validates it against the allowlist (pdf/png/jpeg/txt) and the
  // 25 MB cap.
  const fileInput = el('input', { type: 'file', name: 'file', required: true, accept: '.pdf,.png,.jpg,.jpeg,.txt' });
  const uploadForm = el('form', { class: 'card' }, [
    el('div', { class: 'field' }, [el('label', null, 'Add a file'), fileInput]),
    el('p', { class: 'hint' }, 'Accepted: PDF, PNG, JPEG, TXT. Max 25 MB. Stored in the database.'),
    el('div', { class: 'form-actions' }, [el('button', { type: 'submit', class: 'primary' }, 'Upload')]),
  ]);
  uploadForm.addEventListener('submit', async function (ev) {
    ev.preventDefault();
    if (!fileInput.files || fileInput.files.length === 0) { toast('Choose a file first.', true); return; }
    try {
      const fd = new FormData();
      fd.append(ownerField, String(ownerId));
      fd.append('file', fileInput.files[0]);
      const res = await fetch(apiUrl('/attachments'), { method: 'POST', body: fd });
      if (!res.ok) {
        let detail = '';
        try { detail = (await res.text()).trim(); } catch (e) { /* ignore */ }
        throw new Error('HTTP ' + res.status + (detail ? ': ' + detail : ''));
      }
      toast('Uploaded.');
      refresh();
    } catch (e) {
      toast(e.message, true);
    }
  });

  container.appendChild(el('h2', null, 'Attachments'));
  container.appendChild(el('p', { class: 'view-desc' },
    'Files attached to ' + ownerName + '. Stored in the database.'
    + (anyLinked ? ' Rows marked (linked) belong to the record this trade was created from; uploads here attach to the trade, and a linked document is deleted from its own record\'s view.' : '')));
  container.appendChild(uploadForm);
  container.appendChild(table);
  setMain(container);
}

// ---- health banner ------------------------------------------------------
// Cross-view strip driven by GET /reports/health: stale closing prices or RBA
// FX rates (the staleness thresholds live server-side), any job whose latest
// run failed (linking to Jobs), any listing with errored closing-price
// rows — a wrong/renamed/delisted provider symbol otherwise only shows up
// indirectly as a missing snapshot (linking to Closing Prices, where the
// backfill action re-fetches once the symbol is fixed) — any duplicated
// corporate action, whose effect is silently compounded (linking to Corporate
// Actions, where the surplus row is deleted), any fund-year with two AMMA
// statements for one holding account, counted twice in the income, gains and
// cost-base figures alike (linking to AMMA Statements), and any distribution
// entered twice — identical amounts, same listing, account and payment date —
// which declares the dividend and its franking credits twice (linking to
// Income), and the same double-entry on the two listing-less sides of the tax
// summary: an identical interest credit (linking to Interest Income) or an
// identical deductible expense (linking to Investment Expenses), each of which
// is counted once per row, and the same double-entry on the employee-share
// scheme side — an identical ESS statement (linking to ESS Statements), whose
// discount is assessed and whose parcel is vested once per statement — and one
// alert that is a date pattern rather than a double entry: a sale of ESS-vested
// shares within 30 days after the taxing point, where the ATO's 30-day rule
// re-measures the discount at the sale proceeds, can move it into the next
// financial year, and leaves no separate capital gain (also linking to ESS
// Statements, where the employer's amended statement replaces the original).
// Refreshed on every
// route render, so fixing the cause clears it on the next navigation. A
// failing health fetch hides the banner rather than breaking the app.
async function refreshHealthBanner() {
  const banner = document.getElementById('health-banner');
  try {
    const h = await api('GET', '/reports/health');
    const problems = [];
    if (h.prices_stale) {
      problems.push('Closing prices are stale — latest is ' + h.latest_price_date
        + '; check the price-import job.');
    }
    if (h.fx_stale) {
      problems.push('RBA FX rates are stale — latest month is ' + h.latest_fx_month
        + '; check the rba-fx-import job.');
    }
    (h.failed_jobs || []).forEach(function (j) {
      problems.push("Job '" + j.name + "' failed" + (j.error ? ': ' + j.error : '') + '.');
    });
    const erroredPrices = h.errored_prices || [];
    if (erroredPrices.length > 0) {
      problems.push(erroredPrices.length + ' listing(s) have errored closing prices ('
        + erroredPrices.map(function (r) { return r.ticker; }).join(', ') + ').');
    }
    const duplicateActions = h.duplicate_actions || [];
    duplicateActions.forEach(function (d) {
      problems.push(d.action_count + ' ' + d.action_type + ' actions on ' + d.ticker + ' dated '
        + d.date + ' (ids ' + d.action_ids.join(', ')
        + ') — each is applied separately; delete the duplicate unless both are real.');
    });
    const duplicateAmma = h.duplicate_amma_statements || [];
    duplicateAmma.forEach(function (d) {
      problems.push(d.statement_count + ' AMMA statements for ' + d.ticker + ' FY'
        + (d.tax_year - 1) + '/' + String(d.tax_year).slice(-2) + ' in one holding account (ids '
        + d.statement_ids.join(', ')
        + ') — every figure is counted once per statement; delete the superseded one unless both are real.');
    });
    const duplicateIncome = h.duplicate_income || [];
    duplicateIncome.forEach(function (d) {
      problems.push(d.income_count + ' identical income rows of ' + d.gross_amount + ' '
        + d.currency + ' for ' + d.ticker + ' paid ' + d.date_paid + ' into one holding account (ids '
        + d.income_ids.join(', ')
        + ') — the dividend and its franking credits are counted once per row; delete the duplicate unless both are real.');
    });
    const duplicateInterest = h.duplicate_interest || [];
    duplicateInterest.forEach(function (d) {
      problems.push(d.interest_count + ' identical interest rows of ' + d.amount + ' ' + d.currency
        + (d.source ? ' from ' + d.source : '') + ' credited ' + d.date_paid + ' (ids '
        + d.interest_ids.join(', ')
        + ') — the year’s gross interest counts each row; delete the duplicate unless both are real.');
    });
    const duplicateExpenses = h.duplicate_expenses || [];
    duplicateExpenses.forEach(function (d) {
      problems.push(d.expense_count + ' identical ' + d.expense_type + ' expenses of ' + d.amount
        + ' ' + d.currency + (d.ticker ? ' for ' + d.ticker : '') + ' incurred ' + d.date_incurred
        + ' (ids ' + d.expense_ids.join(', ')
        + ') — the deduction is claimed once per row; delete the duplicate unless both are real.');
    });
    const duplicateEss = h.duplicate_ess_statements || [];
    duplicateEss.forEach(function (d) {
      problems.push(d.statement_count + ' identical ESS statements for ' + d.ticker + ' vesting '
        + d.quantity + ' shares at ' + d.taxing_point_date + ' with a ' + d.discount_total + ' '
        + d.currency + ' discount (ids ' + d.statement_ids.join(', ')
        + ') — the discount is assessed and the parcel vested once per statement;'
        + ' delete the superseded one unless both are real.');
    });
    const ess30Day = h.ess_30_day_rule || [];
    ess30Day.forEach(function (d) {
      problems.push('Sale of ' + d.units_sold + ' ' + d.ticker + ' ESS shares on ' + d.sale_date
        + ' is ' + d.days_after + ' day(s) after the taxing point of statement ' + d.ess_statement_id
        + ' (' + d.taxing_point_date + ') — the 30-day rule moves the taxing point to the sale date,'
        + ' so the ' + d.statement_discount + ' ' + d.currency + ' discount is re-measured at the'
        + ' sale proceeds'
        + (d.disposal_tax_year === d.statement_tax_year
          ? '' : ' and moves from FY' + d.statement_tax_year + ' to FY' + d.disposal_tax_year)
        + ', and there is no separate capital gain. Enter the employer’s amended statement over the'
        + ' original.');
    });
    if (problems.length === 0) {
      banner.hidden = true;
      banner.innerHTML = '';
      return;
    }
    banner.innerHTML = '';
    banner.appendChild(el('span', null, '⚠ ' + problems.join(' ')));
    banner.appendChild(el('a', { href: '#/jobs' }, 'Open Jobs →'));
    if (erroredPrices.length > 0) {
      banner.appendChild(el('a', { href: '#/prices' }, 'Open Closing Prices →'));
    }
    if (duplicateActions.length > 0) {
      banner.appendChild(el('a', { href: '#/e/corporate_actions' }, 'Open Corporate Actions →'));
    }
    if (duplicateAmma.length > 0) {
      banner.appendChild(el('a', { href: '#/e/amma_statements' }, 'Open AMMA Statements →'));
    }
    if (duplicateIncome.length > 0) {
      banner.appendChild(el('a', { href: '#/e/income' }, 'Open Income →'));
    }
    if (duplicateInterest.length > 0) {
      banner.appendChild(el('a', { href: '#/e/interest_income' }, 'Open Interest Income →'));
    }
    if (duplicateExpenses.length > 0) {
      banner.appendChild(el('a', { href: '#/e/investment_expenses' }, 'Open Investment Expenses →'));
    }
    if (duplicateEss.length > 0 || ess30Day.length > 0) {
      banner.appendChild(el('a', { href: '#/e/ess_statements' }, 'Open ESS Statements →'));
    }
    banner.hidden = false;
  } catch (e) {
    banner.hidden = true;
  }
}

// ---- maintenance jobs -------------------------------------------------
const JOB_DESC = {
  'backup': 'Copy the database to a dated backup file beside it (skipped if today\'s already exists).',
  'rba-fx-import': 'Fetch the RBA F11 monthly FX rates and import any new months.',
  'mic-import': 'Fetch and refresh the ISO 10383 MIC registry.',
  'currency-import': 'Fetch ISO 4217 fiat and ISO 24165 token currencies.',
  'price-import': 'Store the closing price of every trading day in the last 7 whose row is missing or errored, for every held listing (days already stored ok are never re-fetched, so runs are idempotent and outages self-heal).',
  'report-snapshot': 'Store the price-dependent reports\' results for every missing date in the last 14 days up to the latest the whole portfolio can be valued at with final prices, regenerating stale or provisional ones; a blocked date is skipped (reported) and retried next run.',
};

async function viewJobs() {
  setActiveNav('jobs');
  // GET /jobs returns each registered job with its last run (started/finished
  // timestamps, success flag, error text), or nulls if it has never run.
  const jobs = await api('GET', '/jobs');
  const rows = jobs.map(function (j) {
    return {
      job: j.name,
      description: JOB_DESC[j.name] || '',
      last_run: j.last_finished_at || '',
      status: j.last_started_at == null ? 'never' : (j.last_success ? 'ok' : 'failed'),
      error: j.last_error || '',
      _runs: j.runs || [],
    };
  });
  const cols = ['job', 'description', 'last_run', 'status', 'error'];
  const table = filterableTable(rows, cols, {
    statusField: 'status',
    // Expand a job to its stored run history (the server keeps a bounded
    // number of recent runs per job), so a flapping job — an intermittent
    // failure that later succeeded — is diagnosable from here.
    expand: function (row) {
      if (!row._runs.length) return null;
      const runs = row._runs.map(function (r) {
        return {
          started_at: r.started_at,
          finished_at: r.finished_at,
          status: r.success ? 'ok' : 'failed',
          error: r.error || '',
        };
      });
      return {
        rows: runs,
        cols: ['started_at', 'finished_at', 'status', 'error'],
        opts: { statusField: 'status' },
      };
    },
    actions: function (row) {
      const btn = el('button', { class: 'small primary' }, 'Run now');
      btn.addEventListener('click', async function () {
        btn.disabled = true;
        btn.textContent = 'Running…';
        try {
          await api('POST', '/jobs/' + row.job);
          toast("Job '" + row.job + "' completed.");
        } catch (e) {
          toast(e.message, true);
        } finally {
          // Reload so the table reflects the freshly recorded last run.
          viewJobs();
        }
      });
      return el('td', { class: 'actions' }, btn);
    },
  });
  setMain(el('div', null, [
    el('h2', null, 'Jobs'),
    el('p', { class: 'view-desc' }, 'Trigger scheduled maintenance jobs on demand, and see when each last ran (and any error). Expand a job to see its recent run history. Each also runs automatically on its cron schedule; running here is for retries or missed runs.'),
    table,
  ]));
}

// ---- closing prices -----------------------------------------------------
// Stored daily closing prices (incl. errored fetches) with the two on-demand
// actions: re-fetch one (listing, day) — typically to replace an errored row
// — and backfill a listing over a date range. Collection otherwise runs on
// the price-import job's schedule.
// 'YYYY-MM-DD' minus `days` calendar days, computed in UTC to avoid
// timezone-boundary drift.
function isoDateMinusDays(dateStr, days) {
  const d = new Date(dateStr + 'T00:00:00Z');
  d.setUTCDate(d.getUTCDate() - days);
  return d.toISOString().slice(0, 10);
}

async function viewClosingPrices() {
  setActiveNav('closing_prices');
  const listings = await api('GET', '/listings');
  const byId = {};
  listings.forEach(function (l) { byId[l.id] = l; });
  const prices = await api('GET', '/closing_prices');
  // Listings with errored rows (health.errored_prices) — a wrong, renamed, or
  // delisted provider symbol otherwise only shows up indirectly as a missing
  // snapshot. Surfaced here with a Backfill action pre-filling the form
  // below over a generous window ending at the latest errored date.
  const health = await api('GET', '/reports/health').catch(function () { return {}; });
  const erroredListings = health.errored_prices || [];
  // Held days no fetch was ever attempted for (health.unpriced_days) — the
  // missing-row counterpart, typically a trade entered long after the fact.
  const unpricedListings = health.unpriced_days || [];
  const rows = prices.map(function (p) {
    const l = byId[p.listing_id];
    return {
      // The surrogate key, shown so a row can be looked up on the Row History
      // screen (which asks for the record's id).
      id: p.id,
      listing: l ? ((l.exchange_mic || 'Crypto') + ':' + l.ticker) : ('listing ' + p.listing_id),
      date: p.price_date,
      price: p.price == null ? '' : p.price,
      currency: l ? l.currency : '',
      source: p.source,
      status: p.status,
      origin: p.origin,
      sourced_from: p.sourced_from || '',
      reason: p.reason || '',
      error: p.error || '',
      fetched_at: p.fetched_at,
      _listing_id: p.listing_id,
    };
  });
  const cols = ['id', 'listing', 'date', 'price', 'currency', 'source', 'status', 'origin',
    'sourced_from', 'reason', 'error', 'fetched_at'];
  const table = filterableTable(rows, cols, {
    statusField: 'status',
    actions: function (row) {
      // A hand-entered price is a deliberate correction for a day the
      // provider got wrong or cannot serve, so the provider never takes the
      // day back: the server refuses a re-fetch (422) and there is nothing to
      // discard (only errored rows are deletable). It is changed by entering
      // another manual price below.
      if (row.origin === 'manual') {
        return el('td', { class: 'actions' },
          el('span', { class: 'hint' }, 'Manual — re-enter to change'));
      }
      const btn = el('button', { class: 'small' }, 'Re-fetch');
      btn.addEventListener('click', async function () {
        btn.disabled = true;
        btn.textContent = 'Fetching…';
        try {
          const stored = await api('POST', '/closing_prices/fetch',
            { listing_id: row._listing_id, price_date: row.date });
          if (stored.status === 'ok') toast('Stored ' + stored.price + ' for ' + row.date + '.');
          else toast('Fetch failed again: ' + stored.error, true);
        } catch (e) {
          toast(e.message, true);
        }
        viewClosingPrices();
      });
      // An errored day no re-fetch can ever fix — before the security's first
      // trading day, or a permanent hole in the provider's series — is
      // discarded so it stops being reported by the health banner. Only
      // errored rows are deletable; an ok price is replaced by a re-fetch.
      if (row.status !== 'ok') {
        const del = el('button', { class: 'small' }, 'Discard');
        del.addEventListener('click', async function () {
          if (!confirm('Discard the errored row for ' + row.listing + ' on ' + row.date
            + '?\n\nDo this only when no price can ever exist for that day.')) return;
          del.disabled = true;
          try {
            await api('DELETE', '/closing_prices/' + row._listing_id + '/' + row.date);
            toast('Discarded the errored row for ' + row.date + '.');
          } catch (e) {
            toast(e.message, true);
          }
          viewClosingPrices();
        });
        return el('td', { class: 'actions' }, [btn, del]);
      }
      return el('td', { class: 'actions' }, btn);
    },
  });

  // Backfill a listing's history over a date range (e.g. after importing an
  // old trade): trading days only, days already stored ok are skipped.
  const backfillForm = el('form', { class: 'card' });
  backfillForm.appendChild(el('h3', null, 'Backfill'));
  const listingSel = el('select', null, listings.map(function (l) {
    return el('option', { value: l.id }, l.id + ': ' + l.ticker + ' (' + (l.exchange_mic || 'Crypto') + ')');
  }));
  const fromInp = el('input', { type: 'date', required: true });
  const toInp = el('input', { type: 'date', required: true });
  backfillForm.appendChild(el('div', { class: 'field' }, [el('label', null, 'Listing'), listingSel]));
  backfillForm.appendChild(el('div', { class: 'field' }, [el('label', null, 'From'), fromInp]));
  backfillForm.appendChild(el('div', { class: 'field' }, [el('label', null, 'To'), toInp]));
  backfillForm.appendChild(el('div', { class: 'form-actions' }, [
    el('button', { type: 'submit', class: 'primary' }, 'Backfill'),
  ]));
  backfillForm.addEventListener('submit', async function (ev) {
    ev.preventDefault();
    try {
      const s = await api('POST', '/closing_prices/backfill', {
        listing_id: Number(listingSel.value), from: fromInp.value, to: toInp.value,
      });
      toast('Backfill: ' + s.fetched_ok + ' fetched, ' + s.already_stored + ' already stored, '
        + s.errored + ' errored (' + s.trading_days + ' trading days).', s.errored > 0);
      viewClosingPrices();
    } catch (e) {
      toast(e.message, true);
    }
  });

  // Manual price: the way out of a day the provider cannot serve at all (a
  // delisted or mis-served symbol, a permanent hole in its series), which
  // valuation otherwise blocks forever, taking the day's snapshots with it.
  // Both provenance fields are required — a hand-entered figure is only
  // auditable with where it came from and why it was needed.
  const manualForm = el('form', { class: 'card' });
  manualForm.appendChild(el('h3', null, 'Manual price'));
  manualForm.appendChild(el('p', { class: 'hint' },
    'Enter a closing price by hand for a trading day the provider cannot serve. In the '
    + 'listing\'s quote currency, not AUD. Reports value it exactly like a fetched price; the '
    + 'provider will not take the day back, so change it by re-entering it here.'));
  const mListingSel = el('select', null, listings.map(function (l) {
    return el('option', { value: l.id }, l.id + ': ' + l.ticker + ' (' + (l.exchange_mic || 'Crypto') + ')');
  }));
  const mDateInp = el('input', { type: 'date', required: true });
  const mPriceInp = el('input', { type: 'text', inputmode: 'decimal', required: true });
  const mSourcedInp = el('input', {
    type: 'text', required: true, placeholder: 'e.g. asx.com.au closing report',
  });
  const mReasonInp = el('input', {
    type: 'text', required: true, placeholder: 'e.g. provider serves no candle since the delisting',
  });
  manualForm.appendChild(el('div', { class: 'field' }, [el('label', null, 'Listing'), mListingSel]));
  manualForm.appendChild(el('div', { class: 'field' }, [el('label', null, 'Date'), mDateInp]));
  manualForm.appendChild(el('div', { class: 'field' }, [el('label', null, 'Price'), mPriceInp]));
  manualForm.appendChild(el('div', { class: 'field' }, [el('label', null, 'Sourced from'), mSourcedInp]));
  manualForm.appendChild(el('div', { class: 'field' }, [el('label', null, 'Reason'), mReasonInp]));
  manualForm.appendChild(el('div', { class: 'form-actions' }, [
    el('button', { type: 'submit', class: 'primary' }, 'Store price'),
  ]));
  manualForm.addEventListener('submit', async function (ev) {
    ev.preventDefault();
    try {
      await api('PUT', '/closing_prices/' + Number(mListingSel.value) + '/' + mDateInp.value, {
        price: mPriceInp.value, sourced_from: mSourcedInp.value, reason: mReasonInp.value,
      });
      toast('Stored ' + mPriceInp.value + ' for ' + mDateInp.value + '.');
      viewClosingPrices();
    } catch (e) {
      toast(e.message, true);
    }
  });

  // Errored-listings surface: one row per listing with any errored price,
  // its Backfill action pre-fills the form above (a generous window ending
  // at the latest errored date, adjustable before submitting) and scrolls to
  // it — the actual re-fetch is one submit away.
  const erroredRows = erroredListings.map(function (r) {
    return {
      ticker: r.ticker,
      errored_days: r.errored_days,
      latest_errored_date: r.latest_errored_date,
      latest_error: r.latest_error,
      _listing_id: r.listing_id,
      _from: isoDateMinusDays(r.latest_errored_date, r.errored_days * 2),
    };
  });
  const erroredTable = erroredRows.length > 0 ? filterableTable(
    erroredRows,
    ['ticker', 'errored_days', 'latest_errored_date', 'latest_error'],
    {
      actions: function (row) {
        const btn = el('button', { class: 'small primary' }, 'Backfill');
        btn.addEventListener('click', function () {
          listingSel.value = String(row._listing_id);
          fromInp.value = row._from;
          toInp.value = row.latest_errored_date;
          backfillForm.scrollIntoView({ behavior: 'smooth' });
        });
        return el('td', { class: 'actions' }, btn);
      },
    },
  ) : null;

  // Unpriced-days surface: one row per listing with a held day that has no
  // stored row at all. Its Backfill action pre-fills the form above over
  // exactly the hole (earliest → latest unpriced day) and scrolls to it.
  const unpricedRows = unpricedListings.map(function (r) {
    return {
      ticker: r.ticker,
      unpriced_days: r.unpriced_days,
      earliest_date: r.earliest_date,
      latest_date: r.latest_date,
      _listing_id: r.listing_id,
    };
  });
  const unpricedTable = unpricedRows.length > 0 ? filterableTable(
    unpricedRows,
    ['ticker', 'unpriced_days', 'earliest_date', 'latest_date'],
    {
      actions: function (row) {
        const btn = el('button', { class: 'small primary' }, 'Backfill');
        btn.addEventListener('click', function () {
          listingSel.value = String(row._listing_id);
          fromInp.value = row.earliest_date;
          toInp.value = row.latest_date;
          backfillForm.scrollIntoView({ behavior: 'smooth' });
        });
        return el('td', { class: 'actions' }, btn);
      },
    },
  ) : null;

  setMain(el('div', null, [
    el('h2', null, 'Closing Prices'),
    el('p', { class: 'view-desc' },
      'Daily closing prices per held listing, in the listing\'s quote currency, collected by the '
      + 'price-import job after each exchange\'s close (crypto at the UTC-midnight cut-off). '
      + 'A failed fetch shows as an errored row — re-fetch it here once the provider recovers, '
      + 'or enter the price by hand if the provider can never serve that day.'),
    erroredTable ? el('div', { class: 'card' }, [
      el('h3', null, 'Listings with errored prices'),
      el('p', { class: 'hint' },
        'A wrong, renamed, or delisted provider symbol otherwise only shows up indirectly, as a '
        + 'missing snapshot from the errored date onward. Fix the symbol (set price_symbol on the '
        + 'listing, or record a ticker change via Listings) then Backfill to re-fetch.'),
      erroredTable,
    ]) : null,
    unpricedTable ? el('div', { class: 'card' }, [
      el('h3', null, 'Listings with unpriced held days'),
      el('p', { class: 'hint' },
        'A day that was held but never fetched leaves no row at all — it is silent, and the '
        + 'provider stops serving history long before it stops serving last week, so the oldest '
        + 'hole is the urgent one. It happens when a trade is entered long after the fact. '
        + 'Backfill the range; if the provider cannot serve it, enter the price by hand below.'),
      unpricedTable,
    ]) : null,
    backfillForm,
    manualForm,
    table,
  ]));
}

// ---- report snapshots ---------------------------------------------------
// Stored daily results of the price-dependent reports (generate/regenerate
// and inspect them here). The time-series graph itself lives on the
// Portfolio Overview screen (`performancePanel`, below) — this is the
// operational maintenance screen, not where anyone looks to see how the
// portfolio is doing.
async function viewSnapshots() {
  setActiveNav('r:snapshots');
  const metas = await api('GET', '/report_snapshots');
  const defaultRange = await api('GET', '/report_snapshots/regenerate_range');

  // On-demand generation: a past date whose prices have been backfilled, a
  // stale date after recording a back-dated fact, or (date blank) the
  // latest date the whole portfolio can be valued at.
  const genForm = el('form', { class: 'card' });
  const dateInp = el('input', { type: 'date' });
  genForm.appendChild(el('h3', null, 'Generate / regenerate'));
  genForm.appendChild(el('div', { class: 'field' }, [el('label', null, 'Snapshot date'), dateInp]));
  genForm.appendChild(el('p', { class: 'hint' },
    'Blank = the latest date with final prices for every held listing. Every held listing needs an ok stored closing price on (or walked back to) the date — backfill prices first for past dates.'));
  genForm.appendChild(el('div', { class: 'form-actions' }, [
    el('button', { type: 'submit', class: 'primary' }, 'Generate'),
  ]));
  genForm.addEventListener('submit', async function (ev) {
    ev.preventDefault();
    try {
      const body = dateInp.value ? { date: dateInp.value } : {};
      const stored = await api('POST', '/report_snapshots/generate', body);
      toast('Stored ' + stored.length + ' snapshot(s) for ' + stored[0].snapshot_date + '.');
      viewSnapshots();
    } catch (e) {
      toast(e.message, true);
    }
  });

  // Bulk repair: regenerate a date range (after back-dated edits, or to
  // backfill dates that never had a snapshot — e.g. after backfilling old
  // closing prices), or just the provisional dates (the manual counterpart
  // of the true-up that runs after an FX import). Blocked dates are
  // reported; the rest still regenerate.
  const rangeFromInp = el('input', { type: 'date', value: defaultRange.from || '' });
  const rangeToInp = el('input', { type: 'date', value: defaultRange.to || '' });
  genForm.appendChild(el('div', { class: 'field' }, [
    el('label', null, 'Regenerate-all range'),
    rangeFromInp, ' to ', rangeToInp,
  ]));
  genForm.appendChild(el('p', { class: 'hint' },
    'Defaults to the first-ever holding through the latest date with final prices. Every date in range is regenerated, including one with no stored snapshot yet; a date with nothing held is skipped, and a date whose prices aren’t backfilled is reported blocked.'));

  function bulkButton(label, path, buildBody) {
    const btn = el('button', { type: 'button' }, label);
    btn.addEventListener('click', async function () {
      btn.disabled = true;
      btn.textContent = 'Regenerating…';
      try {
        const summary = await api('POST', path, buildBody ? buildBody() : undefined);
        let msg = 'Regenerated ' + summary.regenerated.length + ' date(s).';
        if (summary.blocked.length > 0) {
          const shown = summary.blocked.slice(0, 5).map(function (b) {
            return b.date + ' (' + b.reason + ')';
          }).join('; ');
          const more = summary.blocked.length > 5 ? '; … and ' + (summary.blocked.length - 5) + ' more' : '';
          msg += ' Blocked: ' + shown + more;
        }
        toast(msg, summary.blocked.length > 0);
      } catch (e) {
        toast(e.message, true);
      }
      viewSnapshots();
    });
    return btn;
  }
  genForm.appendChild(el('div', { class: 'form-actions' }, [
    bulkButton('Regenerate all', '/report_snapshots/regenerate_all', function () {
      return { from: rangeFromInp.value || null, to: rangeToInp.value || null };
    }),
    ' ',
    bulkButton('Regenerate provisional', '/report_snapshots/regenerate_provisional'),
  ]));

  const rows = metas.slice().reverse().map(function (m) {
    return {
      date: m.snapshot_date,
      report: m.report,
      generated_at: m.generated_at,
      // Stale wins the badge: regenerating fixes it, and re-flags provisional
      // if the real FX rate is still missing.
      status: m.stale ? 'stale' : (m.provisional ? 'provisional' : 'ok'),
    };
  });
  const table = filterableTable(rows, ['date', 'report', 'generated_at', 'status'], {
    statusField: 'status',
    actions: function (row) {
      const view = el('a', { href: '#/r/snapshots/' + row.report + '/' + row.date }, 'View');
      const regen = el('button', { class: 'small' }, 'Regenerate');
      regen.addEventListener('click', async function () {
        regen.disabled = true;
        regen.textContent = 'Generating…';
        try {
          await api('POST', '/report_snapshots/generate', { date: row.date });
          toast('Regenerated ' + row.date + '.');
        } catch (e) {
          toast(e.message, true);
        }
        viewSnapshots();
      });
      return el('td', { class: 'actions' }, [view, ' ', regen]);
    },
  });

  setMain(el('div', null, [
    el('h2', null, 'Snapshots'),
    el('p', { class: 'view-desc' },
      'Daily stored results of the price-dependent reports, valued at the stored closing prices '
      + '(AUD-converted), written by the report-snapshot job after the day\'s last close — each '
      + 'run also backfills missing dates in the last 14 days. '
      + 'A back-dated fact marks every snapshot dated on or after it stale — the stored result '
      + 'keeps showing, flagged, until regenerated. A snapshot valued while the month\'s FX rate '
      + 'was unpublished is provisional (an earlier month\'s rate was used) and is finalised '
      + 'automatically when the RBA import lands the real rate. A day whose price fetches failed '
      + 'has no snapshot at all until the price re-run succeeds. '
      + 'The market-value graph moved to the Portfolio Overview screen.'),
    genForm,
    table,
  ]));
}

async function viewSnapshotDetail(report, date) {
  setActiveNav('r:snapshots');
  const snap = await api('GET', '/report_snapshots/' + pathSeg(report) + '/' + pathSeg(date));
  const header = el('div', null, [
    el('h2', null, 'Snapshot: ' + report + ' @ ' + date),
    el('p', { class: 'view-desc', title: utcTooltip(snap.generated_at) },
      'Generated ' + cellText(snap.generated_at) + '. '),
    el('p', null, el('a', { href: '#/r/snapshots' }, '← All snapshots')),
  ]);
  if (snap.stale) {
    header.appendChild(el('p', { class: 'hint warn' },
      'Stale: a back-dated fact was recorded after this snapshot was generated. The rows below are the stored (pre-fact) result — regenerate from the Snapshots view.'));
  }
  if (snap.provisional) {
    header.appendChild(el('p', { class: 'hint warn' },
      'Provisional: the valuation month\'s FX rate was not published at generation, so an earlier month\'s rate was used. It is finalised automatically once the RBA import lands the real rate (or regenerate provisional from the Snapshots view).'));
  }
  setMain(el('div', null, [header, await dataTable(snap.rows)]));
}

// ---- reports ----------------------------------------------------------

// The Portfolio Overview performance panel's quick-select range presets and
// the localStorage keys it remembers across reloads under (so the panel
// opens on the last-used preset, e.g. 1Y, instead of resetting to All every
// day the page is checked). A custom From/To range is deliberately *not*
// remembered — applying one clears the stored preset (see performancePanel)
// so the remembered range always keeps moving with new snapshots.
const RANGE_PRESETS = [
  ['1m', '1M'], ['3m', '3M'], ['6m', '6M'], ['1y', '1Y'], ['2y', '2Y'], ['3y', '3Y'],
  ['fytd', 'FY'], ['all', 'All'],
];
const RANGE_PREF_KEY = 'share-tracker.overview.range';
const HIDE_INACTIVE_PREF_KEY = 'share-tracker.overview.hideInactive';

// A period figure formatted through the shared money display rules
// (COLUMN_KINDS 'money': round to 2 dp + thousands grouping, full value on
// hover when rounding drops precision) — the panel below is a hand-built
// stat grid, not a filterableTable, so it calls numericDisplay directly
// rather than formatting money itself.
function moneyEl(value) {
  const nd = numericDisplay(value, 'money');
  return el('span', { title: nd ? nd.tip : null }, nd ? nd.text : cellText(value));
}

function statItem(label, valueEl) {
  return el('div', { class: 'stat' }, [
    el('div', { class: 'stat-label' }, label),
    el('div', { class: 'stat-value' }, valueEl),
  ]);
}

// The period-performance report's response, split into a `headline` (the
// stat grid, which always sums exactly to the period return — see
// reports::period_performance) shown above the chart so it's visible without
// scrolling, and a `detail` (the per-currency FX line and the collapsed
// per-holding contributions) shown below the range control.
async function renderPeriodSummary(r) {
  const headlineParts = [];
  if (r.provisional) {
    headlineParts.push(el('p', { class: 'hint warn' },
      'Provisional: a conversion at one end of this period used a fallback-month FX rate (the real month\'s rate was not published yet). Figures here will change once it lands.'));
  }
  // Windows over a year show the annualised money-weighted return instead of
  // the raw total_return_pct — see periodReturnPct's own comment for why.
  const ret = periodReturnPct(r);
  headlineParts.push(el('div', { class: 'perf-summary' }, [
    statItem('Opening value', moneyEl(r.opening_market_value)),
    statItem('Closing value', moneyEl(r.closing_market_value)),
    statItem('Period return', moneyEl(r.total_return)),
    statItem(ret.annualized ? 'Return % (p.a.)' : 'Return %',
      ret.value == null ? '—' : cellText(ret.value) + '%'),
    statItem('Capital growth', moneyEl(r.capital_growth)),
    statItem('FX movement', moneyEl(r.fx_movement)),
    statItem('Income', moneyEl(r.income)),
    statItem('Purchases', moneyEl(r.purchases)),
    statItem('Sale proceeds', moneyEl(r.sale_proceeds)),
    statItem('Realised capital gain (tax)', moneyEl(r.realised_capital_gain)),
  ]));

  const detailParts = [];
  if (r.fx_by_currency.length > 0) {
    detailParts.push(el('h4', null, 'FX movement by currency'));
    detailParts.push(await dataTable(r.fx_by_currency, ['currency', 'fx_movement', 'rate_from', 'rate_to', 'provisional']));
  }

  // `holdings` includes a row for every holding with any history at all —
  // including one fully closed years before `from`, which shows every figure
  // at exactly zero (see holdingHasActivity's doc comment). The checkbox
  // hides those by default so real movers aren't buried in zero-rows;
  // unticking shows the full set. Remembered across reloads like the range
  // preset.
  const contribHolder = el('div');
  const hideInp = el('input', { type: 'checkbox', id: 'hide-inactive-holdings' });
  hideInp.checked = loadPref(HIDE_INACTIVE_PREF_KEY, 'true') !== 'false';
  async function renderContrib() {
    contribHolder.innerHTML = '';
    const all = r.holdings;
    const rows = hideInp.checked ? all.filter(holdingHasActivity) : all;
    const hiddenCount = all.length - rows.length;
    if (rows.length === 0 && hiddenCount > 0) {
      contribHolder.appendChild(el('p', { class: 'hint' },
        'All ' + hiddenCount + (hiddenCount === 1 ? ' holding had' : ' holdings had') + ' no activity in this period.'));
      return;
    }
    contribHolder.appendChild(await dataTable(rows, [
      'listing_id', 'holding_account_id', 'opening_market_value', 'closing_market_value',
      'purchases', 'sale_proceeds', 'income', 'capital_growth', 'fx_movement', 'total_return',
    ]));
    if (hiddenCount > 0) {
      contribHolder.appendChild(el('p', { class: 'hint' },
        hiddenCount + (hiddenCount === 1 ? ' holding' : ' holdings') + ' with no activity in this period hidden.'));
    }
  }
  hideInp.addEventListener('change', function () {
    savePref(HIDE_INACTIVE_PREF_KEY, hideInp.checked ? 'true' : 'false');
    renderContrib();
  });
  await renderContrib();

  detailParts.push(el('details', null, [
    el('summary', null, 'Per-holding contributions'),
    el('div', { class: 'toggle-row' }, [
      hideInp,
      el('label', { for: 'hide-inactive-holdings' }, 'Hide holdings with no activity in this period'),
    ]),
    contribHolder,
  ]));

  return { headline: el('div', null, headlineParts), detail: el('div', null, detailParts) };
}

// The Portfolio Overview screen's market-value graph and period-performance
// summary (moved here from the Snapshots maintenance screen — see
// `viewSnapshots`). Range presets and a custom from/to both resolve to the
// nearest actual stored snapshot dates before calling the report, so the
// summary always matches stored prices and the chart's own endpoints.
//
// Layout puts the headline stat grid above the chart (so the return figures
// are visible without scrolling, since this panel opens the app's home
// screen) and the FX/per-holding detail below the range control, where a
// closed-by-default `<details>` doesn't compete for attention.
async function performancePanel() {
  const series = await api('GET', '/report_snapshots/series');
  if (!series || series.length < 2) {
    return el('div', { class: 'card perf-panel' }, [el('h3', null, 'Performance'), seriesChart(series)]);
  }

  const statsHolder = el('div');
  const chartHolder = el('div');
  const detailHolder = el('div');
  const fromInp = el('input', { type: 'date' });
  const toInp = el('input', { type: 'date' });

  function nearestSeriesDates(from, to) {
    let rFrom = null, rTo = null;
    series.forEach(function (p) {
      if (p.snapshot_date >= from && rFrom === null) rFrom = p.snapshot_date;
      if (p.snapshot_date <= to) rTo = p.snapshot_date;
    });
    return { from: rFrom, to: rTo };
  }

  async function applyRange(from, to) {
    syncPresetButtons();
    fromInp.value = from;
    toInp.value = to;
    chartHolder.innerHTML = '';
    chartHolder.appendChild(seriesChart(sliceSeries(series, from, to)));
    statsHolder.innerHTML = '';
    detailHolder.innerHTML = '';
    const resolved = nearestSeriesDates(from, to);
    if (!resolved.from || !resolved.to || resolved.from >= resolved.to) {
      statsHolder.appendChild(el('p', { class: 'hint' },
        'Select a range spanning at least two stored snapshots for a period summary.'));
      return;
    }
    try {
      const result = await api('POST', '/portfolio/period-performance', {
        from: resolved.from, to: resolved.to,
      });
      const summary = await renderPeriodSummary(result);
      statsHolder.appendChild(summary.headline);
      detailHolder.appendChild(summary.detail);
    } catch (e) {
      statsHolder.appendChild(el('p', { class: 'hint warn' }, e.message));
    }
  }

  // `activePreset` is null while a custom From/To range is in effect (no
  // preset button highlighted); a preset click or the initial restore sets
  // it before calling applyRange, which then highlights the matching button.
  let activePreset = null;
  const presetButtons = RANGE_PRESETS.map(function (p) {
    const btn = el('button', { type: 'button', class: 'small' }, p[1]);
    btn._presetKey = p[0];
    btn.addEventListener('click', function () {
      activePreset = p[0];
      savePref(RANGE_PREF_KEY, activePreset);
      const r = presetRange(series, p[0]);
      applyRange(r.from, r.to);
    });
    return btn;
  });
  function syncPresetButtons() {
    presetButtons.forEach(function (btn) {
      const active = btn._presetKey === activePreset;
      btn.classList.toggle('active', active);
      btn.setAttribute('aria-pressed', active ? 'true' : 'false');
    });
  }

  const rangeForm = el('form', { class: 'range-control' });
  rangeForm.appendChild(el('div', { class: 'form-actions' }, presetButtons));
  rangeForm.appendChild(el('div', { class: 'field' }, [el('label', null, 'From'), fromInp]));
  rangeForm.appendChild(el('div', { class: 'field' }, [el('label', null, 'To'), toInp]));
  rangeForm.appendChild(el('button', { type: 'submit', class: 'small' }, 'Apply'));
  rangeForm.addEventListener('submit', function (ev) {
    ev.preventDefault();
    if (fromInp.value && toInp.value) {
      // A custom range is ad-hoc: clear the remembered preset so the next
      // page load falls back to the default (all) rather than reapplying
      // stale fixed dates.
      activePreset = null;
      savePref(RANGE_PREF_KEY, null);
      applyRange(fromInp.value, toInp.value);
    }
  });

  const panel = el('div', { class: 'card perf-panel' }, [
    el('h3', null, 'Market value and unrealised gain over time'),
    statsHolder,
    chartHolder,
    rangeForm,
    detailHolder,
  ]);
  const storedPreset = loadPref(RANGE_PREF_KEY, 'all');
  activePreset = RANGE_PRESETS.some(function (p) { return p[0] === storedPreset; }) ? storedPreset : 'all';
  const initial = presetRange(series, activePreset);
  await applyRange(initial.from, initial.to);
  return panel;
}

async function viewReport(report) {
  setActiveNav('r:' + report.slug);
  const header = el('div', null, [
    el('h2', null, report.title),
    el('p', { class: 'view-desc' }, report.desc),
  ]);
  // Tax-return-ready CSV download: reports flagged `export` serve the same
  // rows as CSV from `<api>/export` (Content-Disposition makes it a download).
  if (report.export) {
    header.appendChild(el('p', null,
      el('a', { href: apiUrl(report.api + '/export'), class: 'export-link' }, 'Export CSV')));
  }
  const result = el('div');
  // Config-driven, not overview-specific — any report can opt in by setting
  // `performancePanel` (see config.js). Currently only the Portfolio
  // Overview screen does.
  const panel = report.performancePanel ? await performancePanel() : null;
  // Shortcut buttons for the report's most common follow-on actions — the
  // Portfolio Overview (the app's home screen) uses this for New trade/
  // income/sell/transfer so they don't require a menu hunt.
  const shortcuts = report.shortcuts
    ? el('div', { class: 'toolbar report-shortcuts' }, report.shortcuts.map(function (s) {
      return el('a', { href: s.href }, el('button', { class: s.primary ? 'primary' : null }, s.label));
    }))
    : null;

  // Summarise the live-fetched prices' as-of times into one "as at …" line
  // (the freshness of the valuation), plus a count of holdings the live fetch
  // could not value.
  function asAtSummary(rows) {
    const asOf = [];
    let unavailable = 0, provisional = 0;
    (rows || []).forEach(function (r) {
      if (r && r.price_as_of) asOf.push(r.price_as_of);
      if (r && r.price_unavailable) unavailable += 1;
      if (r && r.fx_provisional) provisional += 1;
    });
    if (asOf.length === 0 && unavailable === 0 && provisional === 0) return null;
    const line = el('p', { class: 'hint as-at' });
    if (asOf.length > 0) {
      asOf.sort();
      const lo = asOf[0], hi = asOf[asOf.length - 1];
      const text = lo === hi
        ? 'Live prices as at ' + fmtLocalTimestamp(lo)
        : 'Live prices as at ' + fmtLocalTimestamp(lo) + ' – ' + fmtLocalTimestamp(hi);
      line.appendChild(el('span', { title: utcTooltip(hi) }, text));
    }
    if (provisional > 0) {
      line.appendChild(el('span', { class: 'warn' },
        (asOf.length > 0 ? ' · ' : '') + provisional
        + ' holding(s) valued at a provisional FX rate (the valuation month\'s rate is not published yet)'));
    }
    if (unavailable > 0) {
      line.appendChild(el('span', { class: 'warn' },
        (asOf.length > 0 || provisional > 0 ? ' · ' : '') + unavailable + ' holding(s) had no live price'));
    }
    return line;
  }

  async function render(rows) {
    result.innerHTML = '';
    const asAt = asAtSummary(Array.isArray(rows) ? rows : [rows]);
    if (asAt) result.appendChild(asAt);
    // Reports with `tables` return one object whose listed keys each render
    // as a titled table (a non-array value renders as a one-row table).
    if (report.tables) {
      for (const t of report.tables) {
        const v = rows[t.key];
        const arr = Array.isArray(v) ? v : (v == null ? [] : [v]);
        result.appendChild(el('h3', null, t.title));
        // `rows` (the whole response object) is threaded through as the
        // `context` an `expand.from` sibling lookup reads (e.g. the parcel
        // optimiser's `strategies` table expanding from its `allocations`).
        // `t.columns`, when given, overrides the auto-derived column list —
        // e.g. to drop a flattened field with no business in this table
        // (the what-if's `years` rows flatten in `NetCapitalGainYear`'s
        // `disposals`, always empty here since the drilldown belongs to the
        // main report, not the hypothetical dry-run).
        result.appendChild(await dataTable(arr, t.columns || null, report.statusField, t.expand, rows));
      }
      return;
    }
    result.appendChild(await dataTable(rows, report.columns || null, report.statusField, report.expand, null, report.rowActions));
  }

  if (report.method === 'GET') {
    await render(await api('GET', report.api));
    setMain(el('div', null, [header, shortcuts, panel, result]));
    return;
  }

  // Parameterised POST reports (the parcel optimiser, the pre-sale what-if):
  // the body comes from the configured `params` fields — the same field
  // constructors the entity forms use — and the report runs on submit only
  // (no auto-run: the inputs are required).
  if (report.params) {
    const form = el('form', { class: 'card' });
    for (const f of report.params) form.appendChild(await buildFieldInput(f));
    form.appendChild(el('div', { class: 'form-actions' }, [
      el('button', { type: 'submit', class: 'primary' }, 'Run report'),
    ]));
    form.addEventListener('submit', async function (ev) {
      ev.preventDefault();
      const body = {};
      report.params.forEach(function (f) {
        const v = readFieldValue(f, form);
        if (v !== null) body[f.name] = v;
      });
      try {
        await render(await api('POST', report.api, body));
      } catch (e) {
        toast(e.message, true);
      }
    });
    setMain(el('div', null, [header, shortcuts, panel, form, result]));
    return;
  }

  // POST reports value each held listing from the live price source by
  // default (live: true); the form below lets the user override specific
  // listings' prices (what-if) and pick an as-of date. An explicit price
  // wins over the live fetch. Styled as a plain (non-card) form and placed
  // below the holdings table it values, not as a headline element — the
  // Portfolio Overview's performance panel and the table itself are what
  // lead the screen.
  const listings = await api('GET', '/listings');
  const priceForm = el('form', { class: 'price-form' });
  const priceDetails = el('details', { class: 'price-overrides' }, [
    el('summary', null, 'Manual Price Overrides'),
    el('p', { class: 'hint' },
      'Leave blank to value from the live price source. Enter a price to override that listing.'),
  ]);
  listings.forEach(function (l) {
    priceDetails.appendChild(el('div', { class: 'field' }, [
      el('label', null, l.id + ': ' + l.ticker + ' (' + (l.exchange_mic || 'Crypto') + ')'),
      el('input', { type: 'text', inputmode: 'decimal', 'data-listing': l.id }),
    ]));
  });
  priceForm.appendChild(priceDetails);
  if (report.asOfDate) {
    priceForm.appendChild(el('div', { class: 'field' }, [
      el('label', null, 'As-of date'),
      el('input', { type: 'date', name: 'as_of_date' }),
    ]));
  }
  priceForm.appendChild(el('div', { class: 'form-actions' }, [
    el('button', { type: 'submit', class: 'primary' }, 'Run report'),
  ]));
  function buildBody() {
    const prices = {};
    priceForm.querySelectorAll('[data-listing]').forEach(function (inp) {
      const v = (inp.value || '').trim();
      if (v !== '') prices[inp.getAttribute('data-listing')] = v;
    });
    const body = { prices: prices, live: true };
    if (report.asOfDate) {
      const d = (priceForm.querySelector('[name="as_of_date"]').value || '').trim();
      if (d !== '') body.as_of_date = d;
    }
    return body;
  }
  priceForm.addEventListener('submit', async function (ev) {
    ev.preventDefault();
    try {
      await render(await api('POST', report.api, buildBody()));
    } catch (e) {
      toast(e.message, true);
    }
  });
  setMain(el('div', null, [header, shortcuts, panel, el('h3', null, 'Holdings'), result, priceForm]));
  // Run live on first load so the valuation is shown without manual entry.
  try {
    await render(await api('POST', report.api, buildBody()));
  } catch (e) {
    toast(e.message, true);
  }
}

// ---- router -----------------------------------------------------------
async function render() {
  refreshHealthBanner(); // deliberately not awaited: the view renders in parallel
  const hash = (location.hash || '').replace(/^#/, '');
  const parts = hash.split('/').filter(Boolean);
  try {
    // `#/` is the app's home screen: the Portfolio Overview, rendered
    // directly (not a redirect) so it's a real, stable URL for the brand
    // link. `#/r/overview` keeps working unchanged (linked from the Reports
    // menu) and resolves to the same view via the `parts[0] === 'r'` branch.
    if (parts.length === 0) return await viewReport(reportBySlug.overview);
    if (parts[0] === 'e') {
      const entity = entityBySlug[parts[1]];
      if (!entity) throw new Error('Unknown view');
      if (parts[2] === 'new') return await viewEntityForm(entity, null);
      if (parts[2] === 'edit') return await viewEntityForm(entity, parts.slice(3));
      return await viewEntityList(entity);
    }
    if (parts[0] === 'sells') {
      if (parts[1] === 'new') return await viewSellForm(null);
      if (parts[1] === 'edit') return await viewSellForm(parts[2]);
      return await viewSellsList();
    }
    if (parts[0] === 'transfers') {
      if (parts[1] === 'new') return await viewTransferForm();
      return await viewTransfersList();
    }
    if (actionBySlug[parts[0]]) return await viewAction(actionBySlug[parts[0]], parts[1]);
    if (parts[0] === 'attachments') return await viewAttachments(parts[1], parts[2]);
    if (parts[0] === 'jobs') return await viewJobs();
    if (parts[0] === 'prices') return await viewClosingPrices();
    if (parts[0] === 'r') {
      const report = reportBySlug[parts[1]];
      if (!report) throw new Error('Unknown report');
      if (report.custom === 'snapshots') {
        if (parts[2] && parts[3]) return await viewSnapshotDetail(parts[2], parts[3]);
        return await viewSnapshots();
      }
      if (report.custom === 'tax-report') return await viewTaxReport();
      return await viewReport(report);
    }
    throw new Error('Not found');
  } catch (e) {
    setMain(el('div', { class: 'error' }, e.message));
  }
}

buildNav();
window.addEventListener('hashchange', render);
render();
