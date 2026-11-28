use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::thread;

use std::sync::Arc;

use dw_engine::{NetworkSession, WalletId};
use dw_vault::{MemoryOsStore, OsSecretStore};

use super::*;
use crate::api::gift_cards::store::{Account, Change, Order, WalletStore};
use crate::api::gift_cards::{Context, GiftAccount, GiftCards, GiftInvoice, GiftOrder};
use crate::ipc_tests::Demo;

/// A regtest session holding one wallet with a seed; the gift card code
/// under test does not check the network.
fn wallet_session() -> (Demo, Arc<NetworkSession>, WalletId) {
    let demo = Demo::new();
    demo.open();
    demo.ok("create_vault", json!({ "passphrase": null }));
    let created = demo.ok("create_wallet", json!({ "wordCount": 12 }));
    let wallet: WalletId = created["walletId"].as_str().unwrap().parse().unwrap();
    let (session, _) = tauri::async_runtime::block_on(demo.state().session_and_epoch()).unwrap();
    (demo, session, wallet)
}

fn context<'a>(
    cards: &'a GiftCards,
    piggy: &'a Piggy,
    session: &Arc<NetworkSession>,
    wallet: WalletId,
) -> Context<'a> {
    Context {
        _guard: cards.serial.try_lock().unwrap(),
        session: Arc::clone(session),
        epoch: 0,
        store: WalletStore::new(Arc::clone(session), wallet),
        cards,
        piggy,
    }
}

fn profile(email: &str) -> GiftSignup {
    GiftSignup {
        first_name: "Fixture".into(),
        last_name: "Buyer".into(),
        email: email.into(),
        state: "CA".into(),
    }
}

fn fixture_order(id: &str, account: &str, created_at: u64) -> Order {
    Order {
        view: GiftOrder {
            id: id.into(),
            name: "Fixture card".into(),
            cents: 2500,
            invoice: Some(GiftInvoice::parse(INVOICE, dashcore::Network::Mainnet).unwrap()),
            expires_at: 0,
            txid: None,
        },
        account: account.into(),
        created_at,
    }
}

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

#[test]
fn otp_sign_in_catalog_invoice_and_claim_follow_the_provider_contract() {
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

    let (_demo, session, wallet) = wallet_session();
    let cards = GiftCards::default();
    tauri::async_runtime::block_on(async {
        let ctx = context(&cards, &piggy, &session, wallet);
        assert!(matches!(
            ctx.signup(profile("fixture@example.invalid")).await.unwrap(),
            GiftAccount::CodeSent { .. }
        ));
        assert!(matches!(
            ctx.verify_email(Zeroizing::new("123456".into())).await.unwrap(),
            GiftAccount::SignedIn { .. }
        ));
        let (account, access) = ctx.active_session().await.unwrap();
        let brands = piggy.brands(&access).await.unwrap();
        let brand = &brands[0];
        assert_eq!(brand.name, "Fixture Groceries");
        let products = piggy.products(&brand.id, &access).await.unwrap();
        assert_eq!(products[0].values, [2500, 5050]);
        let order = piggy
            .create_order(&access, products[0].id, 2500, &account.signup)
            .await
            .unwrap();
        assert!(order.pay_to.is_none());
        let unpaid = piggy.order(&order.id, &access).await.unwrap();
        assert_eq!(unpaid.pay_to.as_deref(), Some(INVOICE));
        assert_eq!(
            GiftInvoice::parse(INVOICE, dashcore::Network::Mainnet)
                .unwrap()
                .duffs,
            12_345_678
        );
        let fulfilled = piggy.order(&order.id, &access).await.unwrap();
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
            ctx.sign_out().await.unwrap(),
            GiftAccount::CanSignIn { .. }
        ));
        drop(ctx);
        // A restart keeps no token, only the sealed account and its password.
        let restarted = GiftCards::default();
        let ctx = context(&restarted, &piggy, &session, wallet);
        assert!(matches!(
            ctx.account_status().await.unwrap(),
            GiftAccount::CanSignIn { .. }
        ));
        assert!(matches!(
            ctx.sign_in(None).await.unwrap(),
            GiftAccount::SignedIn { .. }
        ));
    });
    let rows = tauri::async_runtime::block_on(session.gift_card_rows(wallet)).unwrap();
    assert!(rows.iter().all(|(_, value)| !value.contains("fixture-password")
        && !value.contains("fixture@example.invalid")));
    assert!(!session.data_dir().join("gift-card-orders.json").exists());
    server.join().unwrap();
}

