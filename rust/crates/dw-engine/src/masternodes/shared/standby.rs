//! The standby (MG-04b §9.4, D19, D21, D22, D32, slice 2b): dash-qt's pair
//! of signed unilateral dissolutions kept for later
//! (`DissolveDialog::createStandby`, rc.3 `qt/sharedmndialogs.cpp:1634-1668`),
//! stored, exported in Core's file, imported from a paste, and sent as an
//! MG-04 operation of kind `dissolve`.
//!
//! Every call reads the registration from the wallet's history (D29), not a
//! session, so a restored wallet signs, imports and sends one as the
//! wallet that registered (D19): a share is this wallet's when its owner
//! key is a keychain owner key (D10).

use std::collections::HashSet;
use std::sync::Arc;

use dashcore::hashes::Hash;
use dashcore::{OutPoint, PubkeyHash, ScriptBuf, Txid};
use dw_appdb::StandbyRow;
use dw_protx::shared::dissolve::{Dissolution, penalty_free_from};
use dw_protx::shared::rules::SharedMn;
use dw_protx::shared::session::funding::decode_shared_protx;
use dw_protx::shared::standby::{self, Refusal, Variant};
use dw_protx::shared::{CollateralShare, ProDisTx, SpecialTx};
use dw_units::{Chain, SeparatorStyle, Unit};

use super::super::protx::build::{Context, invalid, refused};
use super::super::protx::charge_fee;
use super::super::protx::ops;
use super::super::protx_records::{ProTxOperation, ProTxTerms};
use super::super::records::MasternodeType;
use super::super::shared_records::{
    DissolveRequest, SharedImport, SharedNext, SharedSession, SharedStage, SharedWarning, Standby,
    StandbyDescription, StandbyFile, StandbySource, StandbyVariant,
};
use super::super::{MasternodesError, display_hex};
use super::dissolve::{
    DUST_REFUSAL, DissolveRecord, HandOff, actor_warnings, address_of, build_refusal,
    check_relay_fee, listed_above, share_returns,
};
use super::flow::signer_refusal;
use super::store::message_refusal;
use crate::coins::now_secs;
use crate::csv_export::iso_local;
use crate::platform::{BudgetPurpose, FlowKind, PlatformError};
use crate::send::lease_failure;
use crate::{EngineError, NetworkSession, WalletId};

/// `SECONDS_PER_BLOCK` of dash-qt's `ApproxBlockDate` (`:76`).
const SECONDS_PER_BLOCK: u64 = 150;

/// A shared masternode as the standby calls read it: its registration from
/// the wallet's history and the height of the block holding it (D29), the
/// list's voting key and service, one block deep and listed (m-16).
pub(super) struct Registered {
    pub hash: [u8; 32],
    collateral: OutPoint,
    pub shares: Vec<CollateralShare>,
    registered_height: u32,
    early_period_blocks: u32,
    early_penalty: i64,
    voting_key_id: [u8; 20],
    operator_public_key: [u8; 48],
    operator_reward: u16,
    service: String,
    /// The tip a call judges by: the list's height, or the headers' when
    /// the list lags them (as D81's bound), so a standby is judged where
    /// dash-qt judges it, at the chain's tip.
    pub tip: u32,
    /// The height a transaction sent now is mined at.
    pub spend_height: u32,
}

impl Registered {
    pub fn mn(&self) -> SharedMn<'_> {
        SharedMn {
            pro_tx_hash: Txid::from_byte_array(self.hash),
            collateral: self.collateral,
            shares: &self.shares,
            registered_height: self.registered_height,
            early_period_blocks: self.early_period_blocks,
            early_penalty: self.early_penalty,
            voting_key_id: self.voting_key_id,
        }
    }

    /// The `dissolve` row's terms for `actor`, as 2a's (`plan_dissolve`).
    fn terms(
        &self,
        actor: u16,
        owner_index: Option<u32>,
        network: dashcore::Network,
    ) -> ProTxTerms {
        let share = &self.shares[usize::from(actor)];
        let p2pkh = |id: [u8; 20]| p2pkh_address(id, network);
        ProTxTerms {
            masternode_type: MasternodeType::Regular,
            version: ProDisTx::CURRENT_VERSION,
            service: self.service.clone(),
            owner_address: p2pkh(share.owner_key_id),
            owner_index,
            voting_address: p2pkh(self.voting_key_id),
            voting_index: None,
            operator_public_key: hex::encode(self.operator_public_key),
            operator_index: None,
            operator_reward: self.operator_reward,
            payout_address: address_of(&share.refund_script, network).unwrap_or_default(),
            collateral: None,
            collateral_outpoint: Some(format!("{}-{}", self.collateral.txid, self.collateral.vout)),
            collateral_address: None,
            platform: None,
            platform_node_index: None,
        }
    }
}

