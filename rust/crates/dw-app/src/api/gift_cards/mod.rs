mod account;
mod legacy;
mod piggy;
mod store;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use dashcore::{Network, hashes::sha256};
use dw_engine::{DashNetwork, NetworkSession, PreparedOutput, WalletId};
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
use store::{Change, Order, WalletStore};

pub(crate) const EXCLUDED: &[(&str, &str)] = &[];
const INVOICE_WINDOW_SECS: u64 = 120;
const US_STATES: &str = "AL AK AZ AR CA CO CT DE DC FL GA HI ID IL IN IA KS KY LA ME MD MA MI MN MS MO MT NE NV NH NJ NM NY NC ND OH OK OR PA RI SC SD TN TX UT VT VA WA WV WI WY";

pub(crate) struct GiftCards {
    os: Arc<dyn OsSecretStore>,
    piggy: OnceLock<Result<Piggy, AppError>>,
    serial: tokio::sync::Mutex<()>,
    /// Each order's quote window, on the monotonic clock from when its
    /// quote was created: a wall-clock change neither extends nor reopens
    /// it, and a restart forgets it, so the order needs a new quote.
    quotes: Mutex<HashMap<[u8; 32], tokio::time::Instant>>,
    /// Access tokens by wallet and account, never stored: a restart signs
    /// in again with the account's stored password.
    tokens: Mutex<HashMap<(WalletId, String), Token>>,
    /// Earlier builds' account and orders were moved into the wallets.
    migrated: AtomicBool,
}

struct Token {
    access: Zeroizing<String>,
    expires: tokio::time::Instant,
}

impl Token {
    fn new(access: Zeroizing<String>, expires_in: u64) -> Self {
        let lifetime = Duration::from_secs(expires_in.saturating_sub(15));
        Self {
            access,
            expires: tokio::time::Instant::now() + lifetime,
        }
    }
}

impl Default for GiftCards {
    fn default() -> Self {
        Self {
            os: Arc::new(KeyringOsStore::new()),
            piggy: OnceLock::new(),
            serial: tokio::sync::Mutex::new(()),
            quotes: Mutex::new(HashMap::new()),
            tokens: Mutex::new(HashMap::new()),
            migrated: AtomicBool::new(false),
        }
    }
}

impl GiftCards {
    fn quotes(&self) -> std::sync::MutexGuard<'_, HashMap<[u8; 32], tokio::time::Instant>> {
        self.quotes.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn start_quote(&self, key: [u8; 32]) {
        let now = tokio::time::Instant::now();
        let mut quotes = self.quotes();
        quotes.retain(|_, deadline| *deadline > now);
        quotes.insert(key, now + Duration::from_secs(INVOICE_WINDOW_SECS));
    }

    /// The order's quote deadline while it is still open.
    fn quote_deadline(&self, key: &[u8; 32]) -> Option<tokio::time::Instant> {
        let deadline = self.quotes().get(key).copied()?;
        (tokio::time::Instant::now() < deadline).then_some(deadline)
    }

    fn tokens(&self) -> std::sync::MutexGuard<'_, HashMap<(WalletId, String), Token>> {
        self.tokens.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn token(&self, wallet: WalletId, account: &str) -> Option<Zeroizing<String>> {
        let tokens = self.tokens();
        let token = tokens.get(&(wallet, account.to_owned()))?;
        (tokio::time::Instant::now() < token.expires).then(|| token.access.clone())
    }

    fn keep_token(&self, wallet: WalletId, account: String, token: Token) {
        self.tokens().insert((wallet, account), token);
    }

    fn forget_tokens(&self, wallet: WalletId) {
        self.tokens().retain(|(w, _), _| *w != wallet);
    }

    fn open_quote(&self, key: &[u8; 32]) -> Result<tokio::time::Instant, AppError> {
        self.quote_deadline(key)
            .ok_or_else(|| fail("The gift card quote expired. Get a new quote before paying."))
    }
}

#[derive(Clone, Deserialize, Serialize, TS)]
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

/// A PiggyCards account stored for the wallet; `id` names it for
/// `giftCardSignIn`.
#[derive(Serialize, TS)]
pub struct GiftStoredAccount {
    id: String,
    email: String,
    active: bool,
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
    #[serde(rename = "claimHost")]
    claim_host: Option<String>,
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
    store: WalletStore,
    cards: &'a GiftCards,
    piggy: &'a Piggy,
}

