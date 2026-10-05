//! testdata/message_cases.json: dashd v24 signmessagewithprivkey/signmessage/
//! verifymessage results, plus Dash Core's unit and functional test vectors.

use dashcore::secp256k1::Secp256k1;
use dw_message::{VerifyError, message_hash, sign_message, sign_message_with_wif, verify_message};
use dw_uri::Network;
use dw_uri::keyio::Secret;
use proptest::prelude::*;
use serde_json::Value;
use zeroize::Zeroizing;

fn load() -> Value {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../testdata/message_cases.json"
    );
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn network(v: &Value) -> Network {
    match v["network"].as_str().unwrap() {
        "mainnet" => Network::Mainnet,
        "testnet" => Network::Testnet,
        "regtest" => Network::Regtest,
        n => panic!("network {n}"),
    }
}

fn secret(hex: &str, compressed: bool) -> Secret {
    let mut key = Zeroizing::new([0u8; 32]);
    for (i, b) in key.iter_mut().enumerate() {
        *b = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).unwrap();
    }
    Secret { key, compressed }
}

#[test]
fn dashd_signatures_reproduce_and_verify() {
    let doc = load();
    let cases = doc["sign"].as_array().unwrap();
    assert!(cases.len() >= 40);
    for c in cases {
        let msg = c["message"].as_str().unwrap().as_bytes();
        let net = network(c);
        let want = c["signature"].as_str().unwrap();
        assert_eq!(
            sign_message_with_wif(c["wif"].as_str().unwrap(), net, msg).unwrap(),
            want,
            "{}",
            c["wif"]
        );
        let s = secret(
            c["secret_hex"].as_str().unwrap(),
            c["compressed"].as_bool().unwrap(),
        );
        assert_eq!(sign_message(&s, msg).unwrap(), want);
        assert_eq!(
            c["signmessage"].as_str().unwrap(),
            want,
            "wallet signmessage differs from signmessagewithprivkey"
        );
        assert!(c["verify"].as_bool().unwrap());
        verify_message(c["address"].as_str().unwrap(), want, msg, net).unwrap();
    }
}

#[test]
fn dashd_verify_results() {
    let doc = load();
    for c in doc["verify"].as_array().unwrap() {
        let got = verify_message(
            c["address"].as_str().unwrap(),
            c["signature"].as_str().unwrap(),
            c["message"].as_str().unwrap().as_bytes(),
            network(c),
        );
        match (&c["result"], &c["error"]) {
            (Value::Bool(true), _) => assert_eq!(got, Ok(()), "{}", c["case"]),
            (Value::Bool(false), _) => {
                let e = got.expect_err(c["case"].as_str().unwrap());
                assert_eq!(
                    e.rpc_message(),
                    None,
                    "{}: RPC returns false, not an error",
                    c["case"]
                );
            }
            (_, Value::String(rpc_error)) => {
                assert_eq!(
                    got.unwrap_err().rpc_message(),
                    Some(rpc_error.as_str()),
                    "{}",
                    c["case"]
                );
            }
            other => panic!("bad case {other:?}"),
        }
    }
}

#[test]
fn core_unit_and_functional_vectors() {
    let doc = load();
    let core = &doc["core_tests"];
    for c in core["sign"].as_array().unwrap() {
        let msg = c["message"].as_str().unwrap().as_bytes();
        let net = network(c);
        let sig = match c.get("wif") {
            Some(w) => sign_message_with_wif(w.as_str().unwrap(), net, msg).unwrap(),
            None => sign_message(&secret(c["secret_hex"].as_str().unwrap(), true), msg).unwrap(),
        };
        assert_eq!(sig, c["signature"].as_str().unwrap(), "{}", c["source"]);
        verify_message(c["address"].as_str().unwrap(), &sig, msg, net).unwrap();
    }
    for c in core["verify"].as_array().unwrap() {
        let got = verify_message(
            c["address"].as_str().unwrap(),
            c["signature"].as_str().unwrap(),
            c["message"].as_str().unwrap().as_bytes(),
            network(c),
        );
        let want = match c["result"].as_str().unwrap() {
            "OK" => Ok(()),
            "ERR_INVALID_ADDRESS" => Err(VerifyError::InvalidAddress),
            "ERR_ADDRESS_NO_KEY" => Err(VerifyError::AddressNoKey),
            "ERR_MALFORMED_SIGNATURE" => Err(VerifyError::MalformedSignature),
            "ERR_PUBKEY_NOT_RECOVERED" => Err(VerifyError::PubkeyNotRecovered),
            "ERR_NOT_SIGNED" => Err(VerifyError::NotSigned),
            r => panic!("result {r}"),
        };
        assert_eq!(got, want, "{c}");
    }
}

/// rust-dashcore's own helpers agree on the digest and on verification.
#[test]
fn agrees_with_rust_dashcore_sign_message() {
    let doc = load();
    let secp = Secp256k1::verification_only();
    for c in doc["sign"].as_array().unwrap() {
        let m = c["message"].as_str().unwrap();
        let theirs = dashcore::sign_message::signed_msg_hash(m);
        assert_eq!(
            dashcore::hashes::Hash::to_byte_array(theirs),
            message_hash(m.as_bytes())
        );
        let sig =
            dashcore::sign_message::MessageSignature::from_base64(c["signature"].as_str().unwrap())
                .unwrap();
        let addr: dashcore::Address<dashcore::address::NetworkUnchecked> =
            c["address"].as_str().unwrap().parse().unwrap();
        let addr = addr.require_network(Network::Regtest).unwrap();
        assert!(sig.is_signed_by_address(&secp, &addr, theirs).unwrap());
    }
}

proptest! {
    #[test]
    fn sign_then_verify(key in prop::array::uniform32(1u8..), compressed in any::<bool>(),
                        msg in prop::collection::vec(any::<u8>(), 0..400)) {
        let s = Secret { key: Zeroizing::new(key), compressed };
        let sig = sign_message(&s, &msg).unwrap();
        let secp = Secp256k1::signing_only();
        let sk = dashcore::secp256k1::SecretKey::from_byte_array(&key).unwrap();
        let pk = dashcore::secp256k1::PublicKey::from_secret_key(&secp, &sk);
        let ser = if compressed { pk.serialize().to_vec() } else { pk.serialize_uncompressed().to_vec() };
        let hash = <dashcore::hashes::hash160::Hash as dashcore::hashes::Hash>::hash(&ser);
        let addr = dw_uri::keyio::encode_destination(
            &dw_uri::keyio::Destination::PubKeyHash(dashcore::hashes::Hash::to_byte_array(hash)), Network::Mainnet);
        prop_assert_eq!(verify_message(&addr, &sig, &msg, Network::Mainnet), Ok(()));
        let mut other = msg.clone();
        other.push(0);
        prop_assert_eq!(verify_message(&addr, &sig, &other, Network::Mainnet), Err(VerifyError::NotSigned));
    }
}
