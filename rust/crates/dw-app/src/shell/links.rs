//! Deep links (DASHPAY §3.8, IOS-048, IOS-125, QT-019): the inbox the OS
//! delivers into and the routes the renderer opens.
//!
//! - **Intake.** A link arrives as a launch argument, in a second launch's
//!   arguments (forwarded by the single-instance plugin) or, on macOS, as an
//!   `Opened` event. [`Inbox::deliver`] queues it and the shell tells the
//!   renderer (`linksPending`); the renderer takes the queue
//!   (`take_links`) when it is ready, so a link that arrives before the page
//!   listens waits instead of being lost.
//! - **Bounded.** At most [`MAX_QUEUED`] links of at most [`MAX_LINK_BYTES`]
//!   each wait; a longer one is queued as a [`LinkRejected::TooLong`]
//!   marker, and further links are dropped until the renderer takes the
//!   queue.
//! - **Secret-safe.** An invitation link is a bearer secret (DASHPAY §3.8):
//!   queued links are zeroed when dropped, their text is never logged (the
//!   self-test trace records the scheme only), and an invitation reaches the
//!   renderer as a bare [`LinkRoute::Invitation`]. DP5-01 owns its intake
//!   and stash.
//! - **Payment links are Core's.** `dash:` and `pay:` go through dash-qt's
//!   URI handler (`api::send::uri`) for the window's network, with `pay:`
//!   read as `dash:` and the iOS `user=` key kept as the DashPay username.
//!   `dashwallet://pay=…` is rebuilt as a `dash:` URI and checked the same
//!   way.

use std::collections::VecDeque;
use std::sync::Mutex;

use dw_uri::deeplink::{DeepLink, classify_link as classify_deep_link};
use serde::{Deserialize, Serialize};
use ts_rs::TS;
use zeroize::Zeroizing;

use crate::api::send::uri::{self, PaymentUri, UriParse, UriRejected};
use crate::error::AppError;

/// The schemes the app registers with the OS (`tauri.conf.json`
/// `plugins.deep-link.desktop.schemes`).
pub const SCHEMES: [&str; 4] = ["dash", "pay", "dashwallet", "dashpay"];

/// Links waiting for the renderer at most.
pub const MAX_QUEUED: usize = 16;

/// The longest link queued as text. A BIP21 URI with a label and a message
/// is a few hundred bytes; dash-qt's QR limit is 255 characters.
pub const MAX_LINK_BYTES: usize = 4096;

/// The scheme of `arg` (lower-cased) when it is one the app registers.
pub fn link_scheme(arg: &str) -> Option<&'static str> {
    let (scheme, _) = arg.trim().split_once(':')?;
    SCHEMES.into_iter().find(|s| s.eq_ignore_ascii_case(scheme))
}

/// Where a link came from, for the self-test trace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkSource {
    /// An argument of this launch.
    Launch,
    /// An argument of a second launch, forwarded to this instance.
    Forwarded,
    /// macOS `application:openURLs:`.
    Opened,
}

impl LinkSource {
    pub fn as_str(self) -> &'static str {
        match self {
            LinkSource::Launch => "launch",
            LinkSource::Forwarded => "forwarded",
            LinkSource::Opened => "opened",
        }
    }
}

enum Queued {
    Link(Zeroizing<String>),
    TooLong,
}

/// The links waiting for the renderer.
#[derive(Default)]
pub struct Inbox {
    queue: Mutex<VecDeque<Queued>>,
}

impl Inbox {
    /// Queues `link`; `false` when the queue is full and it was dropped.
    pub fn deliver(&self, link: String) -> bool {
        let link = Zeroizing::new(link);
        let mut queue = self.queue.lock().unwrap_or_else(|e| e.into_inner());
        if queue.len() >= MAX_QUEUED {
            return false;
        }
        queue.push_back(if link.len() > MAX_LINK_BYTES {
            Queued::TooLong
        } else {
            Queued::Link(link)
        });
        true
    }