/// Signing up with another account keeps the earlier one, and its orders
/// stay listed and reachable: every account of the wallet is stored.
#[test]
fn another_account_keeps_the_earlier_account_and_its_orders() {
    let (_demo, session, wallet) = wallet_session();
    let (piggy, server) = mock_piggy(vec![Mock {
        request_line: "POST /dash/v1/signup HTTP/1.1",
        authorization: None,
        body: Some(
            json!({"firstName":"Fixture","lastName":"Buyer","email":"second@example.invalid","country":"US","state":"CA"}),
        ),
        response: r#"{"userId":"second-user"}"#,
    }]);
    let cards = GiftCards::default();
    tauri::async_runtime::block_on(async {
        let ctx = context(&cards, &piggy, &session, wallet);
        let first = Account {
            signup: profile("first@example.invalid"),
            user_id: "first-user".into(),
            password: Some(Zeroizing::new("first-password".into())),
        };
        let old = fixture_order("first-order", &first.key(), 10);
        ctx.store
            .write(&[
                Change::Account(&first),
                Change::Active(Some(first.key())),
                Change::Order(&old),
            ])
            .await
            .unwrap();
        assert!(matches!(
            ctx.signup(profile("second@example.invalid")).await.unwrap(),
            GiftAccount::CodeSent { .. }
        ));
        let second = Account {
            signup: profile("second@example.invalid"),
            user_id: "second-user".into(),
            password: Some(Zeroizing::new("second-password".into())),
        };
        ctx.store
            .write(&[
                Change::Account(&second),
                Change::Pending(None),
                Change::Active(Some(second.key())),
                Change::Order(&fixture_order("second-order", &second.key(), 20)),
            ])
            .await
            .unwrap();
        let accounts = ctx.stored_accounts().await.unwrap();
        assert_eq!(accounts.len(), 2);
        assert!(accounts.iter().any(|a| a.email == "first@example.invalid" && !a.active));
        assert!(accounts.iter().any(|a| a.email == "second@example.invalid" && a.active));
        let book = ctx.store.read().await.unwrap();
        let ids: Vec<_> = book.orders.iter().map(|o| o.view.id.as_str()).collect();
        assert_eq!(ids, ["second-order", "first-order"]);
        let earlier = book.order("first-order").unwrap();
        assert_eq!(
            book.account(&earlier.account).unwrap().signup.email,
            "first@example.invalid"
        );
    });
    server.join().unwrap();
}

#[cfg(unix)]
/// Earlier builds' account (OS secure store) and order journal move into
/// the wallet that made the orders, once, and the old copies are deleted.
#[test]
fn earlier_builds_account_and_orders_move_into_the_wallet_once() {
    let (_demo, session, wallet) = wallet_session();
    let root = tempfile::tempdir().unwrap();
    let os: Arc<dyn OsSecretStore> = Arc::new(MemoryOsStore::default());
    let account = Account {
        signup: profile("legacy@example.invalid"),
        user_id: "legacy-user".into(),
        password: Some(Zeroizing::new("legacy-password".into())),
    };
    let key = account.key();
    let journal = json!([
        { "view": fixture_order("legacy-order", &key, 0).view, "account": key, "wallet": wallet.to_string() },
        { "view": fixture_order("gone-order", &key, 0).view, "account": key, "wallet": "11".repeat(32) },
    ]);
    let path = session.data_dir().join("gift-card-orders.json");
    std::fs::write(&path, serde_json::to_vec(&journal).unwrap()).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let piggy = Piggy::at(Url::parse("http://127.0.0.1:1/dash/v1/").unwrap()).unwrap();
    tauri::async_runtime::block_on(async {
        let legacy = super::super::legacy::Secrets::new(root.path(), Arc::clone(&os)).unwrap();
        legacy.save_for_test("piggy-account", &account).await;
        legacy.save_for_test("piggy-access", &json!({"access":"old","expires_at":1})).await;
        legacy.save_for_test("piggy-pending", &json!({"user_id":"pending-user"})).await;
        assert!(
            super::super::legacy::migrate(root.path(), &os, &session)
                .await
                .unwrap()
        );
        let cards = GiftCards::default();
        let ctx = context(&cards, &piggy, &session, wallet);
        let book = ctx.store.read().await.unwrap();
        assert_eq!(book.accounts.len(), 1);
        assert_eq!(book.active.as_deref(), Some(key.as_str()));
        assert!(book.pending.is_none(), "a pending signup has no wallet to go to");
        let ids: Vec<_> = book.orders.iter().map(|o| o.view.id.as_str()).collect();
        assert_eq!(ids, ["legacy-order"]);
        assert!(legacy.is_empty_for_test().await);
        assert!(!path.exists());
        assert!(
            super::super::legacy::migrate(root.path(), &os, &session)
                .await
                .unwrap()
        );
        assert_eq!(ctx.store.read().await.unwrap().orders.len(), 1);
    });
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
    let order = fixture_order("fixture", "account", 0);
    let (provider, server) = mock_piggy(vec![get(
        "GET /dash/v1/orders/fixture HTTP/1.1",
        r#"{"code":200,"data":{"orderId":"fixture","status":"Cancelled","payTo":"dash:Xs6DNcUzKPYSvDDxpwFdDpGk4kNqhnUQ1T?amount=0.12345678"}}"#,
    )]);
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
