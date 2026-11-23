//! H-06 performance probes of the contacts read (docs/research/h-06-performance.md):
//! `DashPay::contacts` at 500 and 1,000 contacts, with and without a search,
//! and the cold load of a wallet whose DashPay overlay holds 500 contacts.
//! Ignored; Linux only. The crate's unit tests need debug assertions, so
//! they run in the dev profile, optimised:
//!
//! ```sh
//! CARGO_PROFILE_DEV_OPT_LEVEL=3 CARGO_PROFILE_TEST_OPT_LEVEL=3 \
//!   cargo test -p dw-engine --lib contacts_perf_tests -- --ignored --nocapture --test-threads=1
//! ```

use std::collections::BTreeMap;

use dpp::prelude::Identifier;
use platform_wallet::changeset::{
    ContactChangeSet, ContactRequestEntry, PlatformWalletChangeSet, ReceivedContactRequestKey,
    SentContactRequestKey,
};
use platform_wallet::wallet::identity::{
    ContactProfileEntry, DashPayProfile, PaymentDirection, PaymentEntry, PaymentStatus,
};
use platform_wallet::{ContactRequest, EstablishedContact};

use super::contacts::{ContactQuery, ContactSection, ContactSort, ContactsPage};
use super::contacts_tests::{Fixture, ME, b58, id};
use super::journal_tests::tx_record;
use crate::{DashNetwork, SessionOptions};
use dw_testutil::perf::{Stats, iters, measure, ms, sample};

/// Payments per established contact.
const PAYMENTS: u32 = 3;

fn contact(n: u32) -> Identifier {
    let mut bytes = [0x5A; 32];
    bytes[..4].copy_from_slice(&n.to_be_bytes());
    Identifier::from(bytes)
}

fn request(sender: Identifier, recipient: Identifier, created_at: u64) -> ContactRequest {
    ContactRequest::new(sender, recipient, 0, 0, 0, vec![1; 96], 1, created_at)
}

/// How contact `n` of `total` relates to us: one in ten an incoming
/// request, one in ten an outgoing one, the rest established.
#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Established,
    Incoming,
    Outgoing,
}

fn kind(n: u32) -> Kind {
    match n % 10 {
        0 => Kind::Incoming,
        1 => Kind::Outgoing,
        _ => Kind::Established,
    }
}

/// The payment txid number of contact `n`'s `k`-th payment.
fn payment_tx(n: u32, k: u32) -> u32 {
    1_000_000 + n * PAYMENTS + k
}

/// `total` contacts as the library stores them, with their payments.
fn overlay(total: u32) -> PlatformWalletChangeSet {
    let me = id(ME);
    let mut cs = ContactChangeSet::default();
    let mut payments = BTreeMap::new();
    for n in 0..total {
        let c = contact(n);
        let at = 1_700_000_000_000 + u64::from(n) * 60_000;
        match kind(n) {
            Kind::Incoming => {
                let key = ReceivedContactRequestKey {
                    owner_id: me,
                    sender_id: c,
                };
                let request = request(c, me, at);
                cs.incoming_requests
                    .insert(key, ContactRequestEntry { request });
            }
            Kind::Outgoing => {
                let key = SentContactRequestKey {
                    owner_id: me,
                    recipient_id: c,
                };
                let request = request(me, c, at);
                cs.sent_requests
                    .insert(key, ContactRequestEntry { request });
            }
            Kind::Established => {
                let key = SentContactRequestKey {
                    owner_id: me,
                    recipient_id: c,
                };
                let pair = EstablishedContact::new(c, request(me, c, at), request(c, me, at + 1));
                cs.established.insert(key, pair);
                for k in 0..PAYMENTS {
                    let txid = tx_record(payment_tx(n, k), None).txid;
                    payments.insert(
                        txid.to_string(),
                        PaymentEntry {
                            counterparty_id: c,
                            amount_duffs: 1_000,
                            memo: None,
                            direction: PaymentDirection::Sent,
                            status: PaymentStatus::Confirmed,
                        },
                    );
                }
            }
        }
    }
    PlatformWalletChangeSet {
        contacts: Some(cs),
        dashpay_payments_overlay: Some(BTreeMap::from([(me, payments)])),
        ..Default::default()
    }
}

