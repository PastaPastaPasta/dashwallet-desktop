//! A wallet's masternodes from its provider transactions (MG-03 §3, D1):
//! platform-wallet's `aggregate_masternodes` (Core's `block.vtx` order,
//! per-type numbering), plus what it leaves out: the collateral outpoint
//! resolved per Core with the internal output checked (MG-00 D43, MG-01
//! §7), and the operator's share. Pure; not facade.

use std::collections::HashSet;

use dashcore::hashes::Hash;
use dashcore::transaction::TransactionPayload;
use dashcore::{OutPoint, Transaction, Txid};
use dw_protx::params::{EVONODE_COLLATERAL, MASTERNODE_COLLATERAL};
use dw_protx::payloads::checked_collateral;
use platform_wallet::masternode::{ListMembership, MasternodeRecord, aggregate_masternodes};

/// One of the wallet's masternodes.
#[derive(Debug, Clone)]
pub(crate) struct OwnedMasternode {
    /// The library's record: keys, payout, service, revocation, numbering.
    /// Its `status` and `collateral` are not used.
    pub record: MasternodeRecord,
    /// The collateral per Core, `None` when the ProRegTx isn't held or its
    /// internal output didn't check out.
    pub collateral: Option<OutPoint>,
    /// ProRegTx `operatorReward`, basis points.
    pub operator_reward: Option<u16>,
    /// A version-3 ProRegTx or ProUpRegTx is among its transactions
    /// (MG-03 D25): its payouts and shares aren't read yet (MG-03b), so the
    /// record's keys, payout and service aren't reported.
    pub shared_unsupported: bool,
}

impl OwnedMasternode {
    /// The ProRegTx is held but not in a block.
    pub fn unconfirmed(&self) -> bool {
        self.record.has_registration && self.record.registration_height == 0
    }

    /// The ProRegTx's block height.
    pub fn registered_height(&self) -> Option<u32> {
        (self.record.has_registration && self.record.registration_height > 0)
            .then_some(self.record.registration_height)
    }
}

/// The first ProRegTx/ProUpRegTx version with Core v24's `payouts` and
/// `shares` in place of `scriptPayout` (`ProTxVersion::ExtAddr`,
/// `evo/providertx.h`). The pinned library decodes version 3 (BL-187), but
/// the wallet's masternode rows read only version 2's fields: a version-3
/// payload stays unsupported here until MG-03b reads the rest.
const SHARED_PAYLOAD_VERSION: u16 = 3;

/// The ProTx hash (wire order) of a ProRegTx or ProUpRegTx of version 3 or
/// later.
fn shared_payload(tx: &Transaction) -> Option<[u8; 32]> {
    use TransactionPayload::*;
    match &tx.special_transaction_payload {
        Some(ProviderRegistrationPayloadType(p)) if p.version >= SHARED_PAYLOAD_VERSION => {
            Some(tx.txid().to_byte_array())
        }
        Some(ProviderUpdateRegistrarPayloadType(p)) if p.version >= SHARED_PAYLOAD_VERSION => {
            Some(p.pro_tx_hash.to_byte_array())
        }
        _ => None,
    }
}

/// A ProRegTx, ProUpServTx, ProUpRegTx or ProUpRevTx.
fn is_provider(tx: &Transaction) -> bool {
    use TransactionPayload::*;
    matches!(
        tx.special_transaction_payload,
        Some(
            ProviderRegistrationPayloadType(_)
                | ProviderUpdateServicePayloadType(_)
                | ProviderUpdateRegistrarPayloadType(_)
                | ProviderUpdateRevocationPayloadType(_)
        )
    )
}

/// Whether an amount is an evonode's collateral, a regular masternode's,
/// or neither (`None`).
pub(crate) fn collateral_kind(amount: u64) -> Option<bool> {
    match amount {
        MASTERNODE_COLLATERAL => Some(false),
        EVONODE_COLLATERAL => Some(true),
        _ => None,
    }
}

