//! A wallet's masternodes from its provider transactions (MG-03 §3, D1):
//! platform-wallet's `aggregate_masternodes` (Core's `block.vtx` order,
//! per-type numbering), plus what it leaves out: the collateral outpoint
//! resolved per Core with the internal output checked (MG-00 D43, MG-01
//! §7), and the operator's share. Pure; not facade.

use dashcore::hashes::Hash;
use dashcore::transaction::TransactionPayload;
use dashcore::{OutPoint, Transaction, Txid};
use dw_protx::params::{EVONODE_COLLATERAL, MASTERNODE_COLLATERAL};
use dw_protx::payloads::resolve_collateral;
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

/// The collateral amount of a masternode type (Core `dmn_types`).
pub(crate) fn collateral_amount(evonode: bool) -> u64 {
    if evonode {
        EVONODE_COLLATERAL
    } else {
        MASTERNODE_COLLATERAL
    }
}

/// Aggregates `(height, in-block position, tx)` provider transactions.
pub(crate) fn aggregate(txs: &[(u32, u32, Transaction)]) -> Vec<OwnedMasternode> {
    let records = aggregate_masternodes(txs.iter().map(|(h, p, tx)| (*h, *p, tx)), |_| {
        ListMembership::ListUnavailable
    });
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
            let collateral = registration.and_then(|(tx, p)| {
                let outpoint = resolve_collateral(tx.txid(), p.collateral_outpoint);
                if p.collateral_outpoint.txid != Txid::all_zeros() {
                    return Some(outpoint);
                }
                // Internal collateral: Core checks the ProRegTx's own output
                // (`specialtxman.cpp`); a held ProRegTx that fails it can't
                // be the one Core accepted.
                let out = tx.output.get(outpoint.vout as usize)?;
                (out.value == collateral_amount(record.is_evonode)).then_some(outpoint)
            });
            OwnedMasternode {
                operator_reward: registration.map(|(_, p)| p.operator_reward),
                collateral,
                record,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use dashcore::blockdata::transaction::special_transaction::provider_registration::{
        ProviderMasternodeType, ProviderRegistrationPayload,
    };
    use dashcore::blockdata::transaction::special_transaction::provider_update_registrar::ProviderUpdateRegistrarPayload;
    use dashcore::bls_sig_utils::BLSPublicKey;
    use dashcore::hash_types::{InputsHash, PubkeyHash};
    use dashcore::{ScriptBuf, TxIn, TxOut};

    fn payout(seed: u8) -> ScriptBuf {
        ScriptBuf::new_p2pkh(&PubkeyHash::from_byte_array([seed; 20]))
    }

    fn registration(
        collateral: OutPoint,
        evonode: bool,
        reward: u16,
        outputs: Vec<TxOut>,
    ) -> Transaction {
        let payload = ProviderRegistrationPayload {
            version: 2,
            masternode_type: if evonode {
                ProviderMasternodeType::HighPerformance
            } else {
                ProviderMasternodeType::Regular
            },
            masternode_mode: 0,
            collateral_outpoint: collateral,
            service_address: "127.0.0.1:19899".parse().unwrap(),
            owner_key_hash: PubkeyHash::from_byte_array([1; 20]),
            operator_public_key: BLSPublicKey::from([2; 48]),
            voting_key_hash: PubkeyHash::from_byte_array([3; 20]),
            operator_reward: reward,
            script_payout: payout(4),
            inputs_hash: InputsHash::all_zeros(),
            signature: Vec::new(),
            platform_node_id: None,
            platform_p2p_port: None,
            platform_http_port: None,
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

    fn update(pro_tx_hash: Txid, payout_seed: u8) -> Transaction {
        let payload = ProviderUpdateRegistrarPayload {
            version: 2,
            pro_tx_hash,
            provider_mode: 0,
            operator_public_key: BLSPublicKey::from([2; 48]),
            voting_key_hash: PubkeyHash::from_byte_array([5; 20]),
            script_payout: payout(payout_seed),
            inputs_hash: InputsHash::all_zeros(),
            payload_sig: Vec::new(),
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

    fn null(vout: u32) -> OutPoint {
        OutPoint {
            txid: Txid::all_zeros(),
            vout,
        }
    }

    fn coins(amounts: &[u64]) -> Vec<TxOut> {
        amounts
            .iter()
            .map(|v| TxOut {
                value: *v,
                script_pubkey: payout(9),
            })
            .collect()
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
}
