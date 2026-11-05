# Rates engine contract (`dw_engine::rates`, MP-01)

Contract-Version: 2

Status: **contract**, 2026-11-05. Code: `rust/crates/dw-engine/src/rates/`; bound for the Tauri host in
`rust/crates/dw-app/src/api/rates.rs`; dw-ffi withholds the event and binds no call. Designed in the 0.2
FIAT lane, with the owner's rulings on consent, freshness and rounding.

**A change to the facade's public surface (§3) updates this file and raises `Contract-Version` in the same
change.** `rust/crates/dw-engine/tests/rates_contract.rs` enforces it: §3 is generated from the facade's source
with its SHA-256, and §4 must list every `RatesError` code. To approve a change: raise `Contract-Version`
above, run the test with `DW_BLESS=1` (refused under `CI`), review the diff, and run again without it.

## 0. Scope

Prices of DASH in fiat for the UI: the Home hero and breakdown, Activity, the transaction detail, amount
entry, Settings. Prices are the app's, not a network's: one service per `Engine`, beside its sessions,
persisted in `<data_root>/rates.json` (mode 0600, replaced through a temporary file).

**The engine converts no amount.** The facade gives the price as an exact decimal string; the renderer
converts (`apps/desktop/src/format/fiat.ts`: integer arithmetic, half to even to the currency's minor unit,
pinned by `testdata/fiat_convert.json`). A fiat amount never reaches a send: amount entry turns it into duffs
before the draft, and the confirmation states duffs as the amount that counts.

## 1. Sources and requests

| Order | Source | Request | Price |
|---|---|---|---|
| 1 | CTX (both mobile apps' source) | `GET https://rates.ctx.com/rates?source=ctx` | rows with `baseCurrency == "DASH"`; `price` a decimal string; `retrieved` RFC 3339 UTC |
| 2 | BitPay + DashCentral, when CTX fails | `GET https://bitpay.com/rates`, `GET https://www.dashcentral.org/api/v1/public` | DASH/X = `dash_usd` × (X per BTC ÷ USD per BTC), each step half to even at 12 decimals; BTC, BCH, XAG, XAU, VEF left out; taken at the fetch time |

Every request:

- HTTPS only, with the system's certificate checks; no redirects; no proxy from the environment (an app
  proxy setting, when one exists, carries the requests, and a failing proxy means no request, never a
  direct one);
- the fixed URLs above: no wallet id, address, amount, txid, currency or locale; no cookie; no referer;
  the User-Agent `Mozilla/5.0` (no app name, no version);
- 5 s to connect, 10 s in all, a body of at most 1 MiB.

Not done, a stated deviation from the design (§2 there): the avatar fetcher's `FetchPolicy::STRICT` check
that every address a name resolves to is public. The hosts are the fixed public names above and the
system resolver answers for them; a resolver that pointed one at a local address would get the TLS
handshake (and so learn that a wallet asks for prices) but no request, since the certificate check fails.

**When requests are made.** Only while all of these hold: prices are on (`enabled`), the disclosure was
shown (`disclosed`), and a mainnet network is open. Otherwise no request at all, "Update now" included
(`rates.off`). Testnet, devnet and regtest never fetch: test coins have no price.

**Turning prices off** (`enabled: false`) stops the attempt in flight: its request is dropped, and a result
that lands after Off is never stored, even when prices are on again by then. It also clears every cached
price and attempt, in memory and in `rates.json`, keeping only the settings; on again, the state is
`unavailable(neverFetched)` until a new fetch. The transactions' price stamps (§6) are history and stay.
Off takes effect even when `rates.json` cannot be written: the file is then removed, so a restart reads
prices as never disclosed (no request until the disclosure is shown again), and `rates.storage` is
returned. The removal fails safe but drops the settings with the file: the currency choice is lost, and
the next start offers the locale's currency again with the disclosure. When the removal fails as well
(a read-only data folder, a file another program holds), the failure is logged and the Off is saved, or
the file removed, on the rates task's next pass that can (about once a minute while prices are off).
Remaining gap: if the app stops before any retry lands, the old file is read at the next start, with
prices on.

**Schedule.** One attempt as soon as requests are allowed (after a restart: 5 minutes ± 20 % after the
last attempt), then every 5 minutes ± 20 %. After a failed attempt: 1, 2, then 4 minutes, each ± 20 %. "Update now" is
refused within 30 s of the last attempt (`rates.throttled`). One attempt at a time.

## 2. Rules on prices

**Taken or refused, per currency.** A new row is refused, keeping the cached one, when its provider time is
more than 5 minutes ahead of the clock, is older than the cached row's, or its price is more than ten times
above or below a cached price that has not expired. A row of the chosen currency refused as ahead or as
absurd sets `lastAttemptFailed`; one refused as older does not, the price kept being the newer (the
fallback's rows carry the fetch time, so CTX's next row may be older). Prices are `^[0-9]{1,15}(\.[0-9]{1,12})?$`, above zero; other rows are left out.

**Age** = now − min(provider time, fetch time).

| State | When | A number? |
|---|---|---|
| `off(disabled \| notDisclosed \| notMainnet)` | as named | no |
| `unavailable(noCurrency)` | no currency chosen | no |
| `unavailable(neverFetched)` | no attempt answered yet | no |
| `unavailable(failed)` | attempts made, nothing cached | no |
| `unavailable(currencyMissing)` | the sources have no price for the currency | no |
| `fresh` | age ≤ 30 min | yes |
| `stale` | 30 min < age ≤ 24 h | yes, with the stale warning |
| `unavailable(expired)` | age > 24 h | no |

