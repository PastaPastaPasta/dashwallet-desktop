//! Pending spend authorization comes from the pinned key-wallet verifier.

pub(crate) use dashcore::blockdata::constants::MAX_SCRIPT_SIZE;
pub(crate) use key_wallet::transaction_checking::verify_input_authorization;

#[cfg(test)]
pub(crate) use key_wallet::transaction_checking::SpendAuthorizationError;

#[cfg(test)]
use dashcore::sighash::{EcdsaSighashType, SighashCache};
#[cfg(test)]
use dashcore::{PublicKey, Transaction, TxOut};

/// `tx` with every input signed as its coin's owner signs it, where the coin
/// pays an address of `info`'s funds accounts. A coin is looked up in
/// `fundings`, then among the coins `info` holds; other inputs are left as
/// they are.
#[cfg(test)]
pub(crate) async fn sign_owned_inputs(
    wallet: &key_wallet::Wallet,
    info: &key_wallet::wallet::ManagedWalletInfo,
    mut tx: Transaction,
    fundings: &[&Transaction],
) -> Transaction {
    use dashcore::Address;
    use dashcore::blockdata::script::Builder;
    use key_wallet::managed_account::managed_account_trait::ManagedAccountTrait;
    use key_wallet::wallet::managed_wallet_info::transaction_builder::TransactionSigner;

    let funds: Vec<_> = (info.accounts.all_accounts().into_iter())
        .filter_map(|a| a.as_funds())
        .collect();
    let prevout = |input: &dashcore::TxIn| -> Option<TxOut> {
        let outpoint = input.previous_output;
        fundings
            .iter()
            .find(|funding| funding.txid() == outpoint.txid)
            .and_then(|funding| funding.output.get(outpoint.vout as usize).cloned())
            .or_else(|| {
                funds
                    .iter()
                    .find_map(|a| a.utxos.get(&outpoint))
                    .map(|u| u.txout.clone())
            })
    };
    let mut tasks = Vec::new();
    {
        let cache = SighashCache::new(&tx);
        for (index, input) in tx.input.iter().enumerate() {
            let Some(prevout) = prevout(input) else {
                continue;
            };
            let Ok(address) = Address::from_script(&prevout.script_pubkey, wallet.network) else {
                continue;
            };
            let Some(path) = funds
                .iter()
                .find_map(|a| a.address_derivation_path(&address))
            else {
                continue;
            };
            let sighash = cache
                .legacy_signature_hash(
                    index,
                    &prevout.script_pubkey,
                    EcdsaSighashType::All.to_u32(),
                )
                .expect("sighash");
            tasks.push((index, sighash, path));
        }
    }
    for (index, sighash, path) in tasks {
        let (signature, public_key) = wallet.sig_and_pubkey(sighash, path).await.expect("signs");
        tx.input[index].script_sig = Builder::new()
            .push_slice(dashcore::ecdsa::Signature::sighash_all(signature.into()).serialize())
            .push_key(&PublicKey::new(public_key))
            .into_script();
    }
    tx
}

#[cfg(test)]
mod tests {
    use super::*;
    use dashcore::blockdata::transaction::special_transaction::{
        TransactionPayload, asset_lock::AssetLockPayload,
    };
    use dashcore::ecdsa::{EcdsaSecretKey, EcdsaSignature};
    use dashcore::hashes::hash160;
    use dashcore::opcodes::all::{OP_DROP, OP_SWAP};
    use dashcore::script::{Builder, PushBytesBuf};
    use dashcore::{OutPoint, PubkeyHash, ScriptBuf, TxIn, Txid};
    use key_wallet::managed_account::managed_account_trait::ManagedAccountTrait;
    use key_wallet::test_utils::TestWalletContext;
    use key_wallet::wallet::managed_wallet_info::transaction_builder::TransactionSigner;
    use std::slice;

    /// The order of the secp256k1 group, big-endian.
    const CURVE_ORDER: [u8; 32] = [
        0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
        0xfe, 0xba, 0xae, 0xdc, 0xe6, 0xaf, 0x48, 0xa0, 0x3b, 0xbf, 0xd2, 0x5e, 0x8c, 0xd0, 0x36,
        0x41, 0x41,
    ];

    fn secret(byte: u8) -> EcdsaSecretKey {
        EcdsaSecretKey::from_bytes(&[byte; 32]).unwrap()
    }

    fn compressed(key: &EcdsaSecretKey) -> Vec<u8> {
        key.public_key().to_compressed().to_vec()
    }

