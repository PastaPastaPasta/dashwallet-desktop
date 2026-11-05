//! Where prices come from (`docs/contracts/rates-engine.md` §1): CTX, the
//! source both mobile apps use, and a cross rate from BitPay and
//! DashCentral when CTX fails. Each source is a fixed set of HTTPS GETs
//! with no query of ours beyond the URL: no wallet data, no currency, no
//! locale, no cookie, a User-Agent that names no app or version.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use super::decimal::Price;
use super::types::RateSourceId;

/// The primary source (`AND/…/rates/DashRetailClient.java:25,61`; iOS
/// `RatesProvider.swift`).
pub(crate) const CTX_URL: &str = "https://rates.ctx.com/rates?source=ctx";
/// Fiat per BTC (no DASH row).
pub(crate) const BITPAY_URL: &str = "https://bitpay.com/rates";
/// `exchange_rates.dash_usd`.
pub(crate) const DASHCENTRAL_URL: &str = "https://www.dashcentral.org/api/v1/public";

/// BitPay codes left out of the cross rate, as Android leaves them out
/// (`DashRatesSecondFallback.java:18`): crypto, metals and the dead VEF.
const FALLBACK_EXCLUDED: &[&str] = &["BTC", "BCH", "XAG", "XAU", "VEF"];

/// A body larger than this is refused (CTX's is about 57 KiB).
pub(crate) const MAX_BODY: usize = 1 << 20;
const FETCH_TIME: Duration = Duration::from_secs(10);
const CONNECT_TIME: Duration = Duration::from_secs(5);
/// Names no app and no version (owner ruling for MP-01).
pub(crate) const USER_AGENT: &str = "Mozilla/5.0";

/// One row a source returned: fiat per one DASH.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SourceRow {
    pub currency: String,
    pub price: Price,
    /// When the provider took the price (UNIX seconds); the fetch time
    /// for a source that does not say.
    pub retrieved_at: u64,
}

/// One answer of a source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SourceRates {
    pub source: RateSourceId,
    pub rows: Vec<SourceRow>,
}

/// GETs a URL: the production client, or a test double.
pub(crate) trait HttpGet: Send + Sync {
    fn get<'a>(
        &'a self,
        url: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<u8>, String>> + Send + 'a>>;
}

/// A price source the service asks, in order, until one answers.
pub(crate) trait RateSource: Send + Sync {
    fn id(&self) -> RateSourceId;
    fn fetch<'a>(
        &'a self,
        http: &'a dyn HttpGet,
        now: u64,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<SourceRates, String>> + Send + 'a>>;
}

pub(crate) struct Ctx;

impl RateSource for Ctx {
    fn id(&self) -> RateSourceId {
        RateSourceId::Ctx
    }

    fn fetch<'a>(
        &'a self,
        http: &'a dyn HttpGet,
        _now: u64,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<SourceRates, String>> + Send + 'a>>
    {
        Box::pin(async move {
            let body = http.get(CTX_URL).await?;
            Ok(SourceRates {
                source: RateSourceId::Ctx,
                rows: parse_ctx(&body)?,
            })
        })
    }
}

pub(crate) struct BitPayDashCentral;

impl RateSource for BitPayDashCentral {
    fn id(&self) -> RateSourceId {
        RateSourceId::BitPayDashCentral
    }

    fn fetch<'a>(
        &'a self,
        http: &'a dyn HttpGet,
        now: u64,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<SourceRates, String>> + Send + 'a>>
    {
        Box::pin(async move {
            let bitpay = http.get(BITPAY_URL).await?;
            let central = http.get(DASHCENTRAL_URL).await?;
            Ok(SourceRates {
                source: RateSourceId::BitPayDashCentral,
                rows: parse_fallback(&bitpay, &central, now)?,
            })
        })
    }
}

/// The production sources, primary first.
pub(crate) fn production_sources() -> Vec<Arc<dyn RateSource>> {
    vec![Arc::new(Ctx), Arc::new(BitPayDashCentral)]
}