`rate` is present exactly in `fresh` and `stale`. **Volatile**: in `fresh` only, the price moved more than
half, up or down, from the price before it, when that one was fetched at most 7 days earlier.

## 3. The surface

<!-- BEGIN GENERATED: rates-surface -->
<!-- surface-sha256: 15762d96a0f9caf85d8cacd35e6459cfe74d2c04175090451fdbabeec3da7ef2 version: 2 -->

```rust
// src/rates/errors.rs
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RatesError {
    InvalidCurrency(String),
    Throttled,
    Off(RatesOff),
    Storage(String),
}
impl RatesError {
    pub fn code(&self) -> &'static str;
}
// src/rates/mod.rs
pub use errors::RatesError;
pub use types::{ CurrencyRate, FiatRate, RateSourceId, RateState, RatesOff, RatesSettings, RatesUnavailable, RatesView, TxFiat, };
#[derive(Clone)]
pub struct Rates {
    ..
}
impl crate::Engine {
    pub fn rates(&self) -> Rates;
}
impl Rates {
    pub fn status(&self) -> RatesView;
    pub fn currencies(&self) -> Vec<CurrencyRate>;
    pub fn set_settings(&self, settings: RatesSettings) -> Result<RatesView, RatesError>;
    pub async fn refresh(&self) -> Result<RatesView, RatesError>;
}
// src/rates/types.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RateSourceId {
    Ctx,
    BitPayDashCentral,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RatesSettings {
    pub enabled: bool,
    pub currency: Option<String>,
    pub disclosed: bool,
}
impl Default for RatesSettings;
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RatesOff {
    Disabled,
    NotDisclosed,
    NotMainnet,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RatesUnavailable {
    NeverFetched,
    Failed,
    Expired,
    NoCurrency,
    CurrencyMissing,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "reason", rename_all = "camelCase")]
pub enum RateState {
    Off(RatesOff),
    Unavailable(RatesUnavailable),
    Fresh,
    Stale,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FiatRate {
    pub currency: String,
    pub price: String,
    pub rate_at: u64,
    pub source: RateSourceId,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RatesView {
    pub settings: RatesSettings,
    pub state: RateState,
    pub rate: Option<FiatRate>,
    pub volatile: bool,
    pub last_attempt_failed: bool,
    pub last_attempt_at: Option<u64>,
    pub last_success_at: Option<u64>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CurrencyRate {
    pub code: String,
    pub price: String,
    pub rate_at: u64,
    pub source: RateSourceId,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TxFiat {
    pub currency: String,
    pub price: String,
    pub rate_at: u64,
    pub source: RateSourceId,
}
```

<!-- END GENERATED: rates-surface -->

## 4. Error codes

| Code | When |
|---|---|
| `rates.invalid_currency` | `set_settings` with a currency that is not three capital letters |
| `rates.throttled` | `refresh` within 30 s of the last attempt |
| `rates.off` | `refresh` while no request may be made (§1) |
| `rates.storage` | `rates.json` could not be written |

## 5. Events

`EngineEvent::Rates` (no payload) whenever the view changes: an attempt, a setting, a price turning stale or
expiring (re-read once a minute), a mainnet network opened or closed. dw-app sends it as `{ kind: "rates" }`;
hosts re-read `Rates::status`. dw-ffi withholds it.

## 6. Historic stamps

The price a transaction was first seen at, kept so Activity and the transaction detail can say what it was
worth then. Version 2 adds `TxFiat` to the surface and `fiat_then: Option<TxFiat>` to m1's `TxRecord` (so
`history_page` and `tx_detail`'s records); dw-app's `TxRow` binds it as the optional `fiatThen`; dw-ffi
withholds it.

**When a stamp is written.** When a Mainnet session's history marks a transaction (events.rs
`flush_history`, after its first-seen time), while the chosen currency's price is `fresh`, and only while
the transaction's time, the earlier of when the wallet first saw it and its block's time (whichever are
known), is at most 3 hours before now. A `stale` price, prices off or undisclosed, another network: no
stamp. Turning prices off (§1) stops new stamps: nothing is fresh while they are off, a result discarded
after Off never reaches the stamper, and on again nothing stamps until a new fetch is fresh. The stamps
are written off the event thread; the write checks again that prices were not turned off since the price
was read, else it writes none. There is no
back-fill: a transaction older than 3 hours never gets one, so a rescan, a restore or a
payment seen while no price was fresh is never stamped with a much later price.

**Written once.** One row per wallet and transaction in dw-appdb's `tx_fiat` (`V2026110501`), inserted with
`ON CONFLICT DO NOTHING`: a later price or a currency switch leaves it as it was, so a stamp may be in a
currency other than the one chosen now. Its fields are those of the fresh `FiatRate` (currency, exact
price string, `rate_at`, source). A removed wallet's insert is skipped (DEC-210), and a `.dwbackup` carries
the rows like every `wallet_id` table. A stored source the engine does not know is left out on read.

**Shown, never used.** The renderer converts the record's amount with the stamp's own price and currency
(`format/fiat.ts`, the same rounding as §0); nothing re-rounds it, and nothing in a send reads it.
`tx_fiat.price` is checked as the engine's decimal (§2): 1 to 15 digits, optionally a dot and 1 to 12
more, above zero, so a `.dwbackup` cannot bring in another form.
