//! Wallets: the registry, creation and import, balances, receive addresses
//! and payment requests, the address book and labels, load/unload, message
//! signing and verification.

use dw_engine::{AddressChain, BookEntryInfo, BookPurpose, WalletLoadState};
use serde::{Deserialize, Serialize};
use ts_rs::TS;
use zeroize::Zeroizing;

use crate::dto::{Balances, WalletSummary, mirror_enum};
use crate::ipc::commands;
use crate::state::parse_wallet_id;

pub(crate) const EXCLUDED: &[(&str, &str)] = &[];

/// A wallet made by `createWallet`. The phrase is secret: the UI shows it
/// once and checks the user wrote it down (QT-103, IOS-004).
#[derive(Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct CreatedWallet {
    pub wallet_id: String,
    /// The recovery phrase. The vault already holds it.
    pub mnemonic: String,
}

impl std::fmt::Debug for CreatedWallet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CreatedWallet")
            .field("wallet_id", &self.wallet_id)
            .field("mnemonic", &"<redacted>")
            .finish()
    }
}

impl From<dw_engine::CreatedWallet> for CreatedWallet {
    fn from(w: dw_engine::CreatedWallet) -> Self {
        let dw_engine::CreatedWallet {
            wallet_id,
            mnemonic,
        } = w;
        CreatedWallet {
            wallet_id: wallet_id.to_string(),
            mnemonic: (*mnemonic).clone(),
        }
    }
}

/// Options of `importWallet`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
pub struct ImportOptions {
    /// Display name, 1-64 characters after trimming; `null`: "Wallet N".
    pub name: Option<String>,
    /// First block to scan. `0` is genesis; `null`: the SPV tip, or the
    /// latest checkpoint (right for a new phrase).
    pub birth_height: Option<u32>,
    /// Derive the seed with Dash Core's BIP39 quirks (weak checksum, no
    /// NFKD, salt cut at 256 bytes; QT-104).
    pub core_compat: bool,
    /// Address lookahead (gap limit) of the BIP44 chains, 1..=1000; `null`:
    /// 30, or 1000 with `coreCompat` (QT-105).
    pub lookahead: Option<u32>,
}

impl From<ImportOptions> for dw_engine::ImportOptions {
    fn from(o: ImportOptions) -> Self {
        let ImportOptions {
            name,
            birth_height,
            core_compat,
            lookahead,
        } = o;
        dw_engine::ImportOptions {
            birth_height,
            core_compat,
            name,
            lookahead,
        }
    }
}

/// Options of `importWatchOnly` (QT-114).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
pub struct WatchOnlyOptions {
    /// 1-64 characters after trimming; `null`: "Wallet N".
    pub name: Option<String>,
    /// First block to scan; `null`: genesis (an xpub has no birth date).
    pub birth_height: Option<u32>,
    /// Gap limit of the watched chains, 1..=1000; `null`: 30.
    pub lookahead: Option<u32>,
}

impl From<WatchOnlyOptions> for dw_engine::WatchOnlyOptions {
    fn from(o: WatchOnlyOptions) -> Self {
        let WatchOnlyOptions {
            name,
            birth_height,
            lookahead,
        } = o;
        dw_engine::WatchOnlyOptions {
            name,
            birth_height,
            lookahead,
        }
    }
}

/// External (receiving) or internal (change) BIP44 chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum AddressChainName {
    Receiving,
    Change,
}

mirror_enum!(AddressChain => AddressChainName { Receiving, Change });
mirror_enum!(AddressChainName => AddressChain { Receiving, Change });

/// One address of the wallet's BIP44 account 0.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct AddressInfo {
    pub address: String,
    pub chain: AddressChainName,
    pub index: u32,
    /// e.g. `m/44'/5'/0'/0/7`.
    pub derivation_path: String,
    /// Has received funds at least once.
    pub used: bool,
    pub label: Option<String>,
    /// Duffs held by this address.
    pub balance: Option<u64>,
    pub tx_count: u32,
}

impl From<dw_engine::AddressInfo> for AddressInfo {
    fn from(a: dw_engine::AddressInfo) -> Self {
        let dw_engine::AddressInfo {
            address,
            chain,
            index,
            derivation_path,
            used,
            label,
            balance,
            tx_count,
        } = a;
        AddressInfo {
            address,
            chain: chain.into(),
            index,
            derivation_path,
            used,
            label,
            balance,
            tx_count,
        }
    }
}