/// CTX's body: `[{baseCurrency, quoteCurrency, price, retrieved, …}]`.
/// Rows of another base, with a price that is not a plain positive decimal,
/// a code that is not three capital letters or a time that does not parse
/// are left out; a body with no DASH row left is an error.
pub(crate) fn parse_ctx(body: &[u8]) -> Result<Vec<SourceRow>, String> {
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Row {
        base_currency: Option<String>,
        quote_currency: Option<String>,
        price: Option<serde_json::Value>,
        retrieved: Option<String>,
    }
    let rows: Vec<Row> =
        serde_json::from_slice(body).map_err(|e| format!("ctx: not the rates list: {e}"))?;
    let mut out = BTreeMap::new();
    for r in rows {
        if r.base_currency.as_deref() != Some("DASH") {
            continue;
        }
        let Some(code) = r.quote_currency.filter(|c| is_code(c)) else {
            continue;
        };
        let Some(price) = r
            .price
            .as_ref()
            .and_then(|v| v.as_str())
            .and_then(Price::parse)
        else {
            continue;
        };
        let Some(at) = r.retrieved.as_deref().and_then(parse_rfc3339_utc) else {
            continue;
        };
        out.insert(
            code.clone(),
            SourceRow {
                currency: code,
                price,
                retrieved_at: at,
            },
        );
    }
    if out.is_empty() {
        return Err("ctx: no DASH price".into());
    }
    Ok(out.into_values().collect())
}

/// DASH per X = DashCentral `dash_usd` × BitPay `X per BTC / USD per BTC`:
/// DashCentral's ten-digit USD price, not its three-digit `btc_dash`, sets
/// the precision. Each step rounds half to even to 12 decimals. Every row
/// is stamped `now`: neither source says when it took its prices.
pub(crate) fn parse_fallback(
    bitpay: &[u8],
    central: &[u8],
    now: u64,
) -> Result<Vec<SourceRow>, String> {
    #[derive(serde::Deserialize)]
    struct BitPay {
        data: Vec<BitPayRow>,
    }
    #[derive(serde::Deserialize)]
    struct BitPayRow {
        code: Option<String>,
        rate: Option<serde_json::Number>,
    }
    #[derive(serde::Deserialize)]
    struct Central {
        exchange_rates: CentralRates,
    }
    #[derive(serde::Deserialize)]
    struct CentralRates {
        dash_usd: serde_json::Number,
    }
    let bitpay: BitPay =
        serde_json::from_slice(bitpay).map_err(|e| format!("bitpay: not the rates list: {e}"))?;
    let central: Central = serde_json::from_slice(central)
        .map_err(|e| format!("dashcentral: no exchange_rates.dash_usd: {e}"))?;
    let dash_usd = number_price(&central.exchange_rates.dash_usd)
        .ok_or("dashcentral: dash_usd is not a positive price")?;
    let per_btc: BTreeMap<String, Price> = bitpay
        .data
        .into_iter()
        .filter_map(|r| {
            Some((
                r.code.filter(|c| is_code(c))?,
                number_price(r.rate.as_ref()?)?,
            ))
        })
        .collect();
    let usd = *per_btc.get("USD").ok_or("bitpay: no USD rate")?;
    let rows: Vec<SourceRow> = per_btc
        .iter()
        .filter(|(code, _)| !FALLBACK_EXCLUDED.contains(&code.as_str()))
        .filter_map(|(code, x)| {
            let price = dash_usd.mul(x.div(usd)?)?;
            Some(SourceRow {
                currency: code.clone(),
                price,
                retrieved_at: now,
            })
        })
        .collect();
    if rows.is_empty() {
        return Err("bitpay: no fiat rate".into());
    }
    Ok(rows)
}

/// A JSON number as a price: its shortest decimal text, which must be a
/// plain decimal (no exponent).
fn number_price(n: &serde_json::Number) -> Option<Price> {
    Price::parse_rounded(&n.to_string())
}

fn is_code(c: &str) -> bool {
    c.len() == 3 && c.bytes().all(|b| b.is_ascii_uppercase())
}