fn p2pkh_address(id: [u8; 20], network: dashcore::Network) -> String {
    address_of(
        &ScriptBuf::new_p2pkh(&PubkeyHash::from_byte_array(id)),
        network,
    )
    .unwrap_or_default()
}

/// D32's refusals: Core's rule by its reason; the rest name the check.
fn standby_refusal(e: Refusal) -> MasternodesError {
    match e {
        Refusal::NotStandby => message_refusal("not_standby"),
        Refusal::Rule(reason) => refused(reason),
        Refusal::NotBuilt => message_refusal("outputs"),
    }
}

fn variant_of(v: Variant) -> StandbyVariant {
    match v {
        Variant::Full => StandbyVariant::Full,
        Variant::Immediate => StandbyVariant::Immediate,
    }
}

/// A stored row's description: no bytes (m-22).
fn standby_of(row: &StandbyRow) -> Standby {
    Standby {
        pro_tx_hash: row.pro_tx_hash.clone(),
        share_index: row.share_index,
        fee: row.fee,
        full_valid_from: row.full_valid_from,
        full: row.full_tx.is_some(),
        immediate: row.immediate_tx.is_some(),
        source: match row.source.as_str() {
            "imported" => StandbySource::Imported,
            _ => StandbySource::Created,
        },
        created_at: row.created_at,
    }
}

/// `SharedMnFormatAmount` in DASH (`qt/mnsharesession.cpp:200-216`):
/// `BitcoinUnits::format` with separators always, trailing zeros and then
/// a bare point dropped, U+202F for the thin space, a no-break space, the
/// unit's name.
pub(super) fn format_amount(duffs: u64, chain: Chain) -> String {
    let amount = i64::try_from(duffs).unwrap_or(i64::MAX);
    let mut number = dw_units::format(Unit::Dash, amount, false, SeparatorStyle::Always, false);
    if number.contains('.') {
        let kept = number.trim_end_matches('0').trim_end_matches('.').len();
        number.truncate(kept);
    }
    format!(
        "{}\u{a0}{}",
        number.replace(dw_units::THIN_SP, "\u{202f}"),
        Unit::Dash.name(chain)
    )
}

/// What Core's standby file shows (`standbyFileText`).
pub(super) struct FileFacts<'a> {
    /// The ProTx hash as displayed.
    pub hash: &'a str,
    pub share_index: u16,
    pub share_count: usize,
    /// UNIX seconds the standby was stored (`Created:`).
    pub created: u64,
    /// UNIX seconds now, from which the full variant's date is estimated.
    pub now: u64,
    /// The tip (`m_current_height`).
    pub tip: u32,
    pub full_valid_from: u32,
    pub penalty: u64,
    pub chain: Chain,
    pub full_hex: Option<String>,
    pub immediate_hex: Option<String>,
}

/// `standbyFileName` (`:1695-1700`): the hash's first 8 characters, the
/// share 1-based.
pub(super) fn file_name(hash: &str, share_index: u16) -> String {
    let short: String = hash.chars().take(8).collect();
    format!("standby-dissolution-{short}-share{}.txt", share_index + 1)
}

