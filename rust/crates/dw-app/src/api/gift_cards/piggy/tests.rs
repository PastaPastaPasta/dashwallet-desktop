use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::thread;

use dw_vault::MemoryOsStore;

use super::*;
use crate::api::gift_cards::{GiftAccount, GiftInvoice, store::Secrets};

const INVOICE: &str = "dash:Xs6DNcUzKPYSvDDxpwFdDpGk4kNqhnUQ1T?amount=0.12345678";

struct Mock {
    request_line: &'static str,
    authorization: Option<&'static str>,
    body: Option<Value>,
    response: &'static str,
}

fn mock_piggy(fixtures: Vec<Mock>) -> (Piggy, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = Url::parse(&format!(
        "http://{}/dash/v1/",
        listener.local_addr().unwrap()
    ))
    .unwrap();
    let server = thread::spawn(move || {
        for fixture in fixtures {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut reader = BufReader::new(&mut socket);
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            assert_eq!(line.trim_end(), fixture.request_line);
            let mut length = 0;
            let mut authorization = None;
            loop {
                let mut line = String::new();
                assert_ne!(reader.read_line(&mut line).unwrap(), 0);
                if line == "\r\n" {
                    break;
                }
                let (name, value) = line.split_once(':').unwrap();
                if name.eq_ignore_ascii_case("content-length") {
                    length = value.trim().parse::<usize>().unwrap();
                }
                if name.eq_ignore_ascii_case("authorization") {
                    authorization = Some(value.trim().to_owned());
                }
            }
            assert_eq!(authorization.as_deref(), fixture.authorization);
            let mut body = vec![0; length];
            reader.read_exact(&mut body).unwrap();
            assert_eq!(serde_json::from_slice::<Value>(&body).ok(), fixture.body);
            write!(socket, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", fixture.response.len(), fixture.response).unwrap();
        }
    });
    (Piggy::at(url).unwrap(), server)
}

fn get(request_line: &'static str, response: &'static str) -> Mock {
    Mock {
        request_line,
        authorization: Some("Bearer fixture-token"),
        body: None,
        response,
    }
}

#[tokio::test]
async fn otp_sign_in_catalog_invoice_and_claim_follow_the_provider_contract() {
    let login = || Mock {
        request_line: "POST /dash/v1/login HTTP/1.1",
        authorization: None,
        body: Some(json!({"userId":"fixture-user","password":"fixture-password"})),
        response: r#"{"access_token":"fixture-token","token_type":"bearer","expires_in":3600}"#,
    };
    let (piggy, server) = mock_piggy(vec![
        Mock {
            request_line: "POST /dash/v1/signup HTTP/1.1",
            authorization: None,
            body: Some(
                json!({"firstName":"Fixture","lastName":"Buyer","email":"fixture@example.invalid","country":"US","state":"CA"}),
            ),
            response: r#"{"userId":"fixture-user"}"#,
        },
        Mock {
            request_line: "POST /dash/v1/verify-otp HTTP/1.1",
            authorization: None,
            body: Some(json!({"email":"fixture@example.invalid","otp":"123456"})),
            response: r#"{"generatedPassword":"fixture-password"}"#,
        },
        login(),
        get(
            "GET /dash/v1/brands/US HTTP/1.1",
            r#"[{"id":"45","name":"Fixture Groceries"}]"#,
        ),
        get(
            "GET /dash/v1/giftcards/US?brandId=45 HTTP/1.1",
            r#"{"code":200,"message":"OK","data":[{"id":21,"name":"Fixture Groceries","currency":"USD","price_type":"option","min_denomination":0,"max_denomination":0,"denomination":"25, 50.50","quantity":5}]}"#,
        ),
        Mock {
            request_line: "POST /dash/v1/orders HTTP/1.1",
            authorization: Some("Bearer fixture-token"),
            body: Some(json!({
                "orders":[{"productId":21,"quantity":1,"denomination":25.0,"currency":"USD"}],
                "recipientEmail":"fixture@example.invalid",
                "user":{"name":"Fixture Buyer","ip":"192.168.100.1","metadata":{"registeredSince":"2025-07-01","country":"US","state":"CA"}}
            })),
            response: r#"{"id":"fixture-order","payTo":null}"#,
        },
        get(
            "GET /dash/v1/orders/fixture-order HTTP/1.1",
            r#"{"code":200,"message":"OK","data":{"orderId":"fixture-order","payTo":"dash:Xs6DNcUzKPYSvDDxpwFdDpGk4kNqhnUQ1T?amount=0.12345678","status":"Payment pending","cards":[]}}"#,
        ),
        get(
            "GET /dash/v1/orders/fixture-order HTTP/1.1",
            r#"{"code":200,"message":"OK","data":{"orderId":"fixture-order","status":"Complete","cards":[{"name":"Fixture Groceries","claimCode":"FIXTURE-CODE","claimPin":"0000"},{"name":"Fixture link-only card","claimLink":"https://redeem.fixture.invalid/claim"}]}}"#,
        ),
        login(),
    ]);

    let folder = tempfile::tempdir().unwrap();
    let os = std::sync::Arc::new(MemoryOsStore::default());
    let secrets = Secrets::new(folder.path(), os.clone()).unwrap();
    let profile = GiftSignup {
        first_name: "Fixture".into(),
        last_name: "Buyer".into(),
        email: "fixture@example.invalid".into(),
        state: "CA".into(),
    };
    assert!(matches!(
        secrets.signup(&piggy, profile).await.unwrap(),
        GiftAccount::CodeSent { .. }
    ));
    assert!(matches!(
        secrets
            .verify_email(&piggy, Zeroizing::new("123456".into()))
            .await
            .unwrap(),
        GiftAccount::SignedIn { .. }
    ));
    let (account, token) = secrets.session(&piggy).await.unwrap();
    let brands = piggy.brands(&token.access).await.unwrap();
    let brand = &brands[0];
    assert_eq!(brand.name, "Fixture Groceries");
    let products = piggy.products(&brand.id, &token.access).await.unwrap();
    assert_eq!(products[0].values, [2500, 5050]);
    let order = piggy
        .create_order(&token.access, products[0].id, 2500, &account.signup)
        .await
        .unwrap();
    assert!(order.pay_to.is_none());
    let unpaid = piggy.order(&order.id, &token.access).await.unwrap();
    assert_eq!(unpaid.pay_to.as_deref(), Some(INVOICE));
    assert_eq!(
        GiftInvoice::parse(INVOICE, dashcore::Network::Mainnet)
            .unwrap()
            .duffs,
        12_345_678
    );
    let fulfilled = piggy.order(&order.id, &token.access).await.unwrap();
    assert_eq!(
        fulfilled.cards[0].claim_code.as_deref(),
        Some("FIXTURE-CODE")
    );
    assert_eq!(fulfilled.cards[0].claim_pin.as_deref(), Some("0000"));
    let link = fulfilled.cards[1].claim_link.as_deref().unwrap();
    assert_eq!(
        crate::api::gift_cards::claim_url(link).unwrap().domain(),
        Some("redeem.fixture.invalid")
    );
    assert!(matches!(
        secrets.sign_out().await.unwrap(),
        GiftAccount::CanSignIn { .. }
    ));
    let reopened = Secrets::new(folder.path(), os).unwrap();
    assert!(matches!(
        reopened.sign_in(&piggy).await.unwrap(),
        GiftAccount::SignedIn { .. }
    ));
    assert_eq!(std::fs::read_dir(folder.path()).unwrap().count(), 0);
    server.join().unwrap();
}

#[test]
fn only_unambiguous_positive_bip21_invoices_on_this_network_can_pay() {
    let mainnet = dashcore::Network::Mainnet;
    assert!(GiftInvoice::parse(INVOICE, mainnet).is_ok());
    assert!(GiftInvoice::parse(INVOICE, dashcore::Network::Testnet).is_err());
    for suffix in [
        "amount=0",
        "amount=0.000000001",
        "amount=1&amount=2",
        "amount=1&%61mount=2",
        "amount=1&%72=https://fixture.invalid/",
        "amount=1&req-amount=2",
        "amount=1&r=https://fixture.invalid/",
        "amount=1#ignored",
        "label=fixture",
    ] {
        let uri = format!("{}?{suffix}", INVOICE.split_once('?').unwrap().0);
        assert!(GiftInvoice::parse(&uri, mainnet).is_err(), "{suffix}");
    }
    assert!(GiftInvoice::parse("dash:?r=https://fixture.invalid/", mainnet).is_err());
}

#[test]
fn a_signing_draft_must_match_the_invoice_and_keep_change_in_the_wallet() {
    use dw_engine::PreparedOutput;

    let invoice = GiftInvoice::parse(INVOICE, dashcore::Network::Mainnet).unwrap();
    let recipient = PreparedOutput {
        address: Some(invoice.address.clone()),
        amount: invoice.duffs,
        is_change: false,
        is_mine: false,
        label: None,
    };
    let change = PreparedOutput {
        address: None,
        amount: 9024,
        is_change: true,
        is_mine: true,
        label: None,
    };
    assert!(invoice.verify(&[recipient.clone(), change.clone()]).is_ok());
    let wrong_amount = PreparedOutput {
        amount: invoice.duffs - 1,
        ..recipient.clone()
    };
    assert!(invoice.verify(&[wrong_amount, change.clone()]).is_err());
    let changed_address = PreparedOutput {
        address: Some("other-address".into()),
        ..recipient.clone()
    };
    assert!(invoice.verify(&[changed_address, change.clone()]).is_err());
    let redirected_change = PreparedOutput {
        is_mine: false,
        ..change.clone()
    };
    assert!(
        invoice
            .verify(&[recipient.clone(), redirected_change])
            .is_err()
    );
    assert!(invoice.verify(&[recipient.clone(), recipient]).is_err());
}

#[test]
fn redemption_links_require_credentialless_public_https_urls() {
    use crate::api::gift_cards::claim_url;

    for bad in [
        "http://merchant.invalid/claim",
        "https://name:secret@merchant.invalid/claim",
        "https://localhost/claim",
        "https://127.0.0.1/claim",
        "https://[::1]/claim",
        "https://merchant.local/claim",
        "https://merchant.invalid:8443/claim",
        "javascript:alert(1)",
        "https://merchant/claim",
    ] {
        assert!(claim_url(bad).is_none(), "{bad}");
    }
}

#[test]
fn refreshed_order_must_still_authorize_the_confirmed_payment() {
    let mut confirmed = GiftInvoice::parse(INVOICE, dashcore::Network::Mainnet).unwrap();
    let check = |status: &str, uri: Option<&str>, expected: &GiftInvoice| {
        let order: OrderData = serde_json::from_value(json!({
            "status":status, "orderId":"fixture", "payTo":uri
        }))
        .unwrap();
        order.verify_quote(expected).is_ok()
    };
    assert!(check("Payment pending", Some(INVOICE), &confirmed));
    assert!(!check("Paid", Some(INVOICE), &confirmed));
    assert!(!check("Cancelled", Some(INVOICE), &confirmed));
    assert!(!check("Payment pending", None, &confirmed));
    confirmed.duffs += 1;
    assert!(!check("Payment pending", Some(INVOICE), &confirmed));
    confirmed.duffs -= 1;
    confirmed.address = "previous-address".into();
    assert!(!check("Payment pending", Some(INVOICE), &confirmed));
}

#[tokio::test]
async fn reopened_order_cannot_relay_when_the_provider_has_cancelled_it() {
    use crate::api::gift_cards::{
        GiftOrder,
        store::{Order, Orders},
    };
    let dir = tempfile::tempdir().unwrap();
    {
        let mut orders = Orders::open(dir.path()).unwrap();
        orders.entries.push(Order {
            view: GiftOrder {
                id: "fixture".into(),
                name: "Fixture card".into(),
                cents: 2500,
                invoice: Some(GiftInvoice::parse(INVOICE, dashcore::Network::Mainnet).unwrap()),
                expires_at: crate::api::gift_cards::now() + 120,
                txid: None,
            },
            account: "account".into(),
            wallet: "wallet".into(),
        });
        orders.save().unwrap();
    }
    let (provider, server) = mock_piggy(vec![get(
        "GET /dash/v1/orders/fixture HTTP/1.1",
        r#"{"code":200,"data":{"orderId":"fixture","status":"Cancelled","payTo":"dash:Xs6DNcUzKPYSvDDxpwFdDpGk4kNqhnUQ1T?amount=0.12345678"}}"#,
    )]);
    let mut orders = Orders::open(dir.path()).unwrap();
    let order = orders.find("fixture", "account", "wallet").unwrap();
    assert!(
        order
            .view
            .revalidate(&provider, "fixture-token")
            .await
            .is_err()
    );
    server.join().unwrap();
}

#[test]
fn a_quote_window_runs_on_the_monotonic_clock_and_ends_at_restart() {
    use crate::api::gift_cards::GiftCards;
    let cards = GiftCards::default();
    let key = [1; 32];
    assert!(cards.open_quote(&key).is_err(), "no quote since this process started");
    cards.start_quote(key);
    let deadline = cards.open_quote(&key).unwrap();
    let window = deadline - tokio::time::Instant::now();
    assert!(window <= Duration::from_secs(120) && window > Duration::from_secs(110));
    cards.quotes().insert(key, tokio::time::Instant::now());
    assert!(cards.open_quote(&key).is_err(), "a passed deadline stays passed");
    assert!(GiftCards::default().open_quote(&key).is_err(), "a restart forgets the quote");
}