/// The contacts' cached profiles and their payments' transactions, in
/// memory (the library keeps contact profiles in its identity snapshot,
/// the history store keeps the payments' blocks).
fn warm(f: &Fixture, total: u32) {
    f.with_me(move |m, _| {
        let profiles = m.dashpay_contact_profiles_mut();
        for n in 0..total {
            let profile = DashPayProfile {
                display_name: Some(format!("Contact {n:04}")),
                ..Default::default()
            };
            profiles.insert(
                contact(n),
                ContactProfileEntry {
                    profile: Some(profile),
                    checked_at_ms: 0,
                },
            );
        }
    });
    f.s.hub.history.with_wallet(f.wallet, |h| {
        for n in (0..total).filter(|n| kind(*n) == Kind::Established) {
            for k in 0..PAYMENTS {
                let record = tx_record(payment_tx(n, k), Some(1_700_000_000 + n * 60 + k));
                h.upsert(&record, None);
            }
        }
    });
}

fn query(sort: ContactSort, text: Option<&str>) -> ContactQuery {
    ContactQuery {
        sections: Vec::new(),
        sort,
        text: text.map(str::to_owned),
    }
}

fn rows(page: &ContactsPage) -> usize {
    page.sections.iter().map(|s| s.contacts.len()).sum()
}

fn reopen(f: &mut Fixture) {
    let engine = &f.engine;
    engine
        .block_on(engine.close_network(DashNetwork::Regtest))
        .unwrap();
    f.s = engine
        .block_on(engine.open_network(
            DashNetwork::Regtest,
            SessionOptions {
                dapi_addresses: vec!["http://127.0.0.1:1".into()],
                quorum_url: Some("http://127.0.0.1:1".into()),
                spv_peers: vec!["127.0.0.1:1".into()],
                no_platform: true,
                ..Default::default()
            },
        ))
        .unwrap();
}

/// The facade's contacts page for each query shape, `iters` times.
fn probe(f: &Fixture, total: u32) -> Vec<(&'static str, Stats)> {
    let dp = f.dp();
    let n = iters(100);
    let shapes: [(&'static str, ContactQuery); 4] = [
        (
            "all sections, display name",
            query(ContactSort::DisplayName, None),
        ),
        (
            "all sections, last activity",
            query(ContactSort::LastActivity, None),
        ),
        (
            "search \"contact 01\"",
            query(ContactSort::DisplayName, Some("contact 01")),
        ),
        (
            "search, no hit",
            query(ContactSort::DisplayName, Some("zzzz")),
        ),
    ];
    let all = dp.contacts(b58(ME), shapes[0].1.clone()).unwrap();
    assert_eq!(rows(&all) as u32, total, "every contact is listed");
    let contacts = (all.sections.iter())
        .find(|s| s.section == ContactSection::Contacts)
        .unwrap();
    assert!(
        contacts.contacts[0].display_name.is_some()
            && contacts.contacts[0].last_activity_at.is_some()
    );
    shapes
        .into_iter()
        .map(|(name, q)| {
            let s = measure(5, n, || {
                dp.contacts(b58(ME), q.clone()).unwrap();
            });
            println!("H06 contacts {total} {name}: {s}");
            (name, s)
        })
        .collect()
}

/// `DashPay::contacts` at 500 and 1,000 contacts: under 100 ms at 1k
/// (ROADMAP H-06), asserted on the median's CPU time with headroom.
#[test]
#[ignore = "H-06 probe: run in release"]
fn h06_contacts_query_at_500_and_1000() {
    for total in [500, 1_000] {
        let mut f = Fixture::new(true);
        f.give_me();
        f.store(overlay(total));
        reopen(&mut f);
        warm(&f, total);
        for (name, s) in probe(&f, total) {
            assert!(s.cpu_p50 < 100.0, "{total} contacts, {name}: {s}");
        }
    }
}

/// The cold half of a restore with DashPay: the session reopened over a
/// store whose overlay holds 500 contacts and 1,350 payments, then the first
/// contacts read, which builds the read model.
#[test]
#[ignore = "H-06 probe: run in release"]
fn h06_cold_load_of_500_contacts() {
    let total = 500;
    let mut f = Fixture::new(true);
    f.give_me();
    f.store(overlay(total));
    let runs = iters(5).min(10);
    for run in 0..runs {
        let ((), open) = sample(|| reopen(&mut f));
        let dp = f.dp();
        let (page, first) = sample(|| dp.contacts(b58(ME), query(ContactSort::DisplayName, None)));
        assert_eq!(rows(&page.unwrap()) as u32, total);
        println!(
            "H06 cold load {total} contacts run {run}: session open wall {:.1} ms cpu {:.1} ms; \
             first contacts read wall {:.2} ms cpu {:.2} ms",
            ms(open.wall),
            ms(open.cpu),
            ms(first.wall),
            ms(first.cpu),
        );
    }
}