/// `standbyFileText` (`:1702-1741`), its lines joined by `\n`, ending with
/// one. The header is English (D22). The two dates are UTC and ISO, where
/// Core writes the local date and the locale's short date-time (D84). A
/// variant the wallet doesn't hold (half of an imported pair) is left out
/// with its label; at penalty 0 the immediate line repeats the one copy, as
/// Core's file does.
pub(super) fn file_text(f: &FileFacts) -> String {
    let blocks_left = f.full_valid_from.saturating_sub(f.tip).saturating_sub(1);
    let created = &iso_local(f.created, 0)[..10];
    let mut lines = vec![
        "Dash shared masternode — standby dissolution".to_string(),
        format!("Masternode: {}", f.hash),
        format!("Share {} of {}", f.share_index + 1, f.share_count),
        format!("Created: {created}"),
        String::new(),
        "Each block below is a complete signed transaction. Broadcast one of them with \
         \"sendrawtransaction\" only when you want to end the masternode and get your principal \
         back. They never expire. Store this file with your refund-address backup, separately \
         from the wallet."
            .to_string(),
        String::new(),
    ];
    if let Some(full) = &f.full_hex {
        lines.push(match blocks_left {
            0 => "FULL PRINCIPAL — valid now, the early period has ended".to_string(),
            n => {
                let when = f.now + u64::from(n) * SECONDS_PER_BLOCK;
                format!(
                    "FULL PRINCIPAL — valid from block {} (~{} UTC)",
                    f.full_valid_from,
                    iso_local(when, 0)[..16].replace('T', " ")
                )
            }
        });
        lines.push(full.clone());
        lines.push(String::new());
    }
    let immediate = match (&f.immediate_hex, f.penalty) {
        (Some(hex), _) => Some(hex),
        (None, 0) => f.full_hex.as_ref(),
        (None, _) => None,
    };
    if let Some(hex) = immediate {
        lines.push(format!(
            "IMMEDIATE — pays the {} early-exit penalty, valid right away",
            format_amount(f.penalty, f.chain)
        ));
        lines.push(hex.clone());
        lines.push(String::new());
    }
    lines.join("\n")
}

impl NetworkSession {
    /// The masternode `hash` as the standby calls read it ([`Registered`]):
    /// `shared_registration_unknown` unless its registration is a shared
    /// ProRegTx in a block of the wallet's history, `shared_too_early`
    /// unless it is listed with the list above that block (m-16).
    pub(super) async fn registered(
        &self,
        wallet: WalletId,
        hash: [u8; 32],
        ctx: &Context,
    ) -> Result<Registered, MasternodesError> {
        let txid = Txid::from_byte_array(hash);
        let chain = self.shared_chain(wallet).await?;
        let entry = chain
            .history
            .get(&txid)
            .ok_or(MasternodesError::SharedRegistrationUnknown)?;
        let height = entry
            .context
            .block_info()
            .map(|b| b.height())
            .ok_or(MasternodesError::SharedRegistrationUnknown)?;
        let (_, p) = decode_shared_protx(&hex::encode(dashcore::consensus::serialize(&entry.tx)))
            .ok_or(MasternodesError::SharedRegistrationUnknown)?;
        let (list_height, listed) = listed_above(ctx, hash, height)?;
        let tip = list_height.max(self.hub.tracker().tip_height().unwrap_or(0));
        Ok(Registered {
            hash,
            collateral: OutPoint::new(txid, p.collateral.vout),
            shares: p.shares,
            registered_height: height,
            early_period_blocks: p.early_period_blocks,
            early_penalty: p.early_penalty,
            voting_key_id: listed.voting_key_id,
            operator_public_key: p.operator_public_key,
            operator_reward: p.operator_reward,
            service: listed
                .service_address
                .map(|a| a.to_string())
                .unwrap_or_default(),
            tip,
            spend_height: tip + 1,
        })
    }

    /// The keychain owner index of each share of `reg` that is this
    /// wallet's (D10).
    async fn my_shares(
        &self,
        wallet: WalletId,
        reg: &Registered,
    ) -> Result<Vec<(u16, u32)>, MasternodesError> {
        let mut mine = Vec::new();
        for (i, share) in reg.shares.iter().enumerate() {
            if let Some(index) = self.keychain_owner(wallet, share.owner_key_id).await? {
                mine.push((i as u16, index));
            }
        }
        Ok(mine)
    }

    async fn standby_rows(
        &self,
        wallet: WalletId,
        hash: Option<[u8; 32]>,
    ) -> Result<Vec<StandbyRow>, MasternodesError> {
        let (w, hash) = (wallet.to_string(), hash.map(|h| display_hex(&h)));
        Ok(self
            .appdb_op(move |db| db.standbys(&w, hash.as_deref()))
            .await?)
    }

    async fn standby_row(
        &self,
        wallet: WalletId,
        hash: [u8; 32],
        share_index: u16,
        fee: u64,
    ) -> Result<StandbyRow, MasternodesError> {
        let (w, h) = (wallet.to_string(), display_hex(&hash));
        self.appdb_op(move |db| db.standby(&w, &h, share_index, fee))
            .await?
            .ok_or_else(|| invalid("no standby of that share and fee is stored"))
    }