/// Address list filter; `null` fields match either.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
pub struct AddressFilter {
    pub chain: Option<AddressChainName>,
    pub used: Option<bool>,
}

impl From<AddressFilter> for dw_engine::AddressFilter {
    fn from(f: AddressFilter) -> Self {
        let AddressFilter { chain, used } = f;
        dw_engine::AddressFilter {
            chain: chain.map(Into::into),
            used,
        }
    }
}

/// A stored payment request (QT-081/083, IOS-055).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ReceiveRequest {
    pub id: u64,
    /// UNIX seconds.
    pub created_at: u64,
    pub address: String,
    /// Duffs; `null`: any amount.
    pub amount: Option<u64>,
    pub label: Option<String>,
    pub message: Option<String>,
    /// The `dash:` URI of the request.
    pub uri: String,
}

impl From<dw_engine::ReceiveRequest> for ReceiveRequest {
    fn from(r: dw_engine::ReceiveRequest) -> Self {
        let dw_engine::ReceiveRequest {
            id,
            created_at,
            address,
            amount,
            label,
            message,
            uri,
        } = r;
        ReceiveRequest {
            id,
            created_at,
            address,
            amount,
            label,
            message,
            uri,
        }
    }
}

/// dash-qt's address-book purpose.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum AddressPurposeName {
    /// Someone else's address (the sending tab).
    Send,
    /// One of this wallet's addresses (the receiving tab).
    Receive,
}

mirror_enum!(BookPurpose => AddressPurposeName { Send, Receive });
mirror_enum!(AddressPurposeName => BookPurpose { Send, Receive });

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct AddressBookEntry {
    pub address: String,
    /// Empty when unlabelled (dash-qt shows "(no label)").
    pub label: String,
    pub purpose: AddressPurposeName,
    /// UNIX seconds.
    pub created_at: Option<u64>,
}

impl From<BookEntryInfo> for AddressBookEntry {
    fn from(e: BookEntryInfo) -> Self {
        let BookEntryInfo {
            address,
            label,
            purpose,
            created_at,
        } = e;
        AddressBookEntry {
            address,
            label,
            purpose: purpose.into(),
            created_at,
        }
    }
}

/// Load state of one registered wallet (QT-101). An unloaded wallet keeps
/// its data but is absent from `listWallets`, and every wallet-scoped call
/// for it is `wallet_not_found`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct WalletLoadStatus {
    pub wallet_id: String,
    pub name: String,
    pub loaded: bool,
    /// Loaded when the network opens.
    pub load_on_startup: bool,
    pub watch_only: bool,
}

impl From<WalletLoadState> for WalletLoadStatus {
    fn from(w: WalletLoadState) -> Self {
        let WalletLoadState {
            wallet_id,
            name,
            loaded,
            load_on_startup,
            watch_only,
        } = w;
        WalletLoadStatus {
            wallet_id: wallet_id.to_string(),
            name,
            loaded,
            load_on_startup,
            watch_only,
        }
    }
}

/// An account-level extended public key (IOS-111).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct AccountXpub {
    /// BIP44 account index.
    pub account: u32,
    /// `m/44'/5'/0'` on mainnet, `m/44'/1'/0'` elsewhere.
    pub derivation_path: String,
    /// `xpub...` on mainnet, `tpub...` elsewhere.
    pub xpub: String,
}

impl From<dw_engine::AccountXpub> for AccountXpub {
    fn from(x: dw_engine::AccountXpub) -> Self {
        let dw_engine::AccountXpub {
            account,
            derivation_path,
            xpub,
        } = x;
        AccountXpub {
            account,
            derivation_path,
            xpub,
        }
    }
}

/// What a verification found (dash-qt's Verify Message tab).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum VerifyOutcomeName {
    Verified,
    /// The address does not decode on the network.
    InvalidAddress,
    /// The address is valid but not P2PKH.
    AddressNoKey,
    /// The signature is not strict base64.
    MalformedSignature,
    /// No public key could be recovered from the signature.
    PubkeyNotRecovered,
    /// The recovered key does not own the address.
    NotSigned,
}

