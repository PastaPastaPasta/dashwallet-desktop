//! testdata/uri_ext_cases.json: the iOS-superset parser, the iOS URI writer,
//! deep-link routing and invitation normalization.

use dw_uri::deeplink::{self, DeepLink};
use dw_uri::ext::{self, PaymentKind, PaymentRequest, PaymentUriBuilder};
use serde_json::Value;

fn load() -> Value {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../testdata/uri_ext_cases.json"
    );
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn opt_str(v: &Value) -> Option<&str> {
    v.as_str()
}

/// Compares the fields a case lists against a parsed request.
fn check_payment(c: &Value, p: &PaymentRequest) {
    let input = c["input"].as_str().unwrap();
    let obj = c.as_object().unwrap();
    for (k, want) in obj {
        match k.as_str() {
            "from" | "input" | "kind" => {}
            "scheme" => assert_eq!(p.scheme, want.as_str().unwrap(), "{input}"),
            "address" => assert_eq!(p.address.as_deref(), opt_str(want), "{input}"),
            "amount" => assert_eq!(p.amount, want.as_u64(), "{input}"),
            "label" => assert_eq!(p.label.as_deref(), opt_str(want), "{input}"),
            "message" => assert_eq!(p.message.as_deref(), opt_str(want), "{input}"),
            "r" => assert_eq!(p.r.as_deref(), opt_str(want), "{input}"),
            "callback_scheme" => assert_eq!(p.callback_scheme.as_deref(), opt_str(want), "{input}"),
            "dashpay_username" => {
                assert_eq!(p.dashpay_username.as_deref(), opt_str(want), "{input}")
            }
            "fiat_currency_code" => {
                assert_eq!(p.fiat_currency_code.as_deref(), opt_str(want), "{input}")
            }
            "fiat_amount" => assert_eq!(p.fiat_amount, want.as_f64().map(|f| f as f32), "{input}"),
            "required_fields" => {
                let want: Vec<&str> = want
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|v| v.as_str().unwrap())
                    .collect();
                let got: Vec<&str> = p.required_fields.iter().map(String::as_str).collect();
                assert_eq!(got, want, "{input}");
            }
            other => panic!("unknown field {other} in {input}"),
        }
    }
    if let Some(kind) = c.get("kind").and_then(Value::as_str) {
        let want = match kind {
            "payment_uri" => PaymentKind::PaymentUri,
            "bare_address" => PaymentKind::BareAddress,
            "bip73_url" => PaymentKind::Bip73Url,
            k => panic!("kind {k}"),
        };
        assert_eq!(p.kind, want, "{input}");
    }
}

#[test]
fn payment_strings() {
    let doc = load();
    for c in doc["payment"].as_array().unwrap() {
        let input = c["input"].as_str().unwrap();
        let parsed = ext::parse_payment_string(input);
        if c.get("rejected").is_some() {
            assert!(parsed.is_none(), "{input}");
            continue;
        }
        check_payment(c, &parsed.unwrap_or_else(|| panic!("{input} rejected")));
    }
}

#[test]
fn builder() {
    let doc = load();
    for c in doc["builder"].as_array().unwrap() {
        let s = |k: &str| c.get(k).and_then(Value::as_str).map(str::to_owned);
        let b = PaymentUriBuilder {
            address: s("address").unwrap(),
            amount: c["amount"].as_u64().unwrap(),
            label: s("label"),
            message: s("message"),
            request_url: s("request_url"),
            fiat_currency_code: s("fiat_currency_code"),
            fiat_amount: c.get("fiat_amount").and_then(Value::as_f64).unwrap_or(0.0) as f32,
            dashpay_username: s("dashpay_username"),
        };
        let uri = b.build();
        assert_eq!(uri, c["uri"].as_str().unwrap());
        // What the iOS writer produces, the parser reads back.
        let back = ext::parse_payment_uri(&uri).unwrap();
        assert_eq!(back.address.as_deref(), Some(b.address.as_str()));
        assert_eq!(back.amount.unwrap_or(0), b.amount);
        assert_eq!(back.label, b.label);
        assert_eq!(back.message, b.message);
    }
}

#[test]
fn links() {
    let doc = load();
    for c in doc["link"].as_array().unwrap() {
        let input = c["input"].as_str().unwrap();
        let got = deeplink::classify_link(input);
        let kind = match &got {
            DeepLink::DashConnectKey(_) => "dashconnect_key",
            DeepLink::DashConnectSt(_) => "dashconnect_st",
            DeepLink::Integration(_) => "integration",
            DeepLink::ScanQr => "scan_qr",
            DeepLink::AddressRequest { .. } => "address_request",
            DeepLink::Payment(_) => "payment",
            DeepLink::DashPayUser(_) => "dashpay_user",
            DeepLink::Invitation(_) => "invitation",
            DeepLink::Unknown => "unknown",
        };
        assert_eq!(kind, c["kind"].as_str().unwrap(), "{input}: {got:?}");
        match &got {
            DeepLink::AddressRequest { sender, request } => {
                assert_eq!(sender, c["sender"].as_str().unwrap());
                assert_eq!(request, "address");
            }
            DeepLink::Payment(p) => {
                let mut fields = c.clone();
                fields.as_object_mut().unwrap().remove("kind");
                check_payment(&fields, p);
            }
            DeepLink::DashPayUser(u) => {
                assert_eq!(u.username, c["username"].as_str().unwrap());
                if let Some(id) = c.get("identity_id_hex") {
                    let hex: String = u.identity_id.iter().map(|b| format!("{b:02x}")).collect();
                    assert_eq!(hex, id.as_str().unwrap());
                }
            }
            _ => {}
        }
    }
}

#[test]
fn invitations() {
    let doc = load();
    for c in doc["invitation"].as_array().unwrap() {
        let input = c["input"].as_str().unwrap();
        assert_eq!(
            deeplink::normalize_invitation(input).as_deref(),
            c["normalized"].as_str(),
            "{input}"
        );
    }
}