    async fn store_standby(
        &self,
        wallet: WalletId,
        row: StandbyRow,
    ) -> Result<bool, MasternodesError> {
        let w = wallet.to_string();
        let stored = self
            .appdb_op(move |db| db.store_standby_durable(&w, &row))
            .await?;
        if stored {
            self.masternodes.local_change();
        }
        Ok(stored)
    }

    /// The wallet's standbys of `hash` (§7 `standby`): descriptions only.
    pub(super) async fn standbys(
        &self,
        wallet: WalletId,
        hash: [u8; 32],
    ) -> Result<Vec<Standby>, MasternodesError> {
        let rows = self.standby_rows(wallet, Some(hash)).await?;
        Ok(rows.iter().map(standby_of).collect())
    }

    /// Both variants for each of this wallet's shares of `hash`, paying
    /// `fee` (§9.4): signed under one `Dissolution` lease charged what the
    /// immediate ones give up (§6.3), checked as an import is (D32), then
    /// stored, synced, before anything is returned. Signing again gives the
    /// same bytes (RFC 6979), so a second call stores nothing new.
    pub(super) async fn create_standbys(
        self: &Arc<Self>,
        wallet: WalletId,
        hash: [u8; 32],
        fee: u64,
        grant: &str,
    ) -> Result<Vec<Standby>, MasternodesError> {
        let ctx = super::v24_context(self, wallet).await?;
        let reg = self.registered(wallet, hash, &ctx).await?;
        let mn = reg.mn();
        let mine = self.my_shares(wallet, &reg).await?;
        if mine.is_empty() {
            return Err(MasternodesError::ShareNotMine);
        }
        let mut pairs = Vec::with_capacity(mine.len());
        for (share, index) in mine {
            let pair = standby::pair(&mn, share, fee).map_err(build_refusal)?;
            // The full variant pays the actor, so it is the larger.
            check_relay_fee(&pair.full, fee)?;
            pairs.push((share, index, pair));
        }
        let debit: u64 = pairs.len() as u64 * (standby::early_penalty(&mn) + fee);
        let registration = self.registration_bytes(wallet, hash).await?;
        let lease = self
            .begin_lease(wallet, FlowKind::Dissolution, &[grant.to_string()])
            .await
            .map_err(PlatformError::from)?;
        let result = async {
            let (charge, signer) = lease
                .charge_tx_signer(BudgetPurpose::Masternode, debit)
                .map_err(|e| lease_failure(e, debit))?;
            let signed = (|| {
                let mut rows = Vec::with_capacity(pairs.len());
                let now = now_secs();
                for (share, index, pair) in &pairs {
                    let sign = |d: &Dissolution, want| {
                        let sig = signer
                            .sign_dissolution(*index, &d.tx.to_bytes(), &registration, 1)
                            .map_err(signer_refusal)?;
                        let signed = d.signed(vec![sig]);
                        let checked = standby::check(&signed, &mn, reg.spend_height)
                            .map_err(standby_refusal)?;
                        if checked.variant != want {
                            return Err(EngineError::Internal(
                                "a standby variant reads as the other".into(),
                            )
                            .into());
                        }
                        Ok::<_, MasternodesError>(signed.to_bytes())
                    };
                    let full = sign(&pair.full, Variant::Full)?;
                    let immediate = pair
                        .immediate
                        .as_ref()
                        .map(|d| sign(d, Variant::Immediate))
                        .transpose()?;
                    rows.push(StandbyRow {
                        pro_tx_hash: display_hex(&hash),
                        share_index: *share,
                        fee,
                        full_tx: Some(full),
                        immediate_tx: immediate,
                        full_valid_from: penalty_free_from(&mn),
                        source: "created".into(),
                        created_at: now,
                    });
                }
                Ok::<_, MasternodesError>(rows)
            })();
            let rows = match signed {
                Ok(rows) => rows,
                Err(e) => {
                    lease.refund(BudgetPurpose::Masternode, charge);
                    return Err(e);
                }
            };
            // Nothing new stored, nothing kept: the charge goes back.
            let mut wrote = false;
            for row in rows {
                match self.store_standby(wallet, row).await {
                    Ok(stored) => wrote |= stored,
                    Err(e) => {
                        if !wrote {
                            lease.refund(BudgetPurpose::Masternode, charge);
                        }
                        return Err(e);
                    }
                }
            }
            Ok(())
        }
        .await;
        lease.end();
        result?;
        self.standbys(wallet, hash).await
    }

