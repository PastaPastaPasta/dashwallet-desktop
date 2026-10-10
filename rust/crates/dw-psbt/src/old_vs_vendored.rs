//! TEMPORARY: DELETE AT THE PIN MOVE (E0-10c), together with its `mod`
//! line in lib.rs.
//!
//! Compares the vendored container (`crate::psbt`) with key-wallet's, which
//! the current pin (rust-dashcore 40268cc0) still ships, over dw-psbt's
//! dashd vectors and mutations of them. The module goes away from key-wallet
//! with rust-dashcore #1041, so this cannot outlive the move.
//!
//! The upstream module is imported through a renamed `use` so that a grep
//! for the old path finds nothing outside this file's own docs.

use base64::Engine as _;
use dashcore::Network;
use dashcore::hashes::{Hash, hash160, ripemd160, sha256, sha256d};
use dashcore::secp256k1::{self, Secp256k1};
use key_wallet::bip32::{ExtendedPrivKey, ExtendedPubKey};
use key_wallet::psbt as upstream;

use crate::psbt::PartiallySignedTransaction as Vendored;

fn vector(name: &str) -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../testdata/compat")
        .join(name);
    let text = std::fs::read_to_string(path).unwrap();
    base64::engine::general_purpose::STANDARD
        .decode(text.trim())
        .unwrap()
}

/// The vectors plus PSBTs built from them with the fields the container
/// carries beyond what the dashd vectors use.
fn corpus() -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    for name in ["psbt/unsigned.b64", "psbt/signed.b64"] {
        out.push((name.to_string(), vector(name)));
    }
    let mut psbt = Vendored::deserialize(&out[0].1).unwrap();
    // A finalized one: final scriptSigs, signing data dropped.
    let mut done = Vendored::deserialize(&vector("psbt/signed.b64")).unwrap();
    assert!(crate::finalize(&mut done));
    out.push(("finalized".into(), done.serialize()));
    // Every container field set: sighash type, witness data, redeem and
    // witness scripts, preimages, proprietary and unknown pairs, and the
    // global xpub, proprietary and unknown maps.
    let input = &mut psbt.inputs[0];
    input.sighash_type = Some(crate::PsbtSighashType::from_u32(1));
    input.redeem_script = Some(dashcore::ScriptBuf::from(vec![0x51]));
    input.witness_script = Some(dashcore::ScriptBuf::from(vec![0x52]));
    input.final_script_witness = Some(dashcore::Witness::from_slice(&[vec![1u8, 2], vec![3u8]]));
    input.witness_utxo = Some(dashcore::TxOut {
        value: 7,
        script_pubkey: dashcore::ScriptBuf::from(vec![0x6a]),
    });
    input
        .ripemd160_preimages
        .insert(ripemd160::Hash::hash(b"abc"), b"abc".to_vec());
    input
        .sha256_preimages
        .insert(sha256::Hash::hash(b"def"), b"def".to_vec());
    input
        .hash256_preimages
        .insert(sha256d::Hash::hash(b"def"), b"def".to_vec());
    input
        .hash160_preimages
        .insert(hash160::Hash::hash(b"ghi"), b"ghi".to_vec());
    input.proprietary.insert(
        crate::psbt::raw::ProprietaryKey {
            prefix: b"dw".to_vec(),
            subtype: 3,
            key: vec![9, 9],
        },
        vec![1, 2, 3],
    );
    input.unknown.insert(
        crate::psbt::raw::Key {
            type_value: 0x42,
            key: vec![1],
        },
        vec![5],
    );
    let secp = Secp256k1::new();
    let sk = secp256k1::SecretKey::from_slice(&[7u8; 32]).unwrap();
    let pk = secp256k1::PublicKey::from_secret_key(&secp, &sk);
    psbt.outputs[0].redeem_script = Some(dashcore::ScriptBuf::from(vec![0x53]));
    psbt.outputs[0].witness_script = Some(dashcore::ScriptBuf::from(vec![0x54]));
    psbt.outputs[0].bip32_derivation.insert(
        pk,
        ([1, 2, 3, 4].into(), "m/44'/1'/0'/1/5".parse().unwrap()),
    );
    psbt.unknown.insert(
        crate::psbt::raw::Key {
            type_value: 0x77,
            key: vec![],
        },
        vec![8],
    );
    psbt.outputs[1].proprietary.insert(
        crate::psbt::raw::ProprietaryKey {
            prefix: b"dw".to_vec(),
            subtype: 2,
            key: vec![],
        },
        vec![9],
    );
    psbt.outputs[1].unknown.insert(
        crate::psbt::raw::Key {
            type_value: 0x66,
            key: vec![2],
        },
        vec![1],
    );
    psbt.proprietary.insert(
        crate::psbt::raw::ProprietaryKey {
            prefix: b"dw".to_vec(),
            subtype: 1,
            key: vec![4],
        },
        vec![6],
    );
    let master = ExtendedPrivKey::new_master(Network::Regtest, &[3u8; 32]).unwrap();
    psbt.xpub.insert(
        ExtendedPubKey::from_priv(&secp, &master),
        ([5, 6, 7, 8].into(), "m/44'/1'/0'".parse().unwrap()),
    );
    out.push(("every-field".into(), psbt.serialize()));
    out
}

/// Both implementations agree on `bytes`: same verdict, and when it parses
/// the same re-serialization and the same error text otherwise.
fn assert_agree(label: &str, bytes: &[u8]) {
    match (
        upstream::PartiallySignedTransaction::deserialize(bytes),
        Vendored::deserialize(bytes),
    ) {
        (Ok(old), Ok(new)) => {
            assert_eq!(
                old.serialize(),
                new.serialize(),
                "{label}: re-serialization"
            );
        }
        (Err(old), Err(new)) => {
            assert_eq!(old.to_string(), new.to_string(), "{label}: error text");
        }
        (old, new) => panic!(
            "{label}: verdicts differ: upstream {:?}, vendored {:?}",
            old.map(|_| ()),
            new.map(|_| ())
        ),
    }
}

#[test]
fn vendored_container_matches_upstream_byte_for_byte() {
    for (name, bytes) in corpus() {
        // Parses and re-serializes identically in both, and the canonical
        // encoding survives the round trip byte for byte.
        assert_agree(&name, &bytes);
        let new = Vendored::deserialize(&bytes).unwrap();
        assert_eq!(new.serialize(), bytes, "{name}: round trip");
    }
}

#[test]
fn vendored_container_agrees_on_truncated_and_corrupted_input() {
    for (name, bytes) in corpus() {
        for len in 0..bytes.len() {
            assert_agree(&format!("{name} cut at {len}"), &bytes[..len]);
        }
        for at in 0..bytes.len() {
            let mut bad = bytes.clone();
            bad[at] ^= 0xff;
            assert_agree(&format!("{name} flipped at {at}"), &bad);
        }
    }
}

/// `PsbtSighashType`'s text is upstream's for every one-byte value and
/// samples above it (the vendored `Display` is a plain match, upstream's
/// goes through the taproot sighash type).
#[test]
fn sighash_type_text_matches_upstream() {
    for n in (0..=0x1ffu32).chain([0xdddd_dddd, u32::MAX]) {
        assert_eq!(
            crate::PsbtSighashType::from_u32(n).to_string(),
            upstream::PsbtSighashType::from_u32(n).to_string(),
            "{n:#x}"
        );
    }
}
