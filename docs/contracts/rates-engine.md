# Rates engine contract (`dw_engine::rates`, MP-01)

Contract-Version: 1

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

**When requests are made.** Only while all of these hold: prices are on (`enabled`), the disclosure was
shown (`disclosed`), and a mainnet network is open. Otherwise no request at all, "Update now" included
(`rates.off`). Testnet, devnet and regtest never fetch: test coins have no price.

**Schedule.** One attempt as soon as requests are allowed (after a restart: 5 minutes after the last
attempt), then every 5 minutes ± 20 %. After a failed attempt: 1, 2, then 4 minutes. "Update now" is
refused within 30 s of the last attempt (`rates.throttled`). One attempt at a time.

## 2. Rules on prices

**Taken or refused, per currency.** A new row is refused, keeping the cached one, when its provider time is
more than 5 minutes ahead of the clock, is older than the cached row's, or its price is more than ten times
above or below a cached price that has not expired. A refused row of the chosen currency sets
`lastAttemptFailed`. Prices are `^[0-9]{1,15}(\.[0-9]{1,12})?$`, above zero; other rows are left out.

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
<!-- surface-sha256: 63fe20b3b0f8ed356d244b6aa8a3e547d228011c0f8a1a60b483b1a7966c05af version: 1 -->

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
pub use types::{ CurrencyRate, FiatRate, RateSourceId, RateState, RatesOff, RatesSettings, RatesUnavailable, RatesView, };
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