    fn p2pkh_prevout(public_key: &[u8]) -> TxOut {
        TxOut {
            value: 100_000,
            script_pubkey: ScriptBuf::new_p2pkh(&PubkeyHash::from_byte_array(
                hash160::Hash::hash(public_key).to_byte_array(),
            )),
        }
    }

    /// An unsigned spend of one input per funding output, paying back to the first owner.
    fn spend_of(prevouts: &[TxOut]) -> Transaction {
        Transaction {
            version: 1,
            lock_time: 0,
            input: (0..prevouts.len() as u32)
                .map(|vout| TxIn {
                    previous_output: OutPoint {
                        txid: Txid::from_byte_array([3; 32]),
                        vout,
                    },
                    ..Default::default()
                })
                .collect(),
            output: vec![TxOut {
                value: prevouts.iter().map(|prevout| prevout.value).sum::<u64>() - 10_000,
                script_pubkey: prevouts[0].script_pubkey.clone(),
            }],
            special_transaction_payload: None,
        }
    }

    fn signed(uncompressed: bool) -> (Transaction, TxOut) {
        let key = secret(7);
        let public_key = if uncompressed {
            key.public_key().to_uncompressed().to_vec()
        } else {
            compressed(&key)
        };
        let prevout = p2pkh_prevout(&public_key);
        let mut tx = spend_of(slice::from_ref(&prevout));
        sign_input(&mut tx, 0, &prevout, &key, &public_key, 1);
        (tx, prevout)
    }

    fn sign_input(
        tx: &mut Transaction,
        input_index: usize,
        prevout: &TxOut,
        key: &EcdsaSecretKey,
        public_key: &[u8],
        mode: u8,
    ) {
        let hash = SighashCache::new(&*tx)
            .legacy_signature_hash(input_index, &prevout.script_pubkey, u32::from(mode))
            .unwrap();
        let mut signature = key.sign(&hash.to_byte_array()).with_der(<[u8]>::to_vec);
        signature.push(mode);
        tx.input[input_index].script_sig = Builder::new()
            .push_slice(PushBytesBuf::try_from(signature).unwrap())
            .push_slice(PushBytesBuf::try_from(public_key.to_vec()).unwrap())
            .into_script();
    }

    /// Two inputs funded by different owners, each signed by its own.
    fn signed_by_two_owners() -> (Transaction, Vec<TxOut>) {
        let keys = [secret(7), secret(9)];
        let public_keys = keys.iter().map(compressed).collect::<Vec<_>>();
        let prevouts = public_keys
            .iter()
            .map(|public_key| p2pkh_prevout(public_key))
            .collect::<Vec<_>>();
        let mut tx = spend_of(&prevouts);
        for index in 0..2 {
            sign_input(
                &mut tx,
                index,
                &prevouts[index],
                &keys[index],
                &public_keys[index],
                1,
            );
        }
        (tx, prevouts)
    }

    #[test]
    fn should_authorize_each_input_only_against_its_own_funding_output() {
        let (tx, prevouts) = signed_by_two_owners();
        for index in 0..2 {
            assert_eq!(
                verify_input_authorization(&tx, index, &prevouts[index]),
                Ok(true)
            );
            assert_eq!(
                verify_input_authorization(&tx, index, &prevouts[1 - index]),
                Ok(false)
            );
        }

        // SIGHASH_ALL commits to every input, so changing one input revokes the other's.
        let mut resequenced = tx.clone();
        resequenced.input[1].sequence = 7;
        assert_eq!(
            verify_input_authorization(&resequenced, 0, &prevouts[0]),
            Ok(false)
        );

        // A signature does not follow its input to another position.
        let mut reordered = tx;
        reordered.input.swap(0, 1);
        for index in 0..2 {
            assert_eq!(
                verify_input_authorization(&reordered, index, &prevouts[1 - index]),
                Ok(false)
            );
        }
    }

    #[test]
    fn should_accept_compressed_and_uncompressed_owner_signatures() {
        for uncompressed in [false, true] {
            let (tx, prevout) = signed(uncompressed);
            assert_eq!(verify_input_authorization(&tx, 0, &prevout), Ok(true));
        }
    }