/// `YYYY-MM-DDTHH:MM:SS[.fraction]Z` to UNIX seconds (fraction dropped).
/// Other offsets and forms are `None`.
pub(crate) fn parse_rfc3339_utc(s: &str) -> Option<u64> {
    let s = s.strip_suffix('Z')?;
    let (date, time) = s.split_once('T')?;
    let time = time.split_once('.').map_or(time, |(t, frac)| {
        if !frac.is_empty() && frac.bytes().all(|b| b.is_ascii_digit()) {
            t
        } else {
            ""
        }
    });
    let num = |t: &str, len: usize| -> Option<u64> {
        (t.len() == len && t.bytes().all(|b| b.is_ascii_digit())).then(|| t.parse().ok())?
    };
    let mut d = date.split('-');
    let (y, mo, da) = (num(d.next()?, 4)?, num(d.next()?, 2)?, num(d.next()?, 2)?);
    let mut t = time.split(':');
    let (h, mi, se) = (num(t.next()?, 2)?, num(t.next()?, 2)?, num(t.next()?, 2)?);
    if d.next().is_some() || t.next().is_some() {
        return None;
    }
    if y < 1970
        || !(1..=12).contains(&mo)
        || !(1..=31).contains(&da)
        || h > 23
        || mi > 59
        || se > 60
    {
        return None;
    }
    // Howard Hinnant's days_from_civil.
    let (y, mo, da) = (y as i64, mo as i64, da as i64);
    let y = if mo <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (mo + 9) % 12;
    let doy = (153 * mp + 2) / 5 + da - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    u64::try_from(days * 86_400 + (h * 3600 + mi * 60 + se) as i64).ok()
}

/// The production HTTP client: HTTPS only with the system's certificate
/// checks (reqwest's default TLS), no redirects, no cookies, no proxy from
/// the environment (an app proxy setting, when there is one, replaces
/// this), no referer, [`FETCH_TIME`] and [`MAX_BODY`]. The URLs are this
/// file's constants, never from the network.
pub(crate) struct ReqwestGet {
    client: reqwest::Client,
}

impl ReqwestGet {
    pub(crate) fn new() -> Result<Self, String> {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .https_only(true)
            .referer(false)
            .connect_timeout(CONNECT_TIME)
            .timeout(FETCH_TIME)
            .user_agent(USER_AGENT)
            .build()
            .map_err(|e| format!("http client: {e}"))?;
        Ok(Self { client })
    }
}

