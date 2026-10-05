//! testdata/uri_cases.json (dash-qt code run against Qt 5.15) and Dash Core's
//! key_io_valid.json / key_io_invalid.json.

use dw_uri::Network;
use dw_uri::core::{SendCoinsRecipient, format_bitcoin_uri, parse_bitcoin_uri};
use dw_uri::keyio::{self, Destination};
use proptest::prelude::*;
use serde_json::Value;

fn load(name: &str) -> Value {
    let path = format!("{}/../../../testdata/{name}", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}")))
        .unwrap()
}

fn int(v: &Value) -> i64 {
    v.as_i64()
        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
        .unwrap_or_else(|| panic!("bad int {v}"))
}

#[test]
fn parse_matches_dash_qt() {
    let doc = load("uri_cases.json");
    let cases = doc["parse"].as_array().unwrap();
    assert!(cases.len() > 200);
    let mut failures = Vec::new();
    for c in cases {
        let input = c["input"].as_str().unwrap();
        let got = parse_bitcoin_uri(input);
        let want = c["ok"].as_bool().unwrap().then(|| SendCoinsRecipient {
            address: c["address"].as_str().unwrap().to_owned(),
            label: c["label"].as_str().unwrap().to_owned(),
            message: c["message"].as_str().unwrap().to_owned(),
            amount: int(&c["amount"]),
        });
        if got != want {
            failures.push(format!("{input:?}\n   got  {got:?}\n   want {want:?}"));
        }
    }
    assert!(
        failures.is_empty(),
        "{} mismatches:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn format_matches_dash_qt() {
    let doc = load("uri_cases.json");
    for c in doc["format"].as_array().unwrap() {
        let r = SendCoinsRecipient {
            address: c["address"].as_str().unwrap().to_owned(),
            label: c["label"].as_str().unwrap().to_owned(),
            message: c["message"].as_str().unwrap().to_owned(),
            amount: int(&c["amount"]),
        };
        let uri = format_bitcoin_uri(&r);
        assert_eq!(uri, c["uri"].as_str().unwrap(), "case {c}");
        let back = &c["reparsed"];
        let got = parse_bitcoin_uri(&uri);
        assert_eq!(
            got.is_some(),
            back["ok"].as_bool().unwrap(),
            "reparse of {uri}"
        );
        if let Some(got) = got {
            assert_eq!(got.address, back["address"].as_str().unwrap());
            assert_eq!(got.label, back["label"].as_str().unwrap());
            assert_eq!(got.message, back["message"].as_str().unwrap());
            assert_eq!(got.amount, int(&back["amount"]));
        }
    }
}

fn network(chain: &str) -> Network {
    match chain {
        "main" => Network::Mainnet,
        "test" => Network::Testnet,
        "regtest" => Network::Regtest,
        other => panic!("chain {other}"),
    }
}

fn hex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

/// Core's key_io_tests.cpp: every valid string decodes on its own chain to
/// the listed script (or key), and re-encodes to the same string.
#[test]
fn key_io_valid() {
    let doc = load("core/key_io_valid.json");
    for t in doc.as_array().unwrap() {
        let s = t[0].as_str().unwrap();
        let payload = hex(t[1].as_str().unwrap());
        let meta = &t[2];
        let net = network(meta["chain"].as_str().unwrap());
        if meta["isPrivkey"].as_bool().unwrap() {
            let secret = keyio::decode_secret(s, net).unwrap_or_else(|| panic!("{s}"));
            assert_eq!(&secret.key[..], payload.as_slice(), "{s}");
            assert_eq!(
                secret.compressed,
                meta["isCompressed"].as_bool().unwrap(),
                "{s}"
            );
            assert_eq!(keyio::encode_secret(&secret, net).as_str(), s);
            assert!(
                keyio::decode_destination(s, net).is_err(),
                "privkey {s} decoded as address"
            );
        } else {
            let dest = keyio::decode_destination(s, net).unwrap_or_else(|e| panic!("{s}: {e}"));
            let script = match dest {
                Destination::PubKeyHash(h) => [&[0x76, 0xa9, 0x14][..], &h, &[0x88, 0xac]].concat(),
                Destination::ScriptHash(h) => [&[0xa9, 0x14][..], &h, &[0x87]].concat(),
            };
            assert_eq!(script, payload, "{s}");
            assert_eq!(keyio::encode_destination(&dest, net), s);
            assert!(
                keyio::decode_secret(s, net).is_none(),
                "address {s} decoded as privkey"
            );
        }
    }
}

/// Core's key_io_tests.cpp: no invalid string decodes on any chain.
#[test]
fn key_io_invalid() {
    let doc = load("core/key_io_invalid.json");
    for t in doc.as_array().unwrap() {
        let s = t[0].as_str().unwrap();
        for net in [Network::Mainnet, Network::Testnet, Network::Regtest] {
            assert!(
                keyio::decode_destination(s, net).is_err(),
                "{s} decoded on {net:?}"
            );
            assert!(
                keyio::decode_secret(s, net).is_none(),
                "{s} decoded as key on {net:?}"
            );
        }
    }
}

/// testdata/address_cases.json: dashd's `validateaddress` verdict, error text
/// and error positions, next to the kind each string has by construction.
#[test]
fn address_cases_match_dashd() {
    use keyio::{AddressKind, DestinationError, PlatformDestination};
    let doc = load("address_cases.json");
    for c in doc["cases"].as_array().unwrap() {
        let input = c["input"].as_str().unwrap();
        let net = network(c["network"].as_str().unwrap());
        let decoded = keyio::decode_destination(input, net);
        let kind = keyio::classify_address(input, net);
        match c["expected_kind"].as_str().unwrap() {
            "core_p2pkh" => {
                assert!(matches!(decoded, Ok(Destination::PubKeyHash(_))), "{input}");
                assert!(matches!(
                    kind,
                    AddressKind::Core(Destination::PubKeyHash(_))
                ));
            }
            "core_p2sh" => {
                assert!(matches!(decoded, Ok(Destination::ScriptHash(_))), "{input}");
                assert!(matches!(
                    kind,
                    AddressKind::Core(Destination::ScriptHash(_))
                ));
            }
            "platform_p2pkh" | "platform_p2sh" => {
                // validateaddress accepts DIP-18 addresses; the L1 decoder
                // rejects them with a dedicated message.
                assert!(c["core_valid"].as_bool().unwrap());
                assert_eq!(decoded, Err(DestinationError::PlatformAddress), "{input}");
                let p2pkh = c["expected_kind"] == "platform_p2pkh";
                assert!(
                    matches!(
                        (&kind, p2pkh),
                        (
                            AddressKind::Platform(PlatformDestination::PubKeyHash(_)),
                            true
                        ) | (
                            AddressKind::Platform(PlatformDestination::ScriptHash(_)),
                            false
                        )
                    ),
                    "{input}: {kind:?}"
                );
            }
            "shielded" => {
                assert_eq!(
                    decoded.unwrap_err().to_string(),
                    c["core_error"].as_str().unwrap()
                );
                assert!(matches!(kind, AddressKind::Shielded(_)), "{input}");
            }
            "invalid" => {
                let e = decoded.expect_err(input);
                assert_eq!(
                    e.to_string(),
                    c["core_error"].as_str().unwrap(),
                    "{input:?}"
                );
                assert_eq!(kind, AddressKind::Invalid(e));
                let want_locations: Vec<usize> = c["core_error_locations"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|v| v.as_u64().unwrap() as usize)
                    .collect();
                if !want_locations.is_empty() {
                    let (msg, got) = keyio::locate_bech32_errors(input);
                    assert_eq!(
                        (msg, got),
                        (c["core_error"].as_str().unwrap(), want_locations),
                        "{input}"
                    );
                }
            }
            other => panic!("kind {other}"),
        }
    }
}

fn arb_text() -> impl Strategy<Value = String> {
    // Unreserved, delimiter, percent and non-ASCII characters.
    proptest::string::string_regex("[a-zA-Z0-9 ~._%&=?#+/:@!$'()*,;é€😀-]{0,24}").unwrap()
}

proptest! {
    // dash-qt's own reader never panics and recovers the address and amount
    // of anything dash-qt writes for a plain Base58 address.
    #[test]
    fn written_uri_reparses(amount in -dw_units::MAX_MONEY..=dw_units::MAX_MONEY,
                            label in arb_text(), message in arb_text()) {
        let r = SendCoinsRecipient {
            address: "XwnLY9Tf7Zsef8gMGL2fhWA9ZmMjt4KPwg".into(), label, message, amount,
        };
        let back = parse_bitcoin_uri(&format_bitcoin_uri(&r)).expect("dash-qt output parses");
        prop_assert_eq!(back.address, r.address);
        prop_assert_eq!(back.amount, r.amount);
    }

    #[test]
    fn parser_is_total(s in "\\PC{0,64}") {
        let _ = parse_bitcoin_uri(&s);
        let _ = dw_uri::ext::parse_payment_string(&s);
        let _ = dw_uri::deeplink::classify_link(&s);
        let _ = keyio::classify_address(&s, Network::Mainnet);
    }
}