    #[test]
    fn should_accept_a_high_s_owner_signature() {
        let (mut tx, prevout) = signed(false);
        let script = tx.input[0].script_sig.as_bytes().to_vec();
        let signature_len = usize::from(script[0]);
        let signature = EcdsaSignature::from_der(&script[1..signature_len]).unwrap();
        let low = signature.to_bytes();
        assert!(signature.is_low_s(), "the signer produces low S");
        let order = CURVE_ORDER;
        let mut high = low;
        let mut borrow = 0i16;
        for i in (0..32).rev() {
            let value = i16::from(order[i]) - i16::from(low[32 + i]) - borrow;
            high[32 + i] = value.rem_euclid(256) as u8;
            borrow = i16::from(value < 0);
        }
        let high = EcdsaSignature::from_bytes(&high).unwrap();
        assert!(!high.is_low_s(), "the test signature has high S");
        let mut der = high.with_der(<[u8]>::to_vec);
        der.push(1);
        tx.input[0].script_sig = Builder::new()
            .push_slice(PushBytesBuf::try_from(der).unwrap())
            .push_slice(PushBytesBuf::try_from(script[signature_len + 2..].to_vec()).unwrap())
            .into_script();
        assert_eq!(verify_input_authorization(&tx, 0, &prevout), Ok(true));
    }

    #[tokio::test]
    async fn should_accept_what_the_wallet_signer_produces() {
        let (ctx, _) = TestWalletContext::new_random()
            .with_mempool_funding(100_000)
            .await;
        let utxo = ctx.first_utxo().clone();
        let path = ctx
            .bip44_account()
            .address_derivation_path(&utxo.address)
            .unwrap();
        let mut unsigned = Transaction::dummy_empty();
        unsigned.input.push(TxIn {
            previous_output: utxo.outpoint,
            ..Default::default()
        });
        unsigned.output.push(TxOut {
            value: 90_000,
            script_pubkey: utxo.txout.script_pubkey.clone(),
        });
        let signed = ctx
            .wallet
            .sign_tx(unsigned, vec![utxo.clone()], move |_| Some(path.clone()))
            .await
            .unwrap();
        assert_eq!(
            verify_input_authorization(&signed, 0, &utxo.txout),
            Ok(true)
        );
    }

    #[test]
    fn should_reject_unsigned_tampered_and_wrong_funding_claims() {
        let (tx, prevout) = signed(false);
        let mut unsigned = tx.clone();
        unsigned.input[0].script_sig = ScriptBuf::new();
        assert_eq!(
            verify_input_authorization(&unsigned, 0, &prevout),
            Ok(false)
        );
        let mut changed = tx.clone();
        changed.output[0].value -= 1;
        assert_eq!(verify_input_authorization(&changed, 0, &prevout), Ok(false));
        changed = tx.clone();
        changed.input[0].previous_output.vout += 1;
        assert_eq!(verify_input_authorization(&changed, 0, &prevout), Ok(false));
        let (_, different_encoding) = signed(true);
        assert_eq!(
            verify_input_authorization(&tx, 0, &different_encoding),
            Ok(false)
        );
        assert_eq!(
            verify_input_authorization(&tx, 1, &prevout),
            Err(SpendAuthorizationError::InputIndexOutOfRange(1))
        );
        // The owner signs a transaction that lists the outpoint twice.
        let key = secret(7);
        changed = tx.clone();
        changed.input.push(tx.input[0].clone());
        sign_input(&mut changed, 0, &prevout, &key, &compressed(&key), 1);
        assert_eq!(verify_input_authorization(&changed, 0, &prevout), Ok(false));
    }

    #[test]
    fn should_reject_input_scripts_that_cannot_spend_the_funding_output() {
        let (signed_tx, prevout) = signed(false);
        let owner_script = signed_tx.input[0].script_sig.as_bytes();
        let signature_len = usize::from(owner_script[0]);
        let mut oversized = vec![0x61; MAX_SCRIPT_SIZE + 1];
        oversized[..owner_script.len()].copy_from_slice(owner_script);
        let scripts = [
            [owner_script, &[0x00][..]].concat(),
            [&[1, 1][..], &owner_script[signature_len + 1..]].concat(),
            vec![1, 1, 1, 2],
            oversized,
        ];
        for script in scripts {
            let mut tx = signed_tx.clone();
            tx.input[0].script_sig = ScriptBuf::from_bytes(script);
            assert_eq!(verify_input_authorization(&tx, 0, &prevout), Ok(false));
        }
    }