impl HttpGet for ReqwestGet {
    fn get<'a>(
        &'a self,
        url: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<u8>, String>> + Send + 'a>>
    {
        Box::pin(async move {
            let host = url::Url::parse(url)
                .ok()
                .and_then(|u| u.host_str().map(str::to_owned))
                .unwrap_or_default();
            let mut resp = self.client.get(url).send().await.map_err(|e| {
                format!(
                    "{host}: {}",
                    if e.is_timeout() {
                        "timed out".into()
                    } else {
                        e.without_url().to_string()
                    }
                )
            })?;
            if !resp.status().is_success() {
                return Err(format!("{host}: HTTP {}", resp.status().as_u16()));
            }
            if resp.content_length().is_some_and(|n| n > MAX_BODY as u64) {
                return Err(format!("{host}: body over {MAX_BODY} bytes"));
            }
            let mut out = Vec::new();
            while let Some(chunk) = resp
                .chunk()
                .await
                .map_err(|e| format!("{host}: {}", e.without_url()))?
            {
                if out.len() + chunk.len() > MAX_BODY {
                    return Err(format!("{host}: body over {MAX_BODY} bytes"));
                }
                out.extend_from_slice(&chunk);
            }
            Ok(out)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CTX: &[u8] = include_bytes!("../../tests/fixtures/rates/ctx.json");
    const BITPAY: &[u8] = include_bytes!("../../tests/fixtures/rates/bitpay.json");
    const CENTRAL: &[u8] = include_bytes!("../../tests/fixtures/rates/dashcentral.json");

    #[test]
    fn rfc3339_utc_seconds() {
        assert_eq!(parse_rfc3339_utc("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(
            parse_rfc3339_utc("2026-11-04T23:42:43.073929833Z"),
            Some(1_793_835_763)
        );
        assert_eq!(parse_rfc3339_utc("2000-02-29T12:00:00Z"), Some(951_825_600));
        for bad in [
            "2026-11-04T23:42:43+00:00",
            "2026-11-04 23:42:43Z",
            "2026-13-04T23:42:43Z",
            "2026-11-04T23:42:43.Z",
            "26-11-04T23:42:43Z",
            "",
        ] {
            assert_eq!(parse_rfc3339_utc(bad), None, "{bad}");
        }
    }

    #[test]
    fn ctx_keeps_dash_rows_with_a_valid_price_code_and_time() {
        let rows = parse_ctx(CTX).unwrap();
        assert!(rows.len() > 100, "{}", rows.len());
        assert!(rows.iter().all(|r| r.currency.len() == 3));
        let usd = rows.iter().find(|r| r.currency == "USD").unwrap();
        assert!(usd.retrieved_at > 1_700_000_000);
        let body = br#"[
            {"baseCurrency":"BTC","quoteCurrency":"USD","price":"80000","retrieved":"2026-11-04T23:42:43Z"},
            {"baseCurrency":"DASH","quoteCurrency":"USD","price":"49.0967","retrieved":"2026-11-04T23:42:43.1Z"},
            {"baseCurrency":"DASH","quoteCurrency":"EUR","price":"-1","retrieved":"2026-11-04T23:42:43Z"},
            {"baseCurrency":"DASH","quoteCurrency":"GBP","price":"1e3","retrieved":"2026-11-04T23:42:43Z"},
            {"baseCurrency":"DASH","quoteCurrency":"JPY","price":7000,"retrieved":"2026-11-04T23:42:43Z"},
            {"baseCurrency":"DASH","quoteCurrency":"usd","price":"1","retrieved":"2026-11-04T23:42:43Z"},
            {"baseCurrency":"DASH","quoteCurrency":"CHF","price":"40","retrieved":"yesterday"},
            {"baseCurrency":"DASH","quoteCurrency":"CAD","price":"0","retrieved":"2026-11-04T23:42:43Z"}
        ]"#;
        let rows = parse_ctx(body).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].currency, "USD");
        assert_eq!(rows[0].price.to_string(), "49.0967");
        assert_eq!(rows[0].retrieved_at, 1_793_835_763);
    }

    #[test]
    fn ctx_with_no_dash_row_is_an_error() {
        assert!(parse_ctx(b"[]").is_err());
        assert!(parse_ctx(br#"[{"baseCurrency":"BTC","quoteCurrency":"USD","price":"1","retrieved":"2026-11-04T23:42:43Z"}]"#).is_err());
        assert!(parse_ctx(b"{}").is_err());
        assert!(parse_ctx(b"<html>").is_err());
    }

    #[test]
    fn fallback_crosses_dashcentral_usd_with_bitpay_btc_rates() {
        let rows = parse_fallback(BITPAY, CENTRAL, 42).unwrap();
        let get = |c: &str| rows.iter().find(|r| r.currency == c).map(|r| r.price);
        let usd = get("USD").unwrap();
        // dash_usd × USD/USD.
        let central: serde_json::Value = serde_json::from_slice(CENTRAL).unwrap();
        let dash_usd =
            Price::parse_rounded(&central["exchange_rates"]["dash_usd"].to_string()).unwrap();
        assert_eq!(usd, dash_usd);
        assert!(get("EUR").is_some());
        for gone in FALLBACK_EXCLUDED {
            assert_eq!(get(gone), None, "{gone}");
        }
        assert!(rows.iter().all(|r| r.retrieved_at == 42));
        let small = br#"{"data":[{"code":"USD","rate":80000},{"code":"EUR","rate":72000}]}"#;
        let central = br#"{"exchange_rates":{"dash_usd":50}}"#;
        let rows = parse_fallback(small, central, 1).unwrap();
        assert_eq!(
            rows.iter()
                .find(|r| r.currency == "EUR")
                .unwrap()
                .price
                .to_string(),
            "45"
        );
        assert!(parse_fallback(br#"{"data":[{"code":"EUR","rate":1}]}"#, central, 1).is_err());
        assert!(parse_fallback(small, br#"{"exchange_rates":{"dash_usd":-1}}"#, 1).is_err());
    }
}
