mod account;
mod piggy;
mod store;

use std::sync::{Arc, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use dashcore::{Network, hashes::sha256};
use dw_engine::{DashNetwork, NetworkSession, PreparedOutput};
use dw_uri::core::handle_uri;
use dw_vault::{KeyringOsStore, LockState, OsSecretStore};
use serde::{Deserialize, Serialize};
use ts_rs::TS;
use zeroize::Zeroizing;

use super::send::{self, BroadcastOutcome};
use crate::error::AppError;
use crate::ipc::commands;
use crate::settings;
use crate::state::{AppState, parse_wallet_id};
use piggy::Piggy;
use store::{Order, Orders, Secrets};

pub(crate) const EXCLUDED: &[(&str, &str)] = &[];
const INVOICE_WINDOW_SECS: u64 = 120;
const US_STATES: &str = "AL AK AZ AR CA CO CT DE DC FL GA HI ID IL IN IA KS KY LA ME MD MA MI MN MS MO MT NE NV NH NJ NM NY NC ND OH OK OR PA RI SC SD TN TX UT VT VA WA WV WI WY";

pub(crate) struct GiftCards {
    os: Arc<dyn OsSecretStore>,
    piggy: OnceLock<Result<Piggy, AppError>>,
    serial: tokio::sync::Mutex<()>,
}

impl Default for GiftCards {
    fn default() -> Self {
        Self {
            os: Arc::new(KeyringOsStore::new()),
            piggy: OnceLock::new(),
            serial: tokio::sync::Mutex::new(()),
        }
    }
}

#[derive(Deserialize, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct GiftSignup {
    first_name: String,
    last_name: String,
    email: String,
    state: String,
}

impl GiftSignup {
    fn checked(mut self) -> Result<Self, AppError> {
        self.first_name = self.first_name.trim().to_owned();
        self.last_name = self.last_name.trim().to_owned();
        self.email = self.email.trim().to_owned();
        let valid_name =
            |s: &str| !s.is_empty() && s.len() <= 80 && !s.chars().any(char::is_control);
        if !valid_name(&self.first_name) || !valid_name(&self.last_name) {
            return Err(fail("Enter your first and last name."));
        }
        if self.email.len() > 254
            || self
                .email
                .chars()
                .any(|c| c.is_whitespace() || c.is_control())
            || !self
                .email
                .split_once('@')
                .is_some_and(|(a, b)| !a.is_empty() && b.contains('.') && !b.contains('@'))
        {
            return Err(fail("Enter a valid email address."));
        }
        if !US_STATES.split_whitespace().any(|s| s == self.state) {
            return Err(fail("Select your US state."));
        }
        Ok(self)
    }
}

#[derive(Serialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum GiftAccount {
    SignedOut,
    CodeSent { email: String },
    CanSignIn { email: String },
    SignedIn { email: String },
}

#[derive(Deserialize, Serialize, TS)]
pub struct GiftBrand {
    id: String,
    name: String,
}

#[derive(Serialize, TS)]
pub struct GiftCardProduct {
    id: u32,
    name: String,
    values: Vec<u32>,
    min: Option<u32>,
    max: Option<u32>,
}

impl GiftCardProduct {
    fn allows(&self, cents: u32) -> bool {
        match self.min.zip(self.max) {
            Some((min, max)) => (min..=max).contains(&cents),
            None => self.values.contains(&cents),
        }
    }
}

#[derive(Clone, Deserialize, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct GiftInvoice {
    address: String,
    duffs: u64,
}

impl GiftInvoice {
    fn parse(uri: &str, network: Network) -> Result<Self, AppError> {
        let invalid = || fail("PiggyCards did not return a valid DASH payment invoice.");
        let (_, query) = uri
            .strip_prefix("dash:")
            .and_then(|uri| uri.split_once('?'))
            .ok_or_else(invalid)?;
        if uri.contains('#') {
            return Err(invalid());
        }
        let mut amount_seen = false;
        for pair in query.split('&') {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            if key.is_empty()
                || !key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
                || matches!(key, "r" | "req-r")
            {
                return Err(invalid());
            }
            if matches!(key, "amount" | "req-amount") {
                let (whole, fraction) = value.split_once('.').unwrap_or((value, ""));
                if amount_seen
                    || whole.is_empty()
                    || fraction.len() > 8
                    || !whole
                        .bytes()
                        .chain(fraction.bytes())
                        .all(|b| b.is_ascii_digit())
                {
                    return Err(invalid());
                }
                amount_seen = true;
            }
        }
        let recipient = handle_uri(uri, network).map_err(|_| invalid())?;
        let duffs = u64::try_from(recipient.amount)
            .ok()
            .filter(|&n| amount_seen && n > 0)
            .ok_or_else(invalid)?;
        Ok(Self {
            address: recipient.address,
            duffs,
        })
    }