    #[test]
    fn should_keep_a_non_push_only_spend_with_unproven_authorization() {
        let (mut tx, prevout) = signed(false);
        let mut script = tx.input[0].script_sig.as_bytes().to_vec();
        script.push(0x61); // OP_NOP
        tx.input[0].script_sig = ScriptBuf::from_bytes(script);
        tx.output[0].value -= 1;
        assert_eq!(
            verify_input_authorization(&tx, 0, &prevout),
            Err(SpendAuthorizationError::RuntimeAuthorization)
        );
    }

    #[test]
    fn should_bind_asset_lock_payload_to_authorization() {
        let (mut tx, prevout) = signed(false);
        let key = secret(7);
        tx.version = 3;
        tx.special_transaction_payload = Some(TransactionPayload::AssetLockPayloadType(
            AssetLockPayload::new(vec![prevout.clone()]),
        ));
        sign_input(&mut tx, 0, &prevout, &key, &compressed(&key), 1);
        assert_eq!(verify_input_authorization(&tx, 0, &prevout), Ok(true));
        if let Some(TransactionPayload::AssetLockPayloadType(payload)) =
            &mut tx.special_transaction_payload
        {
            payload.credit_outputs[0].value -= 1;
        }
        assert_eq!(verify_input_authorization(&tx, 0, &prevout), Ok(false));
    }

    #[test]
    fn should_preserve_unsupported_authorization_as_an_error() {
        let (signed_tx, prevout) = signed(false);
        let key = secret(7);
        let owner_script = signed_tx.input[0].script_sig.as_bytes();
        let signature = &owner_script[1..=usize::from(owner_script[0])];
        let public_key = &owner_script[signature.len() + 2..];
        let mut tx = signed_tx.clone();
        for mode in [0, 2, 3, 4, 0x81, 0x82, 0x83, 0xff] {
            sign_input(&mut tx, 0, &prevout, &key, public_key, mode);
            assert_eq!(
                verify_input_authorization(&tx, 0, &prevout),
                Err(SpendAuthorizationError::UnsupportedSighash(mode))
            );
            // Swapping only the flag invalidates the signature instead of making it unsupported.
            let mut flagged = signed_tx.clone();
            let mut bytes = owner_script.to_vec();
            bytes[signature.len()] = mode;
            flagged.input[0].script_sig = ScriptBuf::from_bytes(bytes);
            assert_eq!(verify_input_authorization(&flagged, 0, &prevout), Ok(false));
        }
        // Without an output for input 0, SINGLE signs the constant digest 1. That signature
        // still verifies on a changed transaction, and must stay an error there.
        let mut no_outputs = tx.clone();
        no_outputs.output.clear();
        for mode in [3, 0x83] {
            sign_input(&mut no_outputs, 0, &prevout, &key, public_key, mode);
            no_outputs.lock_time += 1;
            assert_eq!(
                verify_input_authorization(&no_outputs, 0, &prevout),
                Err(SpendAuthorizationError::UnsupportedSighash(mode))
            );
        }
        let unknown = TxOut {
            value: prevout.value,
            script_pubkey: ScriptBuf::new(),
        };
        assert_eq!(
            verify_input_authorization(&tx, 0, &unknown),
            Err(SpendAuthorizationError::UnsupportedScript)
        );
        // Each of these input scripts keeps the owner's spend consensus-valid.
        let pushes = |items: &[&[u8]]| {
            items.iter().fold(Builder::new(), |builder, item| {
                builder.push_slice(PushBytesBuf::try_from(item.to_vec()).unwrap())
            })
        };
        let scripts = [
            [&[0x00][..], owner_script].concat(),
            [&[0x61][..], owner_script].concat(),
            [owner_script, &[0x61][..]].concat(),
            pushes(&[signature, public_key, &[1]])
                .push_opcode(OP_DROP)
                .into_bytes(),
            pushes(&[public_key, signature])
                .push_opcode(OP_SWAP)
                .into_bytes(),
        ];
        for script in scripts {
            let mut padded = signed_tx.clone();
            padded.input[0].script_sig = ScriptBuf::from_bytes(script);
            assert_eq!(
                verify_input_authorization(&padded, 0, &prevout),
                Err(SpendAuthorizationError::UnsupportedScriptSig)
            );
            padded.output[0].value -= 1;
            let expected = if crate::tx_actions::push_only(&padded.input[0].script_sig) {
                Ok(false)
            } else {
                Err(SpendAuthorizationError::RuntimeAuthorization)
            };
            assert_eq!(verify_input_authorization(&padded, 0, &prevout), expected);
        }
    }
}