/// The result of `verifyMessage`. A failed check is a result, not an error:
/// the Verify Message tab shows each failure's own text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct VerifyMessageResult {
    pub outcome: VerifyOutcomeName,
    /// dash-qt's status text for the outcome.
    pub text: String,
}

/// dash-qt's `MessageVerify`.
fn verify(
    address: &str,
    signature: &str,
    message: &str,
    network: dashcore::Network,
) -> VerifyMessageResult {
    use dw_message::VerifyError as E;
    let (outcome, text) = match dw_message::verify_message(
        address,
        signature,
        message.as_bytes(),
        network,
    ) {
        Ok(()) => (VerifyOutcomeName::Verified, dw_message::GUI_VERIFIED),
        Err(e) => (
            match e {
                E::InvalidAddress => VerifyOutcomeName::InvalidAddress,
                E::AddressNoKey => VerifyOutcomeName::AddressNoKey,
                E::MalformedSignature => VerifyOutcomeName::MalformedSignature,
                E::PubkeyNotRecovered => VerifyOutcomeName::PubkeyNotRecovered,
                E::NotSigned => VerifyOutcomeName::NotSigned,
            },
            e.gui_message(),
        ),
    };
    VerifyMessageResult {
        outcome,
        text: text.to_owned(),
    }
}