    fn verify(&self, outputs: &[PreparedOutput]) -> Result<(), AppError> {
        let external = |out: &PreparedOutput| !out.is_mine;
        let mut recipients = outputs.iter().filter(|out| external(out));
        if !recipients.next().is_some_and(|out| {
            !out.is_change
                && out.address.as_deref() == Some(&self.address)
                && out.amount == self.duffs
        }) || recipients.next().is_some()
            || outputs.iter().any(|out| out.is_mine && !out.is_change)
        {
            return Err(fail("The payment differs from the gift card invoice."));
        }
        Ok(())
    }
}

#[derive(Clone, Deserialize, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct GiftOrder {
    id: String,
    name: String,
    cents: u32,
    expires_at: u64,
    invoice: Option<GiftInvoice>,
    txid: Option<String>,
}

#[derive(Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum GiftOrderStatus {
    AwaitingPayment,
    Processing,
    Complete,
    Cancelled,
    Unknown,
}

impl From<&str> for GiftOrderStatus {
    fn from(value: &str) -> Self {
        match value {
            "Payment pending" => Self::AwaitingPayment,
            "Paid" | "Processing" => Self::Processing,
            "Complete" => Self::Complete,
            "Cancelled" => Self::Cancelled,
            _ => Self::Unknown,
        }
    }
}

#[derive(Serialize, TS)]
pub struct GiftCardSecret {
    name: String,
    code: Option<String>,
    pin: Option<String>,
}

#[derive(Serialize, TS)]
pub struct GiftOrderDetails {
    order: GiftOrder,
    status: GiftOrderStatus,
    cards: Vec<GiftCardSecret>,
}

fn fail(message: &'static str) -> AppError {
    AppError::invalid_argument(message)
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn check_active(state: &AppState, session: &Arc<NetworkSession>) -> Result<(), AppError> {
    if !state
        .session_now()
        .is_some_and(|active| Arc::ptr_eq(&active, session))
    {
        return Err(fail("The wallet network changed."));
    }
    if !settings::read(&state.data_root)
        .gift_cards_networks
        .iter()
        .any(|n| n == "mainnet")
    {
        return Err(fail("Enable experimental gift cards in Settings first."));
    }
    if !matches!(
        session.vault().status().state,
        LockState::Unlocked | LockState::Unencrypted
    ) {
        return Err(fail("Unlock the wallet to use gift cards."));
    }
    Ok(())
}

async fn active(state: &AppState) -> Result<(Arc<NetworkSession>, u64), AppError> {
    let (session, epoch) = state.session_and_epoch().await?;
    if !matches!(session.network(), DashNetwork::Mainnet) {
        return Err(fail("PiggyCards purchases are available on mainnet only."));
    }
    check_active(state, &session)?;
    Ok((session, epoch))
}

struct Context<'a> {
    _guard: tokio::sync::MutexGuard<'a, ()>,
    session: Arc<NetworkSession>,
    epoch: u64,
    secrets: Secrets,
    piggy: &'a Piggy,
}

impl<'a> Context<'a> {
    async fn open(state: &'a AppState) -> Result<Self, AppError> {
        let guard = state.gift_cards.serial.lock().await;
        let (session, epoch) = active(state).await?;
        let secrets = Secrets::new(&state.data_root, Arc::clone(&state.gift_cards.os))?;
        let piggy = state
            .gift_cards
            .piggy
            .get_or_init(Piggy::new)
            .as_ref()
            .map_err(Clone::clone)?;
        Ok(Self {
            _guard: guard,
            session,
            epoch,
            secrets,
            piggy,
        })
    }

    fn orders(&self) -> Result<Orders, AppError> {
        Orders::open(self.session.data_dir())
    }

    async fn credentials(&self) -> Result<(store::Account, store::Token), AppError> {
        self.secrets.session(self.piggy).await
    }
}

fn checked_id(id: &str) -> Result<(), AppError> {
    if id.is_empty()
        || id.len() > 128
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err(fail("PiggyCards returned an invalid identifier."));
    }
    Ok(())
}

fn invoice_key(account: &str, order: &str) -> [u8; 32] {
    sha256::Hash::hash(format!("piggycards.v1:{account}:{order}").as_bytes()).to_byte_array()
}