/// Adds the provider transactions among `history` that `txs` lacks, then
/// orders `txs` by txid, as the library hands them over.
pub(crate) fn add_history<'a>(
    txs: &mut Vec<(u32, u32, Transaction)>,
    history: impl IntoIterator<Item = (u32, u32, &'a Transaction)>,
) {
    let held: HashSet<Txid> = txs.iter().map(|(_, _, tx)| tx.txid()).collect();
    txs.extend(
        history
            .into_iter()
            .filter(|(_, _, tx)| is_provider(tx) && !held.contains(&tx.txid()))
            .map(|(h, p, tx)| (h, p, tx.clone())),
    );
    txs.sort_by_cached_key(|(_, _, tx)| tx.txid());
}

/// Aggregates `(height, in-block position, tx)` provider transactions.
pub(crate) fn aggregate(txs: &[(u32, u32, Transaction)]) -> Vec<OwnedMasternode> {
    let records = aggregate_masternodes(txs.iter().map(|(h, p, tx)| (*h, *p, tx)), |_| {
        ListMembership::ListUnavailable
    });
    let shared: HashSet<[u8; 32]> = txs
        .iter()
        .filter_map(|(_, _, tx)| shared_payload(tx))
        .collect();
    records
        .into_iter()
        .map(|record| {
            let registration =
                txs.iter()
                    .find_map(|(_, _, tx)| match &tx.special_transaction_payload {
                        Some(TransactionPayload::ProviderRegistrationPayloadType(p))
                            if tx.txid().to_byte_array() == record.pro_tx_hash =>
                        {
                            Some((tx, p))
                        }
                        _ => None,
                    });
            let collateral =
                registration.and_then(|(tx, p)| checked_collateral(tx, p, record.is_evonode));
            let shared_unsupported = shared.contains(&record.pro_tx_hash);
            OwnedMasternode {
                // a version-3 row reads none of its later fields (MG-03b)
                operator_reward: registration
                    .filter(|_| !shared_unsupported)
                    .map(|(_, p)| p.operator_reward),
                collateral,
                shared_unsupported,
                record,
            }
        })
        .collect()
}

#[cfg(test)]
pub(crate) mod fixtures {
    use super::*;
    use dashcore::blockdata::transaction::special_transaction::provider_registration::{
        MasternodePayoutShare, ProviderMasternodeType, ProviderRegistrationPayload,
        SharedCollateral,
    };
    use dashcore::blockdata::transaction::special_transaction::provider_update_registrar::ProviderUpdateRegistrarPayload;
    use dashcore::bls_sig_utils::BLSPublicKey;
    use dashcore::hash_types::{InputsHash, PubkeyHash};
    use dashcore::sml::masternode_list_entry::MasternodeNetInfo;
    use dashcore::sml::masternode_list_entry::net_info::{
        Bip155Network, ExtNetInfo, NetInfoEntry, NetInfoPurpose,
    };
    use dashcore::{ScriptBuf, TxIn, TxOut};

    pub(crate) fn payout(seed: u8) -> ScriptBuf {
        ScriptBuf::new_p2pkh(&PubkeyHash::from_byte_array([seed; 20]))
    }

    /// A version-3 payload's service is an `ExtNetInfo`; before, an address.
    fn service(version: u16) -> MasternodeNetInfo {
        if version < SHARED_PAYLOAD_VERSION {
            return MasternodeNetInfo::Legacy("127.0.0.1:19899".parse().unwrap());
        }
        MasternodeNetInfo::Extended(ExtNetInfo {
            version: 1,
            purposes: vec![(
                NetInfoPurpose::CoreP2P,
                vec![NetInfoEntry::Service {
                    network: Bip155Network::Ipv4,
                    addr: vec![127, 0, 0, 1],
                    port: 19899,
                }],
            )],
        })
    }

    /// A payload's `scriptPayout`: version 3 has `payouts` instead.
    fn script_payout(version: u16, seed: u8) -> ScriptBuf {
        if version >= SHARED_PAYLOAD_VERSION {
            ScriptBuf::new()
        } else {
            payout(seed)
        }
    }