    /// Takes every queued link, routed for `network` (a directory name), in
    /// arrival order.
    pub fn take(&self, network: &str) -> Result<Vec<LinkRoute>, AppError> {
        // Checked first, so a bad name leaves the queue as it was.
        crate::dto::parse_network(network)?;
        let taken: Vec<Queued> = {
            let mut queue = self.queue.lock().unwrap_or_else(|e| e.into_inner());
            queue.drain(..).collect()
        };
        taken
            .iter()
            .map(|q| match q {
                Queued::Link(text) => route(network, text),
                Queued::TooLong => Ok(LinkRoute::Rejected {
                    reason: LinkRejected::TooLong,
                }),
            })
            .collect()
    }

    pub fn len(&self) -> usize {
        self.queue.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Where a link sends the window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum LinkRoute {
    /// Pay ▸ To an address, prefilled. `username` is the DashPay name the
    /// link claims for the payee (iOS `user=`), unverified.
    Pay {
        uri: PaymentUri,
        username: Option<String>,
    },
    /// `dashwallet://scanqr`: Pay ▸ To an address, ready for a QR code.
    ScanQr,
    /// `dashpay://user`: Contacts ▸ Add contact, prefilled. The username
    /// claim is not verified here.
    AddContact {
        identity: String,
        username: String,
    },
    /// A DashPay invitation. The link, a bearer secret, stays in the shell;
    /// DP5-01 takes it over.
    Invitation,
    /// A link the desktop wallet recognises but does not act on.
    Unsupported {
        what: UnsupportedLink,
    },
    Rejected {
        reason: LinkRejected,
    },
}

/// Links the desktop wallet does not act on (D-row "links").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum UnsupportedLink {
    /// `dashwallet://request=address`: the iOS inter-app address hand-off
    /// (IOS-125), which needs a callback scheme the desktop has no use for.
    AddressRequest,
    /// `dash-key:` / `dash-st:` (DashConnect), from Open URI only.
    DashConnect,
    /// An exchange integration callback (Uphold).
    Integration,
    /// A BIP70/BIP72 payment request (`r=` without an address).
    PaymentRequest,
}

/// Why a link was not routed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum LinkRejected {
    /// Not a link the wallet reads.
    NotALink,
    /// Longer than the shell queues.
    TooLong,
    /// A payment link dash-qt's URI handler refused.
    Payment { reason: UriRejected },
}

fn rejected(reason: LinkRejected) -> Result<LinkRoute, AppError> {
    Ok(LinkRoute::Rejected { reason })
}

/// Routes one link (Open URI, or a queued one) for `network`.
pub fn route(network: &str, text: &str) -> Result<LinkRoute, AppError> {
    let text = text.trim();
    match link_scheme(text) {
        Some("dash" | "pay") => {
            let rest = &text[text.find(':').map_or(0, |i| i + 1)..];
            let username = dw_uri::ext::parse_payment_uri(text)
                .and_then(|r| r.dashpay_username)
                .filter(|u| !u.trim().is_empty());
            pay(network, &Zeroizing::new(format!("dash:{rest}")), username)
        }
        _ => Ok(match classify_deep_link(text) {
            DeepLink::Payment(req) => {
                // `dashwallet://pay=…` (or, from Open URI, an https payment
                // string): rebuilt and checked as dash-qt reads it.
                let Some(address) = req.address else {
                    return rejected(LinkRejected::Payment {
                        reason: UriRejected::Unparsable,
                    });
                };
                let Ok(dash_uri) =
                    uri::build_payment_uri(address, req.amount, req.label, req.message)
                else {
                    return rejected(LinkRejected::Payment {
                        reason: UriRejected::InvalidAmount,
                    });
                };
                return pay(network, &dash_uri, req.dashpay_username);
            }
            DeepLink::ScanQr => LinkRoute::ScanQr,
            DeepLink::DashPayUser(user) => LinkRoute::AddContact {
                identity: user.identity_base58(),
                username: user.username,
            },
            DeepLink::Invitation(secret) => {
                drop(Zeroizing::new(secret));
                LinkRoute::Invitation
            }
            DeepLink::AddressRequest { .. } => LinkRoute::Unsupported {
                what: UnsupportedLink::AddressRequest,
            },
            DeepLink::DashConnectKey(_) | DeepLink::DashConnectSt(_) => LinkRoute::Unsupported {
                what: UnsupportedLink::DashConnect,
            },
            DeepLink::Integration(_) => LinkRoute::Unsupported {
                what: UnsupportedLink::Integration,
            },
            DeepLink::Unknown => LinkRoute::Rejected {
                reason: LinkRejected::NotALink,
            },
        }),
    }
}