commands! {
    /// The PiggyCards sign-in state. The OS secure store owns the credentials.
    #[facade("NetworkSession::vault", "Vault::status")]
    fn gift_card_account(state) -> GiftAccount {
        let ctx = Context::open(&state).await?;
        let account = ctx.secrets.account_status().await?;
        check_active(&state, &ctx.session)?;
        Ok(account)
    }

    /// Creates a PiggyCards account and starts email verification if required.
    #[facade("NetworkSession::vault", "Vault::status")]
    fn gift_card_signup(state, info: GiftSignup) -> GiftAccount {
        let ctx = Context::open(&state).await?;
        ctx.secrets.signup(ctx.piggy, info).await
    }

    /// Verifies the email code, saves the generated password, and signs in.
    #[facade("NetworkSession::vault", "Vault::status")]
    fn gift_card_verify_email(state, code: String) -> GiftAccount {
        let code = Zeroizing::new(code);
        let ctx = Context::open(&state).await?;
        ctx.secrets.verify_email(ctx.piggy, code).await
    }

    /// Signs in again with the password stored for this PiggyCards account.
    #[facade("NetworkSession::vault", "Vault::status")]
    fn gift_card_sign_in(state) -> GiftAccount {
        let ctx = Context::open(&state).await?;
        ctx.secrets.sign_in(ctx.piggy).await
    }

    /// Clears the current PiggyCards session.
    #[facade("NetworkSession::vault", "Vault::status")]
    fn gift_card_sign_out(state) -> GiftAccount {
        let ctx = Context::open(&state).await?;
        ctx.secrets.sign_out().await
    }

    /// PiggyCards merchants for the US catalog.
    #[facade("NetworkSession::vault", "Vault::status")]
    fn gift_card_brands(state) -> Vec<GiftBrand> {
        let ctx = Context::open(&state).await?;
        let (_, token) = ctx.credentials().await?;
        ctx.piggy.brands(&token.access).await
    }

    /// In-stock USD products and denominations for a PiggyCards merchant.
    #[facade("NetworkSession::vault", "Vault::status")]
    fn gift_card_products(state, brand: String) -> Vec<GiftCardProduct> {
        checked_id(&brand)?;
        let ctx = Context::open(&state).await?;
        let (_, token) = ctx.credentials().await?;
        ctx.piggy.products(&brand, &token.access).await
    }

    /// Gift card orders for the selected wallet and PiggyCards account.
    #[facade("NetworkSession::vault", "Vault::status")]
    fn gift_card_orders(state, wallet_id: String) -> Vec<GiftOrder> {
        let ctx = Context::open(&state).await?;
        let (account, _) = ctx.credentials().await?;
        let key = account.key();
        Ok(ctx.orders()?.entries.into_iter().rev()
            .filter(|o| o.account == key && o.wallet == wallet_id)
            .map(|o| o.view).collect())
    }

    /// Creates a single-card order, recording its ID before any wallet payment.
    #[facade("NetworkSession::vault", "Vault::status")]
    fn gift_card_create_order(state, wallet_id: String, brand: String, product_id: u32, cents: u32) -> GiftOrder {
        checked_id(&brand)?;
        let id = parse_wallet_id(&wallet_id)?;
        let ctx = Context::open(&state).await?;
        if !ctx.session.vault().has_wallet_secret(&id.0) {
            return Err(fail("Select a wallet that can send DASH."));
        }
        let (account, token) = ctx.credentials().await?;
        let product = ctx.piggy.products(&brand, &token.access).await?.into_iter()
            .find(|p| p.id == product_id && p.allows(cents))
            .ok_or_else(|| fail("This gift card value is unavailable."))?;
        let mut orders = ctx.orders()?;
        check_active(&state, &ctx.session)?;
        let created = ctx.piggy.create_order(&token.access, product.id, cents, &account.signup).await?;
        checked_id(&created.id)?;
        let key = account.key();
        if orders.entries.iter().any(|o| o.view.id == created.id && o.account == key) {
            return Err(fail("PiggyCards returned an order that already exists. Check My gift cards."));
        }
        let expires_at = now() + INVOICE_WINDOW_SECS;
        let invoice = created.pay_to.as_deref()
            .and_then(|uri| GiftInvoice::parse(uri, Network::Mainnet).ok());
        let view = GiftOrder {
            id: created.id, name: product.name, cents, expires_at, invoice, txid: None,
        };
        orders.entries.push(Order { view: view.clone(), account: key, wallet: wallet_id });
        orders.save()?;
        Ok(view)
    }

    /// Reads fulfillment for a known order. Codes and PINs are fetched on demand.
    #[facade("NetworkSession::vault", "Vault::status", "NetworkSession::invoice_payment")]
    fn gift_card_details(state, wallet_id: String, order_id: String) -> GiftOrderDetails {
        let ctx = Context::open(&state).await?;
        let (account, token) = ctx.credentials().await?;
        let mut orders = ctx.orders()?;
        let account_key = account.key();
        let order = orders.find(&order_id, &account_key, &wallet_id)?;
        let payment = ctx.session.invoice_payment(invoice_key(&account_key, &order_id)).await?;
        let data = ctx.piggy.order(&order_id, &token.access).await?;
        let status = GiftOrderStatus::from(data.status.as_str());
        check_active(&state, &ctx.session)?;
        let expires = order.view.expires_at;
        let missing_invoice = order.view.invoice.is_none() && payment.is_none()
            && matches!(status, GiftOrderStatus::AwaitingPayment) && now() < expires;
        if missing_invoice {
            order.view.invoice = data.pay_to.as_deref().and_then(|uri| GiftInvoice::parse(uri, Network::Mainnet).ok());
        }
        let mut view = order.view.clone();
        view.txid = payment;
        if missing_invoice && view.invoice.is_some() {
            orders.save()?;
        }
        let cards = if matches!(status, GiftOrderStatus::Complete) {
            data.cards.into_iter().map(|c| GiftCardSecret { name: c.name, code: c.claim_code, pin: c.claim_pin }).collect()
        } else {
            Vec::new()
        };
        Ok(GiftOrderDetails { order: view, status, cards })
    }

    /// Opens a normal Pay draft bound to a known PiggyCards order.
    #[facade("NetworkSession::vault", "Vault::status", "NetworkSession::invoice_payment", "NetworkSession::new_tx_draft")]
    fn gift_card_draft(state, wallet_id: String, order_id: String) -> u64 {
        let ctx = Context::open(&state).await?;
        let (account, _) = ctx.credentials().await?;
        let account_key = account.key();
        let key = invoice_key(&account_key, &order_id);
        let mut orders = ctx.orders()?;
        let order = orders.find(&order_id, &account_key, &wallet_id)?;
        if now() >= order.view.expires_at || order.view.invoice.is_none() {
            return Err(fail("The gift card invoice is unavailable or has expired."));
        }
        if ctx.session.invoice_payment(key).await?.is_some() {
            return Err(fail("A payment was already started for this gift card order."));
        }
        check_active(&state, &ctx.session)?;
        let draft = ctx.session.new_tx_draft(parse_wallet_id(&wallet_id)?)?;
        state.sends.insert_send_draft(ctx.epoch, draft, Some(key))
    }

    /// Broadcasts an ordinary Pay draft bound to a PiggyCards invoice.
    #[facade("NetworkSession::vault", "Vault::status", "TxDraft::wallet_id", "PreparedTx::summary", "NetworkSession::invoice_payment", "TxDraft::broadcast_invoice")]
    fn gift_card_broadcast(state, order_id: String, draft: u64, prepared: u64) -> BroadcastOutcome {
        let ctx = Context::open(&state).await?;
        let (account, token) = ctx.credentials().await?;
        let account_key = account.key();
        let key = invoice_key(&account_key, &order_id);
        let d = state.sends.for_payment(draft, Some(key))?;
        let tx = send::prepared_of(&state, draft, prepared)?;
        let wallet = d.wallet_id().to_string();
        let mut orders = ctx.orders()?;
        let order = orders.find(&order_id, &account_key, &wallet)?;
        let invoice = order.view.invoice.as_ref().ok_or_else(|| fail("The gift card invoice is unavailable."))?;
        let previous = ctx.session.invoice_payment(key).await?;
        if previous.as_deref().is_some_and(|id| id != tx.summary().txid) {
            return Err(fail("A payment was already started for this gift card order."));
        }
        let data = ctx.piggy.order(&order_id, &token.access).await?;
        let status = GiftOrderStatus::from(data.status.as_str());
        if !matches!(status, GiftOrderStatus::AwaitingPayment) {
            return Err(fail("This gift card order is no longer awaiting payment. Refresh its status."));
        }
        if let Some(uri) = data.pay_to {
            let current = GiftInvoice::parse(&uri, Network::Mainnet)?;
            if current.address != invoice.address || current.duffs != invoice.duffs {
                return Err(fail("The gift card invoice changed. Get a new quote before paying."));
            }
        }
        if now() >= order.view.expires_at {
            return Err(fail("The gift card quote expired. Get a new quote before paying."));
        }
        check_active(&state, &ctx.session)?;
        let d = state.sends.for_payment(draft, Some(key))?;
        let tx = send::prepared_of(&state, draft, prepared)?;
        invoice.verify(&tx.summary().outputs)?;
        Ok(send::broadcast(&state, d, tx, prepared, Some(key)).await?.into())
    }
}
