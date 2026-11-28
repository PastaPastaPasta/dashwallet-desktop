use std::time::Duration;

use reqwest::{Client, Method, StatusCode, Url};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use zeroize::Zeroizing;

use super::{GiftBrand, GiftCardProduct, GiftSignup, fail};
use crate::error::AppError;

const BASE: &str = "https://api.piggy.cards/dash/v1/";
const MAX_RESPONSE: usize = 4 * 1024 * 1024;

pub(super) struct Piggy {
    client: Client,
    base: Url,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SignupResponse {
    pub user_id: String,
    pub generated_password: Option<Zeroizing<String>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct CreatedOrder {
    pub id: String,
    pub pay_to: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct OrderData {
    pub order_id: String,
    pub status: String,
    pub pay_to: Option<String>,
    #[serde(default)]
    pub cards: Vec<Card>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Card {
    pub name: String,
    pub claim_code: Option<String>,
    pub claim_pin: Option<String>,
    pub claim_link: Option<String>,
}

#[derive(Deserialize)]
struct Data<T> {
    code: u16,
    data: T,
}

impl<T> Data<T> {
    fn result(self) -> Result<T, AppError> {
        if (200..300).contains(&self.code) {
            Ok(self.data)
        } else {
            Err(fail("PiggyCards declined the request."))
        }
    }
}

#[derive(Deserialize)]
struct Product {
    id: u32,
    name: String,
    currency: String,
    price_type: String,
    min_denomination: f64,
    max_denomination: f64,
    denomination: String,
    quantity: u32,
}

fn dollar_cents(value: f64) -> Option<u32> {
    let cents = (value * 100.0).round();
    ((1.0..=250_000.0).contains(&cents) && (cents / 100.0 - value).abs() < 0.000_001)
        .then_some(cents as u32)
}

pub(super) fn parse_dollar_cents(value: &str) -> Option<u32> {
    let (dollars, fraction) = value.split_once('.').unwrap_or((value, ""));
    if dollars.is_empty()
        || dollars.len() > 4
        || fraction.len() > 2
        || !dollars
            .bytes()
            .chain(fraction.bytes())
            .all(|b| b.is_ascii_digit())
    {
        return None;
    }
    let cents = dollars.parse::<u32>().ok()? * 100
        + fraction.parse::<u32>().unwrap_or(0) * if fraction.len() == 1 { 10 } else { 1 };
    (1..=250_000).contains(&cents).then_some(cents)
}

/// Products the mobile wallets never sell, by brand and a fragment of the
/// product name (iOS `PiggyCardsCache.disabledGiftCards`).
const EXCLUDED_PRODUCTS: &[(&str, &str)] = &[("174", "Xbox Live"), ("174", "Xbox Game Pass")];

pub(super) fn excluded(brand: &str, name: &str) -> bool {
    EXCLUDED_PRODUCTS
        .iter()
        .any(|(b, fragment)| *b == brand && name.contains(fragment))
}

impl Product {
    fn available(self) -> Option<GiftCardProduct> {
        if self.currency != "USD" || self.quantity == 0 {
            return None;
        }
        let (values, min, max) = match self.price_type.as_str() {
            "range" => {
                let min = dollar_cents(self.min_denomination)?;
                let max = dollar_cents(self.max_denomination)?;
                if min > max {
                    return None;
                }
                (Vec::new(), Some(min), Some(max))
            }
            "fixed" | "option" => {
                let mut values: Vec<_> = self
                    .denomination
                    .split(',')
                    .map(|v| parse_dollar_cents(v.trim()))
                    .collect::<Option<Vec<_>>>()?;
                values.sort_unstable();
                values.dedup();
                (values, None, None)
            }
            _ => return None,
        };
        Some(GiftCardProduct {
            id: self.id,
            name: self.name,
            values,
            min,
            max,
        })
    }
}

impl Piggy {
    pub fn new() -> Result<Self, AppError> {
        Self::at(Url::parse(BASE).expect("fixed provider URL"))
    }

    fn at(base: Url) -> Result<Self, AppError> {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let client = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .timeout(Duration::from_secs(15))
            .build()
            .map_err(|_| fail("Could not start the PiggyCards connection."))?;
        Ok(Self { client, base })
    }

    async fn request<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &[&str],
        query: &[(&str, &str)],
        token: Option<&str>,
        body: Option<Value>,
    ) -> Result<T, AppError> {
        let mut url = self.base.clone();
        url.path_segments_mut()
            .expect("hierarchical provider URL")
            .pop_if_empty()
            .extend(path);
        if !query.is_empty() {
            url.query_pairs_mut().extend_pairs(query);
        }
        let unreachable = || fail("Could not reach PiggyCards. Try again.");
        let mut request = self
            .client
            .request(method, url)
            .header(reqwest::header::ACCEPT, "application/json");
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        if let Some(body) = body {
            request = request.json(&body);
        }
        let mut response = request.send().await.map_err(|_| unreachable())?;
        match response.status() {
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
                return Err(fail(
                    "PiggyCards refused access. Check your code or sign in again.",
                ));
            }
            status if status.is_success() => {}
            _ => {
                return Err(fail(
                    "PiggyCards declined the request. Check the details and try again.",
                ));
            }
        }
        let mut bytes = Zeroizing::new(Vec::new());
        while let Some(chunk) = response.chunk().await.map_err(|_| unreachable())? {
            if bytes.len().saturating_add(chunk.len()) > MAX_RESPONSE {
                return Err(fail("PiggyCards returned an oversized response."));
            }
            bytes.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&bytes)
            .map_err(|_| fail("PiggyCards returned an unexpected response."))
    }

    async fn get<T: DeserializeOwned>(
        &self,
        path: &[&str],
        query: &[(&str, &str)],
        token: &str,
    ) -> Result<T, AppError> {
        self.request(Method::GET, path, query, Some(token), None)
            .await
    }

    async fn post<T: DeserializeOwned>(
        &self,
        path: &str,
        token: Option<&str>,
        body: Value,
    ) -> Result<T, AppError> {
        self.request(Method::POST, &[path], &[], token, Some(body))
            .await
    }

    pub async fn signup(&self, info: &GiftSignup) -> Result<SignupResponse, AppError> {
        self.post(
            "signup",
            None,
            json!({
                "firstName": info.first_name,
                "lastName": info.last_name,
                "email": info.email,
                "country": "US",
                "state": info.state,
            }),
        )
        .await
    }

    pub async fn verify(&self, email: &str, otp: &str) -> Result<Zeroizing<String>, AppError> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Verified {
            generated_password: Zeroizing<String>,
        }
        let response: Verified = self
            .post("verify-otp", None, json!({ "email": email, "otp": otp }))
            .await?;
        Ok(response.generated_password)
    }

    pub async fn login(
        &self,
        id: &str,
        password: &str,
    ) -> Result<(Zeroizing<String>, u64), AppError> {
        #[derive(Deserialize)]
        struct Token {
            access_token: Zeroizing<String>,
            expires_in: u64,
        }
        let response: Token = self
            .post("login", None, json!({ "userId": id, "password": password }))
            .await?;
        if response.access_token.trim().is_empty() || response.expires_in == 0 {
            return Err(fail("PiggyCards returned an invalid sign-in response."));
        }
        Ok((response.access_token, response.expires_in))
    }

    pub async fn brands(&self, token: &str) -> Result<Vec<GiftBrand>, AppError> {
        self.get(&["brands", "US"], &[], token).await
    }

    pub async fn products(
        &self,
        brand: &str,
        token: &str,
    ) -> Result<Vec<GiftCardProduct>, AppError> {
        let payload: Data<Vec<Product>> = self
            .get(&["giftcards", "US"], &[("brandId", brand)], token)
            .await?;
        Ok(payload
            .result()?
            .into_iter()
            .filter_map(Product::available)
            .filter(|p| !excluded(brand, &p.name))
            .collect())
    }

    pub async fn create_order(
        &self,
        token: &str,
        product: u32,
        cents: u32,
        info: &GiftSignup,
    ) -> Result<CreatedOrder, AppError> {
        self.post("orders", Some(token), json!({
            "orders": [{
                "productId": product, "quantity": 1,
                "denomination": f64::from(cents) / 100.0, "currency": "USD",
            }],
            "recipientEmail": info.email,
            "user": {
                "name": format!("{} {}", info.first_name, info.last_name),
                "ip": "192.168.100.1",
                "metadata": { "registeredSince": "2025-07-01", "country": "US", "state": info.state },
            },
        })).await
    }

    pub async fn order(&self, id: &str, token: &str) -> Result<OrderData, AppError> {
        let response: Data<OrderData> = self.get(&["orders", id], &[], token).await?;
        let order = response.result()?;
        if order.order_id != id {
            return Err(fail("PiggyCards returned a different order."));
        }
        Ok(order)
    }
}

#[cfg(test)]
mod tests;