fn pay(network: &str, dash_uri: &str, username: Option<String>) -> Result<LinkRoute, AppError> {
    Ok(match uri::parse_payment_uri(network, dash_uri)? {
        UriParse::Accepted { uri } => LinkRoute::Pay { uri, username },
        UriParse::Rejected {
            reason: UriRejected::Bip70Unsupported,
        } => LinkRoute::Unsupported {
            what: UnsupportedLink::PaymentRequest,
        },
        UriParse::Rejected { reason } => LinkRoute::Rejected {
            reason: LinkRejected::Payment { reason },
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const ADDR: &str = "XwnLY9Tf7Zsef8gMGL2fhWA9ZmMjt4KPwg";

    fn r(text: &str) -> LinkRoute {
        route("mainnet", text).unwrap()
    }

    fn pay_route(address: &str, amount: Option<u64>, label: Option<&str>) -> LinkRoute {
        LinkRoute::Pay {
            uri: PaymentUri {
                address: address.into(),
                amount,
                label: label.map(Into::into),
                message: None,
            },
            username: None,
        }
    }

    #[test]
    fn only_the_registered_schemes_are_links() {
        assert_eq!(link_scheme("dash:X"), Some("dash"));
        assert_eq!(link_scheme("  PAY:X"), Some("pay"));
        assert_eq!(link_scheme("DashPay://user"), Some("dashpay"));
        assert_eq!(link_scheme("dashwallet://scanqr"), Some("dashwallet"));
        for other in [
            "https://x",
            "file.txt",
            "-psn_0_1",
            "--datadir",
            "dash-key:x",
            "",
        ] {
            assert_eq!(link_scheme(other), None, "{other}");
        }
    }

    #[test]
    fn dash_and_pay_links_are_read_as_dash_qt_reads_them() {
        let expect = pay_route(ADDR, Some(150_000_000), Some("a b"));
        assert_eq!(r(&format!("dash:{ADDR}?amount=1.5&label=a%20b")), expect);
        assert_eq!(r(&format!("pay:{ADDR}?amount=1.5&label=a%20b")), expect);
        assert_eq!(r(&format!("PAY:{ADDR}?amount=1.5&label=a%20b")), expect);
        // iOS `user=` names the payee's DashPay account.
        assert_eq!(
            r(&format!("pay:{ADDR}?user=alice")),
            LinkRoute::Pay {
                uri: PaymentUri {
                    address: ADDR.into(),
                    amount: None,
                    label: None,
                    message: None,
                },
                username: Some("alice".into()),
            }
        );
        let payment = |reason| LinkRoute::Rejected {
            reason: LinkRejected::Payment { reason },
        };
        assert_eq!(
            r(&format!("dash://{ADDR}")),
            payment(UriRejected::DoubleSlash)
        );
        assert_eq!(
            r(&format!("pay://{ADDR}")),
            payment(UriRejected::DoubleSlash)
        );
        assert_eq!(
            r(&format!("dash:{ADDR}?amount=-1")),
            payment(UriRejected::InvalidAmount)
        );
        assert_eq!(
            r(&format!("dash:{ADDR}?req-flux=1")),
            payment(UriRejected::Unparsable)
        );
        // The window's network decides the address.
        assert!(matches!(
            route("testnet", &format!("dash:{ADDR}")).unwrap(),
            LinkRoute::Rejected {
                reason: LinkRejected::Payment {
                    reason: UriRejected::InvalidAddress { .. }
                }
            }
        ));
        assert_eq!(
            r("dash:?r=https://merchant.example/pr"),
            LinkRoute::Unsupported {
                what: UnsupportedLink::PaymentRequest
            }
        );
    }

    #[test]
    fn dashwallet_links_follow_ios() {
        assert_eq!(r("dashwallet://scanqr"), LinkRoute::ScanQr);
        assert_eq!(
            r(&format!("dashwallet://pay={ADDR}&sender=shop")),
            pay_route(
                ADDR,
                None,
                Some("Application Shop is requesting a payment to")
            )
        );
        assert_eq!(
            r("dashwallet://request=address&sender=shop"),
            LinkRoute::Unsupported {
                what: UnsupportedLink::AddressRequest
            }
        );
        assert_eq!(
            r("dashwallet://brokers/coinbase/connect"),
            LinkRoute::Rejected {
                reason: LinkRejected::NotALink
            }
        );
    }

    #[test]
    fn dashpay_links_route_without_their_secrets() {
        let user = dw_uri::deeplink::DashPayUserLink {
            identity_id: [7; 32],
            username: "bob".into(),
        };
        let id = user.identity_base58();
        assert_eq!(
            r(&format!("dashpay://user?id={id}&username=bob.dash")),
            LinkRoute::AddContact {
                identity: id.clone(),
                username: "bob".into(),
            }
        );
        assert_eq!(
            r("dashpay://user?id=notbase58&username=bob"),
            LinkRoute::Rejected {
                reason: LinkRejected::NotALink
            }
        );
        for invite in [
            "dashpay://invite?du=alice&assetlocktx=00&pk=secret&islock=00",
            "https://invitations.dashpay.io/applink?du=alice&pk=secret",
        ] {
            let json = serde_json::to_string(&r(invite)).unwrap();
            assert_eq!(json, r#"{"kind":"invitation"}"#);
        }
    }

    #[test]
    fn the_inbox_is_bounded_and_keeps_order() {
        let inbox = Inbox::default();
        assert!(inbox.deliver("dashwallet://scanqr".into()));
        assert!(inbox.deliver(format!("dash:{ADDR}?{}", "x".repeat(MAX_LINK_BYTES))));
        assert!(inbox.deliver("nonsense".into()));
        for _ in 3..MAX_QUEUED {
            assert!(inbox.deliver("dashwallet://scanqr".into()));
        }
        assert!(!inbox.deliver("dashwallet://scanqr".into()), "full");
        assert_eq!(inbox.len(), MAX_QUEUED);
        // A bad network leaves the queue as it was.
        assert!(inbox.take("moon").is_err());
        assert_eq!(inbox.len(), MAX_QUEUED);
        let routes = inbox.take("mainnet").unwrap();
        assert_eq!(routes.len(), MAX_QUEUED);
        assert_eq!(routes[0], LinkRoute::ScanQr);
        assert_eq!(
            routes[1],
            LinkRoute::Rejected {
                reason: LinkRejected::TooLong
            }
        );
        assert_eq!(
            routes[2],
            LinkRoute::Rejected {
                reason: LinkRejected::NotALink
            }
        );
        assert!(inbox.is_empty());
        assert!(inbox.deliver("dashwallet://scanqr".into()), "room again");
    }

    #[test]
    fn routes_serialise_with_a_kind() {
        assert_eq!(
            serde_json::to_value(r("dashwallet://request=address&sender=x")).unwrap(),
            serde_json::json!({"kind": "unsupported", "what": "addressRequest"})
        );
        assert_eq!(
            serde_json::to_value(r(&format!("dash:{ADDR}?amount=1"))).unwrap(),
            serde_json::json!({
                "kind": "pay",
                "uri": {"address": ADDR, "amount": 100_000_000, "label": null, "message": null},
                "username": null,
            })
        );
    }
}
