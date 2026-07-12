---
name: verify
description: Build, launch, and drive share-tracker to verify a change end-to-end at its HTTP/UI surface
---

# Verifying share-tracker changes

## Launch

```bash
cargo run --quiet -- --db /path/to/scratch/verify.db --port 3971   # run_in_background
sleep 2; curl -s -o /dev/null -w '%{http_code}' http://localhost:3971/listings   # 200 = up
```

Any fresh `--db` path works — migrations + seed data (default holding account 1,
currencies, exchanges incl. XASX) run automatically. Kill the server and delete the
scratch DB when done.

## Drive the API

Entities are `PUT /<entity>/:id` with JSON (204 on success; 422 bodies are plain text
worth capturing). Minimal seed for most flows:

```bash
S=http://localhost:3971
curl -s -X PUT $S/listings/1 -H 'content-type: application/json' -d '{"exchange_mic":"XASX","ticker":"VAS","name":"Vanguard Australian Shares ETF","security_type":"ETF","currency":"AUD","amit":false,"preference":false}'
curl -s -X PUT $S/trades/1 -H 'content-type: application/json' -d '{"trade_type":"Buy","date":"2024-01-10","listing_id":1,"average_price":"100","quantity":"10","currency":"AUD","brokerage":"0","gst_on_brokerage":"0","brokerage_currency":"AUD","fx_rate":"1","holding_account_id":1}'
```

`scripts/fixtures/demo.json` is a list of `{path, body}` pairs with more valid bodies
to crib from. Amounts are decimal **strings**.

## Drive the UI

The SPA is hash-routed off `/`. Headless Chrome dumps the rendered DOM:

```bash
"/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" --headless --disable-gpu \
  --dump-dom --virtual-time-budget=4000 'http://localhost:3971/#/e/income' | grep ...
```

Or `scripts/ui-check.sh --seed demo '#/r/open-parcels'` (spins its own ephemeral
server + DB; `--screenshot` also supported) when you don't need state you set up
yourself.