    /// Core's file of a stored standby (§9.4, D22) under a `MasternodeOp`
    /// grant charged nothing (D21): the bytes leave the wallet.
    pub(super) async fn export_standby(
        self: &Arc<Self>,
        wallet: WalletId,
        hash: [u8; 32],
        share_index: u16,
        fee: u64,
        grant: &str,
    ) -> Result<StandbyFile, MasternodesError> {
        let ctx = super::v24_context(self, wallet).await?;
        let row = self.standby_row(wallet, hash, share_index, fee).await?;
        let reg = self.registered(wallet, hash, &ctx).await?;
        let lease = self
            .begin_lease(wallet, FlowKind::Dissolution, &[grant.to_string()])
            .await
            .map_err(PlatformError::from)?;
        let gate = lease
            .charge_tx_signer(BudgetPurpose::Masternode, 0)
            .map(|_| ())
            .map_err(|e| lease_failure(e, 0));
        lease.end();
        gate?;
        let chain = match self.network {
            crate::DashNetwork::Mainnet => Chain::Main,
            _ => Chain::Test,
        };
        let hex_of = |b: &Option<Vec<u8>>| b.as_ref().map(hex::encode);
        let shown = display_hex(&hash);
        Ok(StandbyFile {
            file_name: file_name(&shown, share_index),
            text: file_text(&FileFacts {
                hash: &shown,
                share_index,
                share_count: reg.shares.len(),
                created: row.created_at,
                now: now_secs(),
                tip: reg.tip,
                full_valid_from: row.full_valid_from,
                penalty: standby::early_penalty(&reg.mn()),
                chain,
                full_hex: hex_of(&row.full_tx),
                immediate_hex: hex_of(&row.immediate_tx),
            }),
        })
    }

    /// A pasted standby (m-19, D32): checked against the registration in
    /// the wallet's history and the list, stored (`source = imported`) and
    /// described as dash-qt describes one (`masternodelist.cpp:300-373`).
    pub(super) async fn import_standby(
        self: &Arc<Self>,
        wallet: WalletId,
        tx: SpecialTx,
    ) -> Result<SharedImport, MasternodesError> {
        let p: ProDisTx = dashcore::consensus::deserialize(&tx.payload)
            .map_err(|_| standby_refusal(Refusal::NotStandby))?;
        let hash = p.pro_tx_hash.to_byte_array();
        let ctx = super::v24_context(self, wallet).await?;
        let reg = self.registered(wallet, hash, &ctx).await?;
        let mn = reg.mn();
        let checked = standby::check(&tx, &mn, reg.spend_height).map_err(standby_refusal)?;
        let bytes = Some(tx.to_bytes());
        let (full_tx, immediate_tx) = match checked.variant {
            Variant::Full => (bytes, None),
            Variant::Immediate => (None, bytes),
        };
        let row = StandbyRow {
            pro_tx_hash: display_hex(&hash),
            share_index: checked.actor,
            fee: checked.fee,
            full_tx,
            immediate_tx,
            full_valid_from: penalty_free_from(&mn),
            source: "imported".into(),
            created_at: now_secs(),
        };
        let stored = self.store_standby(wallet, row).await?;
        let kept = self
            .standby_row(wallet, hash, checked.actor, checked.fee)
            .await?;
        let mine: HashSet<u16> = self
            .my_shares(wallet, &reg)
            .await?
            .into_iter()
            .map(|(share, _)| share)
            .collect();
        Ok(SharedImport::Standby {
            description: StandbyDescription {
                standby: standby_of(&kept),
                variant: variant_of(checked.variant),
                txid: tx.txid().to_string(),
                penalty: standby::variant_penalty(&mn, checked.variant),
                returns: share_returns(
                    &reg.shares,
                    &tx.output,
                    checked.actor,
                    checked.actor_output,
                    |i| mine.contains(&i),
                    self.network.core_network(),
                ),
                returns_principal: checked.variant == Variant::Full,
            },
            stored,
        })
    }