impl<'a> Context<'a> {
    async fn open(state: &'a AppState, wallet_id: &str) -> Result<Self, AppError> {
        Self::open_wallet(state, parse_wallet_id(wallet_id)?).await
    }

    async fn open_wallet(state: &'a AppState, wallet: WalletId) -> Result<Self, AppError> {
        let cards = &state.gift_cards;
        let guard = cards.serial.lock().await;
        let (session, epoch) = active(state).await?;
        if session.wallet_info(&wallet).is_err() || !session.vault().has_wallet_secret(&wallet.0) {
            return Err(fail("Choose a wallet that can send DASH to buy a gift card."));
        }
        let piggy = cards
            .piggy
            .get_or_init(Piggy::new)
            .as_ref()
            .map_err(Clone::clone)?;
        if !cards.migrated.load(Ordering::Acquire)
            && legacy::migrate(&state.data_root, &cards.os, &session).await?
        {
            cards.migrated.store(true, Ordering::Release);
        }
        Ok(Self {
            _guard: guard,
            store: WalletStore::new(Arc::clone(&session), wallet),
            session,
            epoch,
            cards,
            piggy,
        })
    }

    /// The order, its account and an access token for that account.
    async fn order(&self, id: &str) -> Result<(Order, Zeroizing<String>), AppError> {
        let book = self.store.read().await?;
        let order = book.order(id)?.clone();
        let account = book
            .account(&order.account)
            .ok_or_else(|| fail("The PiggyCards account of this order is unavailable."))?;
        let access = self.access(account).await?;
        Ok((order, access))
    }

    /// The order as the renderer sees it: a quote this process did not
    /// start, or one past its window, shows as expired.
    fn shown(&self, order: &Order) -> GiftOrder {
        let mut view = order.view.clone();
        if self.cards.quote_deadline(&order.key()).is_none() {
            view.expires_at = 0;
        }
        view
    }
}

impl GiftOrder {
    /// The order's invoice, once a fresh read shows PiggyCards still awaits
    /// exactly that payment.
    async fn revalidate(&self, piggy: &Piggy, access: &str) -> Result<GiftInvoice, AppError> {
        let invoice = self
            .invoice
            .as_ref()
            .ok_or_else(|| fail("The gift card invoice is unavailable."))?;
        let data = piggy.order(&self.id, access).await?;
        data.verify_quote(invoice)?;
        Ok(invoice.clone())
    }
}

impl piggy::OrderData {
    fn verify_quote(&self, invoice: &GiftInvoice) -> Result<(), AppError> {
        if !matches!(
            GiftOrderStatus::from(self.status.as_str()),
            GiftOrderStatus::AwaitingPayment
        ) {
            return Err(fail(
                "This gift card order is no longer awaiting payment. Refresh its status.",
            ));
        }
        let uri = self.pay_to.as_deref().ok_or_else(|| {
            fail("PiggyCards has not returned a payable invoice. Refresh the order before paying.")
        })?;
        let current = GiftInvoice::parse(uri, Network::Mainnet)?;
        if current.address != invoice.address || current.duffs != invoice.duffs {
            return Err(fail(
                "The gift card invoice changed. Get a new quote before paying.",
            ));
        }
        Ok(())
    }
}