    /// A version-3 payload's one payout, all of the reward to `payout(seed)`.
    fn payouts(version: u16, seed: u8) -> Option<Vec<MasternodePayoutShare>> {
        (version >= SHARED_PAYLOAD_VERSION).then(|| {
            vec![MasternodePayoutShare {
                script_payout: payout(seed),
                reward: 10_000,
            }]
        })
    }

    pub(crate) fn registration(
        collateral: OutPoint,
        evonode: bool,
        reward: u16,
        outputs: Vec<TxOut>,
    ) -> Transaction {
        registration_v(2, collateral, evonode, reward, outputs)
    }

    pub(crate) fn registration_v(
        version: u16,
        collateral: OutPoint,
        evonode: bool,
        reward: u16,
        outputs: Vec<TxOut>,
    ) -> Transaction {
        let payload = ProviderRegistrationPayload {
            version,
            masternode_type: if evonode {
                ProviderMasternodeType::HighPerformance
            } else {
                ProviderMasternodeType::Regular
            },
            masternode_mode: 0,
            collateral_outpoint: collateral,
            service_address: service(version),
            owner_key_hash: PubkeyHash::from_byte_array([1; 20]),
            operator_public_key: BLSPublicKey::from([2; 48]),
            voting_key_hash: PubkeyHash::from_byte_array([3; 20]),
            operator_reward: reward,
            script_payout: script_payout(version, 4),
            inputs_hash: InputsHash::all_zeros(),
            signature: Vec::new(),
            platform_node_id: None,
            platform_p2p_port: None,
            platform_http_port: None,
            payouts: payouts(version, 4),
            shared_collateral: (version >= SHARED_PAYLOAD_VERSION).then(|| SharedCollateral {
                shares: Vec::new(),
                join_sigs: Vec::new(),
                early_period_blocks: 0,
                early_penalty: 0,
            }),
        };
        Transaction {
            version: 3,
            lock_time: 0,
            input: vec![TxIn::default()],
            output: outputs,
            special_transaction_payload: Some(TransactionPayload::ProviderRegistrationPayloadType(
                payload,
            )),
        }
    }

    pub(crate) fn update(pro_tx_hash: Txid, payout_seed: u8) -> Transaction {
        update_v(2, pro_tx_hash, payout_seed)
    }

    pub(crate) fn update_v(version: u16, pro_tx_hash: Txid, payout_seed: u8) -> Transaction {
        let payload = ProviderUpdateRegistrarPayload {
            version,
            pro_tx_hash,
            provider_mode: 0,
            operator_public_key: BLSPublicKey::from([2; 48]),
            voting_key_hash: PubkeyHash::from_byte_array([5; 20]),
            script_payout: script_payout(version, payout_seed),
            inputs_hash: InputsHash::all_zeros(),
            payload_sig: Vec::new(),
            payouts: payouts(version, payout_seed),
        };
        Transaction {
            version: 3,
            lock_time: 1,
            input: vec![TxIn::default()],
            output: Vec::new(),
            special_transaction_payload: Some(
                TransactionPayload::ProviderUpdateRegistrarPayloadType(payload),
            ),
        }
    }

    pub(crate) fn null(vout: u32) -> OutPoint {
        OutPoint {
            txid: Txid::all_zeros(),
            vout,
        }
    }

