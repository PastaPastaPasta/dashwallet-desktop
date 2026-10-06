//! Against dashd's PSBTs (testdata/compat/psbt, from
//! testdata/oracle/dashd/gen_compat_vectors.py): a 2-input PSBT made by
//! `walletcreatefundedpsbt`, signed by `walletprocesspsbt` and finalized by
//! `finalizepsbt`. RFC 6979 makes ECDSA deterministic, so our signatures
//! over the same sighashes must equal dashd's byte for byte.

use super::*;
use async_trait::async_trait;
use dashcore::secp256k1::PublicKey;
use key_wallet::SignerMethod;
use key_wallet::bip32::ExtendedPrivKey;

fn vector(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../testdata/compat")
        .join(name);
    std::fs::read_to_string(path).unwrap()
}

fn manifest() -> serde_json::Value {
    serde_json::from_str(&vector("manifest.json")).unwrap()
}

struct SeedSigner(ExtendedPrivKey);

#[async_trait]
impl Signer for SeedSigner {
    type Error = String;

    fn supported_methods(&self) -> &[SignerMethod] {
        &[SignerMethod::Digest]
    }

    async fn sign_ecdsa(
        &self,
        path: &DerivationPath,
        sighash: [u8; 32],
    ) -> Result<(secp256k1::ecdsa::Signature, PublicKey), String> {
        let secp = Secp256k1::new();
        let k = self.0.derive_priv(&secp, path).map_err(|e| e.to_string())?;
        Ok((
            secp.sign_ecdsa_low_r(&Message::from_digest(sighash), &k.private_key),
            PublicKey::from_secret_key(&secp, &k.private_key),
        ))
    }

    async fn public_key(&self, path: &DerivationPath) -> Result<PublicKey, String> {
        let secp = Secp256k1::new();
        let k = self.0.derive_priv(&secp, path).map_err(|e| e.to_string())?;
        Ok(PublicKey::from_secret_key(&secp, &k.private_key))
    }
}

fn signer() -> SeedSigner {
    let m = manifest();
    let phrase = m["psbt"]["mnemonic"].as_str().unwrap();
    let seed = dw_compat::bip39core::core_seed(phrase.as_bytes(), b"");
    SeedSigner(ExtendedPrivKey::new_master(Network::Regtest, &seed[..]).unwrap())
}

/// Paths from the PSBT's own BIP32 derivations, as the engine would find
/// them for its wallet's keys.
fn paths(psbt: &PartiallySignedTransaction) -> KeyPaths {
    psbt.inputs
        .iter()
        .enumerate()
        .filter_map(|(i, input)| {
            input
                .bip32_derivation
                .values()
                .next()
                .map(|(_, p)| (i, p.clone()))
        })
        .collect()
}

#[test]
fn test_qt_078_parses_dashd_psbts_and_writes_them_back() {
    for name in ["psbt/unsigned.b64", "psbt/signed.b64"] {
        let text = vector(name);
        let psbt = parse(text.as_bytes()).unwrap();
        assert_eq!(to_base64(&psbt), text.trim(), "{name}");
        let again = parse(&to_bytes(&psbt)).unwrap();
        assert_eq!(again, psbt);
    }
    assert!(matches!(parse(b"not a psbt"), Err(PsbtError::Invalid(_))));
    assert!(matches!(
        parse(&vec![b'A'; MAX_PSBT_BYTES]),
        Err(PsbtError::TooLarge(_))
    ));
}

#[test]
fn test_qt_079_analysis_matches_dashd() {
    let m = manifest();
    let unsigned = parse(vector("psbt/unsigned.b64").as_bytes()).unwrap();
    let a = analyze(&unsigned, Network::Regtest);
    assert_eq!(a.fee, Some(m["psbt"]["fee"].as_u64().unwrap()));
    assert_eq!(a.status, Status::NeedsSignatures);
    assert_eq!(a.unsigned_inputs, 2);
    assert_eq!(
        a.outputs[0].address.as_deref(),
        m["psbt"]["destination"].as_str()
    );
    assert_eq!(a.total, Some(a.outputs.iter().map(|o| o.amount).sum()));
    // dashd's analyzepsbt estimate for the signed transaction.
    assert_eq!(
        a.estimated_size as u64,
        m["psbt"]["analyze_unsigned"]["estimated_vsize"]
            .as_u64()
            .unwrap()
    );

    let signed = parse(vector("psbt/signed.b64").as_bytes()).unwrap();
    let a = analyze(&signed, Network::Regtest);
    assert_eq!(a.status, Status::Complete);
    // Partially signed, not finalized: dash-qt still counts them unsigned.
    assert_eq!(a.unsigned_inputs, 2);
}

#[tokio::test]
async fn test_qt_079_signatures_equal_walletprocesspsbt() {
    let mut psbt = parse(vector("psbt/unsigned.b64").as_bytes()).unwrap();
    let dashd = parse(vector("psbt/signed.b64").as_bytes()).unwrap();
    let p = paths(&psbt);
    assert_eq!(p.len(), 2);
    assert_eq!(sign(&mut psbt, &p, &signer()).await.unwrap(), 2);
    for (ours, theirs) in psbt.inputs.iter().zip(&dashd.inputs) {
        assert_eq!(ours.partial_sigs, theirs.partial_sigs);
    }
    // A second pass signs nothing: every input is complete.
    assert_eq!(sign(&mut psbt, &p, &signer()).await.unwrap(), 0);

    assert!(finalize(&mut psbt));
    assert_eq!(analyze(&psbt, Network::Regtest).unsigned_inputs, 0);
    let tx = extract(&psbt).unwrap();
    assert_eq!(
        dashcore::consensus::encode::serialize_hex(&tx),
        vector("psbt/final.hex").trim()
    );
    assert_eq!(
        tx.txid().to_string(),
        manifest()["psbt"]["final_txid"].as_str().unwrap()
    );
    let rate = fee_rate_per_kb(&psbt, &tx).unwrap();
    assert!(rate < MAX_BROADCAST_FEE_PER_KB);
}

#[tokio::test]
async fn signing_with_a_wrong_key_is_refused() {
    let mut psbt = parse(vector("psbt/unsigned.b64").as_bytes()).unwrap();
    let mut p = paths(&psbt);
    p.insert(0, "m/44'/1'/0'/0/77".parse().unwrap());
    let err = sign(&mut psbt, &p, &signer()).await.unwrap_err();
    assert!(matches!(err, PsbtError::Signing { index: 0, .. }), "{err}");
    assert!(extract(&psbt).is_err());
}

#[test]
fn create_unsigned_round_trips_dashd_inputs() {
    let dashd = parse(vector("psbt/unsigned.b64").as_bytes()).unwrap();
    let inputs = dashd
        .inputs
        .iter()
        .map(|i| InputData {
            prev_tx: i.non_witness_utxo.clone().unwrap(),
            derivation: i
                .bip32_derivation
                .iter()
                .next()
                .map(|(pk, (fp, path))| (*pk, *fp, path.clone())),
        })
        .collect();
    let outputs = dashd
        .outputs
        .iter()
        .map(|o| {
            o.bip32_derivation
                .iter()
                .next()
                .map(|(pk, (fp, path))| (*pk, *fp, path.clone()))
        })
        .collect();
    let ours = create_unsigned(dashd.unsigned_tx.clone(), inputs, outputs).unwrap();
    assert_eq!(to_base64(&ours), vector("psbt/unsigned.b64").trim());
}