commands! {
    #[facade("NetworkSession::wallet_infos")]
    fn list_wallets(state) -> Vec<WalletSummary> {
        let infos = state.session().await?.wallet_infos()?;
        Ok(infos.into_iter().map(WalletSummary::from).collect())
    }

    /// One registered wallet.
    #[facade("NetworkSession::wallet_info")]
    fn wallet_info(state, wallet_id: String) -> WalletSummary {
        let id = parse_wallet_id(&wallet_id)?;
        Ok(state.session().await?.wallet_info(&id)?.into())
    }

    /// Balance buckets; `null` until the scan reaches the birth height.
    #[facade("NetworkSession::balances")]
    fn balance(state, wallet_id: String) -> Option<Balances> {
        let id = parse_wallet_id(&wallet_id)?;
        Ok(state.session().await?.balances(&id)?.map(Balances::from))
    }

    /// Height up to which the wallet's compact filters were scanned; `null`
    /// when the wallet has no scan state.
    #[facade("NetworkSession::wallet_scan_height")]
    fn wallet_scan_height(state, wallet_id: String) -> Option<u32> {
        let id = parse_wallet_id(&wallet_id)?;
        Ok(state.session().await?.wallet_scan_height(&id))
    }

    /// Creates a wallet from a fresh English phrase of 12, 15, 18, 21 or 24
    /// words and stores its seed in the vault, which must hold its data
    /// key. The phrase comes back so the UI can show it; it is secret.
    #[facade("NetworkSession::create_wallet")]
    fn create_wallet(state, word_count: u8) -> CreatedWallet {
        Ok(state.session().await?.create_wallet(word_count).await?.into())
    }

    /// Adds the wallet of a phrase and BIP39 passphrase (seed-safety order:
    /// the vault stores and reads back the secrets before the wallet is
    /// registered). Returns the wallet id.
    #[facade("NetworkSession::import_wallet")]
    fn import_wallet(
        state,
        mnemonic: String,
        bip39_passphrase: Option<String>,
        options: ImportOptions
    ) -> String {
        let mnemonic = Zeroizing::new(mnemonic.into_bytes());
        let bip39_passphrase = Zeroizing::new(bip39_passphrase.unwrap_or_default().into_bytes());
        let id = state
            .session()
            .await?
            .import_wallet(mnemonic, bip39_passphrase, options.into())
            .await?;
        Ok(id.to_string())
    }

    /// Registers a watch-only wallet from a BIP44 account xpub (QT-114).
    /// Returns the wallet id, a digest of the account key.
    #[facade("NetworkSession::import_watch_only")]
    fn import_watch_only(state, xpub: String, options: WatchOnlyOptions) -> String {
        let id = state
            .session()
            .await?
            .import_watch_only(xpub, options.into())
            .await?;
        Ok(id.to_string())
    }

    /// Deletes the wallet's vault records, then unloads it and deletes its
    /// state and metadata (QT-101, IOS-109). Needs a `wipe` grant for it.
    #[facade("NetworkSession::remove_wallet")]
    fn remove_wallet(state, wallet_id: String, grant_id: String) -> () {
        let id = parse_wallet_id(&wallet_id)?;
        Ok(state.session().await?.remove_wallet(id, grant_id).await?)
    }

    /// Sets the display name (1-64 characters after trimming).
    #[facade("NetworkSession::rename_wallet")]
    fn rename_wallet(state, wallet_id: String, name: String) -> () {
        let id = parse_wallet_id(&wallet_id)?;
        Ok(state.session().await?.rename_wallet(id, name).await?)
    }

    /// Every registered wallet with its load state, in creation order.
    #[facade("NetworkSession::wallet_load_states")]
    fn wallet_load_states(state) -> Vec<WalletLoadStatus> {
        let states = state.session().await?.wallet_load_states()?;
        Ok(states.into_iter().map(WalletLoadStatus::from).collect())
    }

    /// dash-qt "Open Wallet": loads a registered, unloaded wallet and resumes
    /// its scan. Idempotent.
    #[facade("NetworkSession::load_wallet")]
    fn load_wallet(state, wallet_id: String) -> () {
        let id = parse_wallet_id(&wallet_id)?;
        Ok(state.session().await?.load_wallet(id).await?)
    }

    /// dash-qt "Close Wallet": stops tracking the wallet and drops it from
    /// memory; its data and vault records stay. Idempotent.
    #[facade("NetworkSession::unload_wallet")]
    fn unload_wallet(state, wallet_id: String) -> () {
        let id = parse_wallet_id(&wallet_id)?;
        Ok(state.session().await?.unload_wallet(id).await?)
    }

    /// Adds or removes the wallet from the load-on-startup list.
    #[facade("NetworkSession::set_load_on_startup")]
    fn set_load_on_startup(state, wallet_id: String, load_on_startup: bool) -> () {
        let id = parse_wallet_id(&wallet_id)?;
        Ok(state
            .session()
            .await?
            .set_load_on_startup(id, load_on_startup)
            .await?)
    }

    /// The BIP44 account xpub (IOS-111). Public data, no grant.
    #[facade("NetworkSession::account_xpub")]
    fn account_xpub(state, wallet_id: String, account: u32) -> AccountXpub {
        let id = parse_wallet_id(&wallet_id)?;
        Ok(state.session().await?.account_xpub(id, account).await?.into())
    }

    /// Dash Core `signmessage` (QT-099): the base64 compact signature of
    /// `message` by the key of one of the wallet's P2PKH addresses. Needs a
    /// `signMessage` grant for the wallet.
    #[facade("NetworkSession::sign_message")]
    fn sign_message(
        state,
        wallet_id: String,
        address: String,
        message: String,
        grant_id: String
    ) -> String {
        let id = parse_wallet_id(&wallet_id)?;
        let signature = state
            .session()
            .await?
            .sign_message(id, address, message.into_bytes(), grant_id)
            .await?;
        Ok(signature)
    }

    /// `verifymessage` (QT-100) on the active network. Needs no wallet.
    #[facade()]
    fn verify_message(
        state,
        address: String,
        signature: String,
        message: String
    ) -> VerifyMessageResult {
        let network = state.session().await?.network().core_network();
        Ok(verify(&address, &signature, &message, network))
    }

    /// The first unused receiving address (IOS-053). Query again after a
    /// `historyChanged` event: that is how the shown address rotates once paid.
    #[facade("NetworkSession::current_receive_address")]
    fn current_receive_address(state, wallet_id: String) -> AddressInfo {
        let id = parse_wallet_id(&wallet_id)?;
        Ok(state.session().await?.current_receive_address(id).await?.into())
    }

    /// Issues a receiving address that was never issued before and labels
    /// it (QT-081). `gap_limit` once every address inside the gap limit is
    /// issued.
    #[facade("NetworkSession::next_receive_address")]
    fn next_receive_address(state, wallet_id: String, label: Option<String>) -> AddressInfo {
        let id = parse_wallet_id(&wallet_id)?;
        Ok(state
            .session()
            .await?
            .next_receive_address(id, label)
            .await?
            .into())
    }

    /// Addresses of the wallet's BIP44 account 0 (QT-096), by chain then index.
    #[facade("NetworkSession::addresses")]
    fn addresses(state, wallet_id: String, filter: AddressFilter) -> Vec<AddressInfo> {
        let id = parse_wallet_id(&wallet_id)?;
        let addresses = state.session().await?.addresses(id, filter.into()).await?;
        Ok(addresses.into_iter().map(AddressInfo::from).collect())
    }

    /// Stores a payment request on a freshly issued address. An amount of 0
    /// means any amount.
    #[facade("NetworkSession::create_receive_request")]
    fn create_receive_request(
        state,
        wallet_id: String,
        amount: Option<u64>,
        label: Option<String>,
        message: Option<String>
    ) -> ReceiveRequest {
        let id = parse_wallet_id(&wallet_id)?;
        Ok(state
            .session()
            .await?
            .create_receive_request(id, amount, label, message)
            .await?
            .into())
    }

    /// Stored payment requests, newest first (QT-083).
    #[facade("NetworkSession::receive_requests")]
    fn receive_requests(state, wallet_id: String) -> Vec<ReceiveRequest> {
        let id = parse_wallet_id(&wallet_id)?;
        let requests = state.session().await?.receive_requests(id).await?;
        Ok(requests.into_iter().map(ReceiveRequest::from).collect())
    }

    #[facade("NetworkSession::delete_receive_request")]
    fn delete_receive_request(state, wallet_id: String, request_id: u64) -> () {
        let id = parse_wallet_id(&wallet_id)?;
        Ok(state
            .session()
            .await?
            .delete_receive_request(id, request_id)
            .await?)
    }

    /// Address-book entries, sorted by label then address. `search` is
    /// dash-qt's case-insensitive wildcard match (`*`, `?`) on label or address.
    #[facade("NetworkSession::address_book")]
    fn address_book(
        state,
        wallet_id: String,
        purpose: Option<AddressPurposeName>,
        search: Option<String>
    ) -> Vec<AddressBookEntry> {
        let id = parse_wallet_id(&wallet_id)?;
        let entries = state
            .session()
            .await?
            .address_book(id, purpose.map(Into::into), search)
            .await?;
        Ok(entries.into_iter().map(AddressBookEntry::from).collect())
    }

    /// Adds or relabels an entry. A send entry for one of the wallet's own
    /// addresses is refused; a receive entry must be one of them. An address
    /// already in the book is refused unless `replace` is set with the same
    /// purpose.
    #[facade("NetworkSession::save_address_book_entry")]
    fn save_address_book_entry(
        state,
        wallet_id: String,
        address: String,
        label: String,
        purpose: AddressPurposeName,
        replace: bool
    ) -> AddressBookEntry {
        let id = parse_wallet_id(&wallet_id)?;
        Ok(state
            .session()
            .await?
            .save_address_book_entry(id, address, label, purpose.into(), replace)
            .await?
            .into())
    }

    /// Deletes a send entry.
    #[facade("NetworkSession::delete_address_book_entry")]
    fn delete_address_book_entry(state, wallet_id: String, address: String) -> () {
        let id = parse_wallet_id(&wallet_id)?;
        Ok(state
            .session()
            .await?
            .delete_address_book_entry(id, address)
            .await?)
    }

    /// Sets, or clears (`null` or empty), the label of a transaction.
    #[facade("NetworkSession::set_tx_label")]
    fn set_tx_label(state, wallet_id: String, txid: String, label: Option<String>) -> () {
        let id = parse_wallet_id(&wallet_id)?;
        Ok(state.session().await?.set_tx_label(id, txid, label).await?)
    }

    /// The label of a transaction, if any.
    #[facade("NetworkSession::tx_label")]
    fn tx_label(state, wallet_id: String, txid: String) -> Option<String> {
        let id = parse_wallet_id(&wallet_id)?;
        Ok(state.session().await?.tx_label(id, txid).await?)
    }

    /// The message stored with a sent transaction (QT-054).
    #[facade("NetworkSession::tx_message")]
    fn tx_message(state, wallet_id: String, txid: String) -> Option<String> {
        let id = parse_wallet_id(&wallet_id)?;
        Ok(state.session().await?.tx_message(id, txid).await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // testdata/message_cases.json, produced by dashd 24 signmessagewithprivkey.
    const ADDRESS: &str = "yQWsoTNJq59DqBg4Z2Qup3k3qchPaWz29n";
    const MESSAGE: &str = "Trust no one";
    const SIGNATURE: &str =
        "IIOzMDkvw3GtLWXkeEYRRRH53MOLHM44sJ428Nu4NNacTPJTGcKesMJ+3s3OadYK34tpSQIhu922EviNNWTsiQg=";

    #[test]
    fn verify_reports_each_outcome_with_dash_qts_text() {
        let net = dashcore::Network::Regtest;
        let ok = verify(ADDRESS, SIGNATURE, MESSAGE, net);
        assert_eq!(ok.outcome, VerifyOutcomeName::Verified);
        assert_eq!(ok.text, dw_message::GUI_VERIFIED);

        let wrong = verify(ADDRESS, SIGNATURE, "Trust everyone", net);
        assert_eq!(wrong.outcome, VerifyOutcomeName::NotSigned);
        assert_eq!(wrong.text, "Message verification failed.");

        assert_eq!(
            verify("not-an-address", SIGNATURE, MESSAGE, net).outcome,
            VerifyOutcomeName::InvalidAddress
        );
        assert_eq!(
            verify(ADDRESS, "***", MESSAGE, net).outcome,
            VerifyOutcomeName::MalformedSignature
        );
        assert_eq!(
            serde_json::to_value(wrong).unwrap(),
            serde_json::json!({ "outcome": "notSigned", "text": "Message verification failed." })
        );
    }

    #[test]
    fn options_map_every_field() {
        let o: ImportOptions = serde_json::from_str(
            r#"{"name":"Main","birthHeight":0,"coreCompat":true,"lookahead":1000}"#,
        )
        .unwrap();
        let e = dw_engine::ImportOptions::from(o);
        assert_eq!(e.name.as_deref(), Some("Main"));
        assert_eq!(e.birth_height, Some(0));
        assert!(e.core_compat);
        assert_eq!(e.lookahead, Some(1000));
        // A renderer may leave options out.
        let o: ImportOptions = serde_json::from_str("{}").unwrap();
        assert_eq!(o, ImportOptions::default());
        let w: WatchOnlyOptions = serde_json::from_str(r#"{"lookahead":5}"#).unwrap();
        assert_eq!(dw_engine::WatchOnlyOptions::from(w).lookahead, Some(5));
    }

    #[test]
    fn address_filters_and_purposes_parse() {
        let f: AddressFilter = serde_json::from_str(r#"{"chain":"change","used":false}"#).unwrap();
        let e = dw_engine::AddressFilter::from(f);
        assert_eq!(e.chain, Some(AddressChain::Change));
        assert_eq!(e.used, Some(false));
        assert_eq!(
            serde_json::from_str::<AddressPurposeName>("\"receive\"").unwrap(),
            AddressPurposeName::Receive
        );
        assert_eq!(BookPurpose::from(AddressPurposeName::Send), BookPurpose::Send);
        assert!(serde_json::from_str::<AddressChainName>("\"internal\"").is_err());
    }

    #[test]
    fn a_created_wallet_keeps_its_phrase_out_of_logs() {
        let created = CreatedWallet::from(dw_engine::CreatedWallet {
            wallet_id: dw_engine::WalletId([0x11; 32]),
            mnemonic: Zeroizing::new("secret words".into()),
        });
        assert_eq!(created.wallet_id, "11".repeat(32));
        assert_eq!(created.mnemonic, "secret words");
        assert!(!format!("{created:?}").contains("secret"));
    }

    #[test]
    fn load_states_map_to_hex_ids() {
        let s = WalletLoadStatus::from(WalletLoadState {
            wallet_id: dw_engine::WalletId([0x22; 32]),
            name: "W".into(),
            loaded: false,
            load_on_startup: true,
            watch_only: true,
        });
        assert_eq!(s.wallet_id, "22".repeat(32));
        assert!(!s.loaded && s.load_on_startup && s.watch_only);
    }
}