    /// Sends a stored variant as a `dissolve` operation (§9.4), as
    /// `dissolve` sends (D23, D81, D83), charged what the wallet's share
    /// gives up (§6.3). The full variant is refused `shared_too_early`
    /// before it is valid (the network's verdict would be
    /// `bad-prodis-penalty-floor`, D85); the immediate one
    /// `shared_stage{full_valid}` once the full one is valid and stored
    /// (m-20).
    pub(super) async fn send_standby(
        self: &Arc<Self>,
        wallet: WalletId,
        hash: [u8; 32],
        share_index: u16,
        fee: u64,
        variant: StandbyVariant,
        grant: &str,
    ) -> Result<ProTxOperation, MasternodesError> {
        let ctx = super::v24_context(self, wallet).await?;
        let updating = self.masternodes.update_lock().await;
        let fence = self.coin_fence(wallet).await?;
        self.check_no_pending_maintenance(wallet, hash, None)
            .await?;
        let row = self.standby_row(wallet, hash, share_index, fee).await?;
        let reg = self.registered(wallet, hash, &ctx).await?;
        let mn = reg.mn();
        let full_valid = standby::full_valid(&mn, reg.spend_height);
        let bytes = match variant {
            StandbyVariant::Immediate if full_valid && row.full_tx.is_some() => {
                return Err(MasternodesError::SharedStage {
                    stage: "full_valid".into(),
                });
            }
            StandbyVariant::Full if !full_valid => return Err(MasternodesError::SharedTooEarly),
            StandbyVariant::Full => row.full_tx,
            StandbyVariant::Immediate => row.immediate_tx,
        }
        .ok_or_else(|| invalid("that variant of the standby isn't stored"))?;
        let tx = SpecialTx::from_bytes(&bytes)
            .map_err(|e| EngineError::Internal(format!("stored standby: {e}")))?;
        // The list may have moved on since it was stored.
        let checked = standby::check(&tx, &mn, reg.spend_height).map_err(standby_refusal)?;
        let actor = &reg.shares[usize::from(checked.actor)];
        // D83: a dust actor output is never relayed.
        if actor_warnings(checked.actor_output, &actor.refund_script)
            .contains(&SharedWarning::ActorOutputDust)
        {
            return Err(invalid(DUST_REFUSAL));
        }
        let owner_index = self.keychain_owner(wallet, actor.owner_key_id).await?;
        let record = DissolveRecord {
            request: DissolveRequest {
                pro_tx_hash: display_hex(&hash),
                share_index: checked.actor,
                fee: checked.fee,
            },
            penalty: standby::variant_penalty(&mn, checked.variant),
            actor_output: checked.actor_output,
            foreign: owner_index.is_none(),
        };
        let terms = reg.terms(checked.actor, owner_index, self.network.core_network());
        let lease = self
            .begin_lease(wallet, FlowKind::Dissolution, &[grant.to_string()])
            .await
            .map_err(PlatformError::from)?;
        let id = ops::new_id();
        let result = async {
            let _flight = self.claim_operation(wallet, &id)?;
            let (charge, _) = charge_fee(&lease, record.debit())?;
            let handed = HandOff {
                id: &id,
                signed: tx,
                record,
                terms: &terms,
                hash,
                charge,
                fence,
            };
            self.hand_off_dissolution(wallet, handed, updating).await
        }
        .await;
        lease.end();
        result
    }