pub(super) async fn check_before_prepare(
    state: &AppState,
    draft: u64,
) -> Result<Option<(String, u64, tokio::time::Instant)>, AppError> {
    let Some(key) = state.sends.invoice_key(draft)? else {
        return Ok(None);
    };
    let wallet = state.sends.for_payment(draft, Some(key))?.wallet_id();
    let ctx = Context::open_wallet(state, wallet).await?;
    let book = ctx.store.read().await?;
    let id = book
        .orders
        .iter()
        .find(|o| o.key() == key)
        .map(|o| o.view.id.clone())
        .ok_or_else(|| fail("Refresh the gift card order before paying."))?;
    let (order, access) = ctx.order(&id).await?;
    if ctx.session.invoice_payment(key).await?.is_some() {
        return Err(fail(
            "A payment was already started for this gift card order.",
        ));
    }
    let invoice = order.view.revalidate(ctx.piggy, &access).await?;
    let deadline = state.gift_cards.open_quote(&key)?;
    check_active(state, &ctx.session)?;
    Ok(Some((invoice.address, invoice.duffs, deadline)))
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

fn claim_url(text: &str) -> Option<reqwest::Url> {
    let url = reqwest::Url::parse(text).ok()?;
    let domain = url.domain()?;
    let suffix = domain.rsplit_once('.')?.1;
    if url.scheme() != "https"
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
        || matches!(suffix, "" | "local" | "localhost" | "internal" | "test")
    {
        return None;
    }
    Some(url)
}

fn invoice_key(account: &str, order: &str) -> [u8; 32] {
    sha256::Hash::hash(format!("piggycards.v1:{account}:{order}").as_bytes()).to_byte_array()
}

commands! {
    /// The wallet's PiggyCards sign-in state. Its accounts are sealed rows
    /// of the wallet: its backup carries them and removing it deletes them.
    #[facade("NetworkSession::vault", "Vault::status", "NetworkSession::gift_card_rows", "Vault::open_wallet_data")]
    fn gift_card_account(state, wallet_id: String) -> GiftAccount {
        let ctx = Context::open(&state, &wallet_id).await?;
        let account = ctx.account_status().await?;
        check_active(&state, &ctx.session)?;
        Ok(account)
    }

    /// The PiggyCards accounts stored for the wallet.
    #[facade("NetworkSession::vault", "Vault::status", "NetworkSession::gift_card_rows", "Vault::open_wallet_data")]
    fn gift_card_accounts(state, wallet_id: String) -> Vec<GiftStoredAccount> {
        let ctx = Context::open(&state, &wallet_id).await?;
        ctx.stored_accounts().await
    }

    /// Creates a PiggyCards account and starts email verification if required.
    #[facade("NetworkSession::vault", "Vault::status", "NetworkSession::write_gift_card_rows", "Vault::seal_wallet_data")]
    fn gift_card_signup(state, wallet_id: String, info: GiftSignup) -> GiftAccount {
        let ctx = Context::open(&state, &wallet_id).await?;
        ctx.signup(info).await
    }

    /// Verifies the email code, saves the generated password, and signs in.
    #[facade("NetworkSession::vault", "Vault::status", "NetworkSession::write_gift_card_rows", "Vault::seal_wallet_data")]
    fn gift_card_verify_email(state, wallet_id: String, code: String) -> GiftAccount {
        let code = Zeroizing::new(code);
        let ctx = Context::open(&state, &wallet_id).await?;
        ctx.verify_email(code).await
    }

    /// Signs in with a stored account's password: `account` (an id from
    /// `giftCardAccounts`) becomes the active one, or the active one.
    #[facade("NetworkSession::vault", "Vault::status", "NetworkSession::write_gift_card_rows")]
    fn gift_card_sign_in(state, wallet_id: String, account: Option<String>) -> GiftAccount {
        let ctx = Context::open(&state, &wallet_id).await?;
        ctx.sign_in(account).await
    }

    /// Ends the wallet's PiggyCards session; its accounts stay stored.
    #[facade("NetworkSession::vault", "Vault::status", "NetworkSession::write_gift_card_rows")]
    fn gift_card_sign_out(state, wallet_id: String) -> GiftAccount {
        let ctx = Context::open(&state, &wallet_id).await?;
        ctx.sign_out().await
    }

    /// PiggyCards merchants for the US catalog.
    #[facade("NetworkSession::vault", "Vault::status")]
    fn gift_card_brands(state, wallet_id: String) -> Vec<GiftBrand> {
        let ctx = Context::open(&state, &wallet_id).await?;
        let (_, access) = ctx.active_session().await?;
        ctx.piggy.brands(&access).await
    }

    /// In-stock USD products and denominations for a PiggyCards merchant.
    #[facade("NetworkSession::vault", "Vault::status")]
    fn gift_card_products(state, wallet_id: String, brand: String) -> Vec<GiftCardProduct> {
        checked_id(&brand)?;
        let ctx = Context::open(&state, &wallet_id).await?;
        let (_, access) = ctx.active_session().await?;
        ctx.piggy.products(&brand, &access).await
    }

    /// The wallet's gift card orders, of every account it stores, newest first.
    #[facade("NetworkSession::vault", "Vault::status", "NetworkSession::gift_card_rows")]
    fn gift_card_orders(state, wallet_id: String) -> Vec<GiftOrder> {
        let ctx = Context::open(&state, &wallet_id).await?;
        ctx.active_session().await?;
        let book = ctx.store.read().await?;
        Ok(book.orders.iter().map(|o| ctx.shown(o)).collect())
    }

    /// Creates a single-card order, recording it in the wallet before any payment.
    #[facade("NetworkSession::vault", "Vault::status", "NetworkSession::write_gift_card_rows")]
    fn gift_card_create_order(state, wallet_id: String, brand: String, product_id: u32, cents: u32) -> GiftOrder {
        checked_id(&brand)?;
        let ctx = Context::open(&state, &wallet_id).await?;
        let (account, access) = ctx.active_session().await?;
        let product = ctx.piggy.products(&brand, &access).await?.into_iter()
            .find(|p| p.id == product_id && p.allows(cents))
            .ok_or_else(|| fail("This gift card value is unavailable."))?;
        let book = ctx.store.read().await?;
        check_active(&state, &ctx.session)?;
        let created = ctx.piggy.create_order(&access, product.id, cents, &account.signup).await?;
        checked_id(&created.id)?;
        if book.orders.iter().any(|o| o.view.id == created.id) {
            return Err(fail("PiggyCards returned an order that already exists. Check My gift cards."));
        }
        let invoice = created.pay_to.as_deref()
            .and_then(|uri| GiftInvoice::parse(uri, Network::Mainnet).ok());
        let created_at = now();
        let view = GiftOrder {
            id: created.id, name: product.name, cents,
            expires_at: created_at + INVOICE_WINDOW_SECS, invoice, txid: None,
        };
        let order = Order { view: view.clone(), account: account.key(), created_at };
        ctx.store.write(&[Change::Order(&order)]).await?;
        state.gift_cards.start_quote(order.key());
        Ok(view)
    }

    /// Reads fulfillment for a known order. Codes and PINs are fetched on demand.
    #[facade("NetworkSession::vault", "Vault::status", "NetworkSession::invoice_payment")]
    fn gift_card_details(state, wallet_id: String, order_id: String) -> GiftOrderDetails {
        let ctx = Context::open(&state, &wallet_id).await?;
        let (mut order, access) = ctx.order(&order_id).await?;
        let payment = ctx.session.invoice_payment(order.key()).await?;
        let data = ctx.piggy.order(&order_id, &access).await?;
        let status = GiftOrderStatus::from(data.status.as_str());
        check_active(&state, &ctx.session)?;
        let quote = state.gift_cards.quote_deadline(&order.key());
        let missing_invoice = order.view.invoice.is_none() && payment.is_none()
            && matches!(status, GiftOrderStatus::AwaitingPayment) && quote.is_some();
        if missing_invoice {
            order.view.invoice = data.pay_to.as_deref().and_then(|uri| GiftInvoice::parse(uri, Network::Mainnet).ok());
            if order.view.invoice.is_some() {
                ctx.store.write(&[Change::Order(&order)]).await?;
            }
        }
        let mut view = ctx.shown(&order);
        view.txid = payment;
        if view.invoice.as_ref().is_some_and(|known| data.verify_quote(known).is_err()) {
            view.expires_at = 0;
        }
        let cards = if matches!(status, GiftOrderStatus::Complete) {
            data.cards.into_iter().map(|c| GiftCardSecret {
                name: c.name,
                code: c.claim_code,
                pin: c.claim_pin,
                claim_host: c.claim_link.as_deref().and_then(claim_url)
                    .and_then(|url| url.domain().map(str::to_owned)),
            }).collect()
        } else {
            Vec::new()
        };
        Ok(GiftOrderDetails { order: view, status, cards })
    }

    /// Revalidates an order before manually relaying its previously signed payment.
    #[facade("NetworkSession::resend_invoice_transaction", "NetworkSession::invoice_payment")]
    fn gift_card_resend(state, wallet_id: String, order_id: String) -> () {
        let ctx = Context::open(&state, &wallet_id).await?;
        let (order, access) = ctx.order(&order_id).await?;
        let key = order.key();
        let txid = ctx.session.invoice_payment(key).await?.ok_or_else(|| fail("No payment was recorded for this order."))?;
        order.view.revalidate(ctx.piggy, &access).await?;
        check_active(&state, &ctx.session)?;
        let txid = txid.parse().map_err(|_| fail("The recorded transaction ID is invalid."))?;
        ctx.session.resend_invoice_transaction(ctx.store.wallet(), txid, key).await?;
        Ok(())
    }

    /// Opens a completed card's provider-issued HTTPS redemption link.
    #[facade("NetworkSession::vault", "Vault::status")]
    fn gift_card_open_claim(state, wallet_id: String, order_id: String, card: u32, host: String) -> () {
        let ctx = Context::open(&state, &wallet_id).await?;
        let (_, access) = ctx.order(&order_id).await?;
        let data = ctx.piggy.order(&order_id, &access).await?;
        if !matches!(GiftOrderStatus::from(data.status.as_str()), GiftOrderStatus::Complete) {
            return Err(fail("PiggyCards has not completed this gift card."));
        }
        let url = data.cards.get(card as usize).and_then(|card| card.claim_link.as_deref())
            .and_then(claim_url).filter(|url| url.domain() == Some(host.as_str()))
            .ok_or_else(|| fail("The redemption link changed or is unavailable. Refresh the order."))?;
        check_active(&state, &ctx.session)?;
        let open_failed = || fail("Could not open the gift card redemption link.");
        tauri::async_runtime::spawn_blocking(move || open::that_detached(url.as_str()))
            .await.map_err(|_| open_failed())?.map_err(|_| open_failed())
    }

    /// Opens a normal Pay draft bound to a known PiggyCards order.
    #[facade("NetworkSession::vault", "Vault::status", "NetworkSession::invoice_payment", "NetworkSession::new_tx_draft")]
    fn gift_card_draft(state, wallet_id: String, order_id: String) -> u64 {
        let ctx = Context::open(&state, &wallet_id).await?;
        let book = ctx.store.read().await?;
        let order = book.order(&order_id)?;
        let key = order.key();
        if state.gift_cards.quote_deadline(&key).is_none() || order.view.invoice.is_none() {
            return Err(fail("The gift card invoice is unavailable or has expired."));
        }
        if ctx.session.invoice_payment(key).await?.is_some() {
            return Err(fail("A payment was already started for this gift card order."));
        }
        check_active(&state, &ctx.session)?;
        let draft = ctx.session.new_tx_draft(ctx.store.wallet())?;
        state.sends.insert_send_draft(ctx.epoch, draft, Some(key))
    }

    /// Broadcasts an ordinary Pay draft bound to a PiggyCards invoice.
    #[facade("NetworkSession::vault", "Vault::status", "TxDraft::wallet_id", "PreparedTx::summary", "NetworkSession::invoice_payment", "TxDraft::broadcast_invoice")]
    fn gift_card_broadcast(state, order_id: String, draft: u64, prepared: u64) -> BroadcastOutcome {
        let key = state.sends.invoice_key(draft)?
            .ok_or_else(|| fail("This draft belongs to a different payment flow."))?;
        let wallet = state.sends.for_payment(draft, Some(key))?.wallet_id();
        let ctx = Context::open_wallet(&state, wallet).await?;
        let (order, access) = ctx.order(&order_id).await?;
        if order.key() != key {
            return Err(fail("This draft belongs to a different payment flow."));
        }
        let tx = send::prepared_of(&state, draft, prepared)?;
        let previous = ctx.session.invoice_payment(key).await?;
        if previous.as_deref().is_some_and(|id| id != tx.summary().txid) {
            return Err(fail("A payment was already started for this gift card order."));
        }
        let invoice = order.view.revalidate(ctx.piggy, &access).await?;
        check_active(&state, &ctx.session)?;
        let d = state.sends.for_payment(draft, Some(key))?;
        let tx = send::prepared_of(&state, draft, prepared)?;
        invoice.verify(&tx.summary().outputs)?;
        Ok(send::broadcast(&state, d, tx, prepared, Some(key)).await?.into())
    }
}