    pub(crate) fn coins(amounts: &[u64]) -> Vec<TxOut> {
        amounts
            .iter()
            .map(|v| TxOut {
                value: *v,
                script_pubkey: payout(9),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::*;
    use super::*;
    use dashcore::TxIn;

    #[test]
    fn history_adds_the_provider_transactions_the_library_lacks() {
        let held = registration(null(1), false, 0, coins(&[5, MASTERNODE_COLLATERAL]));
        let persisted = registration(null(0), false, 0, coins(&[MASTERNODE_COLLATERAL]));
        let plain = Transaction {
            special_transaction_payload: None,
            ..persisted.clone()
        };
        // a special transaction of another type
        let other = Transaction {
            special_transaction_payload: Some(
                TransactionPayload::ClassicalWithNonStandardVersionTypeBytesPayloadType(8),
            ),
            ..plain.clone()
        };
        let mut txs = vec![(100, 1, held.clone())];
        // the held one again (another context), the persisted one, a payment
        // and the other special transaction
        add_history(
            &mut txs,
            [
                (0, 0, &held),
                (120, 2, &persisted),
                (130, 0, &plain),
                (131, 0, &other),
            ],
        );
        let mut want = vec![(100, 1, held.txid()), (120, 2, persisted.txid())];
        want.sort_by_key(|(_, _, t)| *t);
        let got: Vec<_> = txs.iter().map(|(h, p, tx)| (*h, *p, tx.txid())).collect();
        assert_eq!(got, want);
        assert_eq!(aggregate(&txs).len(), 2);
    }

    #[test]
    fn internal_collateral_is_the_registrations_own_output() {
        let tx = registration(null(1), false, 0, coins(&[5, MASTERNODE_COLLATERAL]));
        let out = aggregate(&[(100, 1, tx.clone())]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].record.pro_tx_hash, tx.txid().to_byte_array());
        assert_eq!(
            out[0].collateral,
            Some(OutPoint {
                txid: tx.txid(),
                vout: 1
            })
        );
        assert_eq!(out[0].registered_height(), Some(100));
        assert!(!out[0].unconfirmed());
    }

    #[test]
    fn internal_collateral_of_the_wrong_amount_or_index_is_unresolved() {
        // A regular masternode's amount on an evonode, and an index past
        // the outputs.
        let evo = registration(null(0), true, 0, coins(&[MASTERNODE_COLLATERAL]));
        let missing = registration(null(3), false, 0, coins(&[MASTERNODE_COLLATERAL]));
        let out = aggregate(&[(10, 0, evo), (11, 0, missing)]);
        assert_eq!(out.len(), 2);
        assert!(out.iter().all(|m| m.collateral.is_none()));
        let evo = registration(null(0), true, 0, coins(&[EVONODE_COLLATERAL]));
        assert!(aggregate(&[(10, 0, evo)])[0].collateral.is_some());
    }

    #[test]
    fn external_collateral_is_the_payload_outpoint() {
        let external = OutPoint {
            txid: Txid::from_byte_array([7; 32]),
            vout: 2,
        };
        let tx = registration(external, false, 250, coins(&[5]));
        let out = aggregate(&[(0, 0, tx)]);
        assert_eq!(out[0].collateral, Some(external));
        assert_eq!(out[0].operator_reward, Some(250));
        assert!(out[0].unconfirmed());
        assert_eq!(out[0].registered_height(), None);
    }

    #[test]
    fn a_later_registrar_update_moves_the_payout_and_voting_key() {
        let reg = registration(null(0), false, 0, coins(&[MASTERNODE_COLLATERAL]));
        let upd = update(reg.txid(), 8);
        // Fed out of order: height decides.
        let out = aggregate(&[(20, 0, upd), (10, 0, reg)]);
        assert_eq!(out.len(), 1);
        assert_eq!(
            out[0].record.payout_script.as_deref(),
            Some(payout(8).as_bytes())
        );
        assert_eq!(out[0].record.voting_key_hash, Some([5; 20]));
        assert_eq!(out[0].record.tx_count, 2);
    }

    #[test]
    fn an_update_without_its_registration_has_no_collateral() {
        let reg = registration(null(0), false, 0, coins(&[MASTERNODE_COLLATERAL]));
        let out = aggregate(&[(20, 0, update(reg.txid(), 8))]);
        assert_eq!(out.len(), 1);
        assert!(!out[0].record.has_registration);
        assert_eq!(out[0].collateral, None);
        assert_eq!(out[0].operator_reward, None);
    }

    #[test]
    fn a_version_3_registration_or_update_marks_the_masternode_shared() {
        let reg = registration(null(0), false, 250, coins(&[MASTERNODE_COLLATERAL]));
        let plain = aggregate(&[(10, 0, reg.clone())]);
        assert!(!plain[0].shared_unsupported);
        assert_eq!(plain[0].operator_reward, Some(250));

        let shared = registration_v(3, null(0), false, 250, coins(&[MASTERNODE_COLLATERAL]));
        let out = aggregate(&[(10, 0, shared.clone())]);
        assert!(out[0].shared_unsupported);
        assert_eq!(out[0].operator_reward, None, "not read until MG-03b");
        // the fields before the service still resolve the collateral
        assert_eq!(
            out[0].collateral,
            Some(OutPoint {
                txid: shared.txid(),
                vout: 0
            })
        );

        // a version-3 ProUpRegTx of a version-2 registration
        let out = aggregate(&[(10, 0, reg.clone()), (20, 0, update_v(3, reg.txid(), 8))]);
        assert_eq!(out.len(), 1);
        assert!(out[0].shared_unsupported);
        // a later version-2 update doesn't clear it
        let out = aggregate(&[
            (10, 0, reg.clone()),
            (20, 0, update_v(3, reg.txid(), 8)),
            (30, 0, update(reg.txid(), 9)),
        ]);
        assert!(out[0].shared_unsupported);
    }

    /// Core v24 bytes of a version-3 ProRegTx (`CProRegTx`'s
    /// `SERIALIZE_METHODS`: an `ExtNetInfo` service, then `payouts` and
    /// `shares` where version 2 has `scriptPayout`), which the pinned
    /// library decodes since BL-187: the masternode is then shared.
    #[test]
    fn the_pinned_library_decodes_core_v24_version_3_registration_bytes() {
        use dashcore::consensus::{Encodable, deserialize};
        let mut payload = Vec::new();
        3u16.consensus_encode(&mut payload).unwrap(); // nVersion = ExtAddr
        0u16.consensus_encode(&mut payload).unwrap(); // nType
        0u16.consensus_encode(&mut payload).unwrap(); // nMode
        null(0).consensus_encode(&mut payload).unwrap(); // collateral
        // ExtNetInfo version 1: one purpose (CORE_P2P), one entry, a
        // service in ADDRv2 (IPv4, 4 bytes, big-endian port)
        payload.extend([1, 1, 0, 1]);
        payload.extend([1, 1, 4, 127, 0, 0, 1, 0x4d, 0xbb]);
        payload.extend([1; 20]); // keyIDOwner
        payload.extend([2; 48]); // pubKeyOperator
        payload.extend([3; 20]); // keyIDVoting
        0u16.consensus_encode(&mut payload).unwrap(); // nOperatorReward
        payload.push(2); // payouts: two shares
        for (seed, reward) in [(4u8, 6000u16), (5, 4000)] {
            payout(seed).consensus_encode(&mut payload).unwrap();
            reward.consensus_encode(&mut payload).unwrap();
        }
        payload.push(0); // shares: none
        0u32.consensus_encode(&mut payload).unwrap(); // nEarlyPeriodBlocks
        0i64.consensus_encode(&mut payload).unwrap(); // nEarlyPenalty
        payload.extend([0; 32]); // inputsHash
        payload.extend([65]);
        payload.extend([0; 65]); // vchSig

        let mut tx = Vec::new();
        3u16.consensus_encode(&mut tx).unwrap(); // version
        1u16.consensus_encode(&mut tx).unwrap(); // TRANSACTION_PROVIDER_REGISTER
        vec![TxIn::default()].consensus_encode(&mut tx).unwrap();
        coins(&[MASTERNODE_COLLATERAL])
            .consensus_encode(&mut tx)
            .unwrap();
        0u32.consensus_encode(&mut tx).unwrap(); // lock time
        payload.consensus_encode(&mut tx).unwrap(); // vExtraPayload
        let tx: Transaction = deserialize(&tx).unwrap();
        let Some(TransactionPayload::ProviderRegistrationPayloadType(p)) =
            &tx.special_transaction_payload
        else {
            panic!("not a ProRegTx: {tx:?}");
        };
        assert_eq!(p.version, 3);
        let rewards: Vec<_> = p
            .payouts
            .iter()
            .flatten()
            .map(|s| (&s.script_payout, s.reward))
            .collect();
        assert_eq!(rewards, [(&payout(4), 6000), (&payout(5), 4000)]);
        assert!(aggregate(&[(10, 0, tx)])[0].shared_unsupported);
    }
}