    /// D91's prompt: a confirmed registration, in the synced list, with a
    /// share of this wallet that has no standby reads `next:
    /// create_standby`. A delisted masternode (dissolved, say) has nothing
    /// to sign, and without a synced list nothing is prompted.
    pub(super) async fn prompt_standbys(
        &self,
        wallet: WalletId,
        sessions: &mut [SharedSession],
    ) -> Result<(), MasternodesError> {
        let waiting = |s: &SharedSession| {
            matches!(s.stage, SharedStage::Confirmed { .. })
                && s.next == SharedNext::Nothing
                && s.pro_tx_hash.is_some()
        };
        // While sending is off `create_standby` refuses: nothing to prompt
        // (the shipped switch; the test-support override isn't read here).
        if !super::SHARED_SENDING || !sessions.iter().any(waiting) {
            return Ok(());
        }
        let Ok(manager) = self.manager() else {
            return Ok(());
        };
        let list = self.protx_list(&manager).await;
        if !list.synced {
            return Ok(());
        }
        let listed: HashSet<String> = list
            .entries
            .iter()
            .map(|e| display_hex(&e.pro_tx_hash))
            .collect();
        // A prompt is advice: a failed read prompts nothing rather than
        // failing the call it rides on.
        let Ok(rows) = self.standby_rows(wallet, None).await else {
            return Ok(());
        };
        let held: HashSet<(String, u16)> = rows
            .into_iter()
            .map(|r| (r.pro_tx_hash, r.share_index))
            .collect();
        for s in sessions.iter_mut().filter(|s| waiting(s)) {
            let hash = s.pro_tx_hash.clone().unwrap_or_default();
            if listed.contains(&hash)
                && s.rows
                    .iter()
                    .any(|r| r.mine && !held.contains(&(hash.clone(), r.index)))
            {
                s.next = SharedNext::CreateStandby;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[allow(non_snake_case)]
mod tests {
    use super::*;

    const HASH: &str = "4d1c3a3b0f6e0f7f8e1c2b5a6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8f90";

    fn facts(
        tip: u32,
        penalty: u64,
        full: Option<&str>,
        immediate: Option<&str>,
    ) -> FileFacts<'static> {
        FileFacts {
            hash: HASH,
            share_index: 1,
            share_count: 3,
            // 2026-10-30T10:00:00Z, stored the day before.
            created: 1_793_354_400 - 86_400,
            now: 1_793_354_400,
            tip,
            full_valid_from: 150,
            penalty,
            chain: Chain::Test,
            full_hex: full.map(Into::into),
            immediate_hex: immediate.map(Into::into),
        }
    }

    const BODY: &str = "Each block below is a complete signed transaction. Broadcast one of them with \
        \"sendrawtransaction\" only when you want to end the masternode and get your principal back. \
        They never expire. Store this file with your refund-address backup, separately from the wallet.";

    #[test]
    fn test_QT_127_the_standby_file_is_cores() {
        assert_eq!(
            file_name(HASH, 1),
            "standby-dissolution-4d1c3a3b-share2.txt"
        );
        let head = format!(
            "Dash shared masternode — standby dissolution\nMasternode: {HASH}\nShare 2 of 3\n\
             Created: 2026-10-29\n\n{BODY}\n\n"
        );
        // 30 blocks left at tip 119 (valid from 150): 75 minutes on.
        let text = file_text(&facts(119, 30_000_000_000 - 50_000, Some("aa"), Some("bb")));
        assert_eq!(
            text,
            format!(
                "{head}FULL PRINCIPAL — valid from block 150 (~2026-10-30 11:15 UTC)\naa\n\n\
                 IMMEDIATE — pays the 299.9995\u{a0}tDASH early-exit penalty, valid right away\nbb\n"
            )
        );
        // At tip 149 a block mined now is the first valid one.
        let text = file_text(&facts(149, 700_000_000, Some("aa"), Some("bb")));
        assert!(text.contains(
            "\nFULL PRINCIPAL — valid now, the early period has ended\naa\n\n\
             IMMEDIATE — pays the 7\u{a0}tDASH early-exit penalty"
        ));
        // Penalty 0: one copy, printed twice as Core prints the pair.
        let text = file_text(&facts(149, 0, Some("aa"), None));
        assert!(text.ends_with(
            "valid now, the early period has ended\naa\n\n\
             IMMEDIATE — pays the 0\u{a0}tDASH early-exit penalty, valid right away\naa\n"
        ));
        // Half of an imported pair: the other is left out.
        let text = file_text(&facts(149, 5, None, Some("bb")));
        assert!(!text.contains("FULL PRINCIPAL") && text.ends_with("right away\nbb\n"));
        let text = file_text(&facts(149, 5, Some("aa"), None));
        assert!(!text.contains("IMMEDIATE") && text.ends_with("ended\naa\n"));
    }

    #[test]
    fn test_QT_127_amounts_are_shared_mn_format_amount() {
        assert_eq!(
            format_amount(100_000_000_000, Chain::Main),
            "1\u{202f}000\u{a0}DASH"
        );
        assert_eq!(format_amount(150_000, Chain::Test), "0.0015\u{a0}tDASH");
        assert_eq!(format_amount(0, Chain::Main), "0\u{a0}DASH");
        assert_eq!(
            format_amount(123_456_789, Chain::Main),
            "1.23456789\u{a0}DASH"
        );
    }
}
