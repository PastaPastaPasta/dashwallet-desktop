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
//!   renderer as a bare [`LinkRoute::Invitation`]. The shell keeps nothing:
//!   the link is dropped once routed, and DP5-01 adds its intake and stash.
//!   Zeroing is best effort: the copies `classify_link` makes and the
//!   forwarded command line (D-Bus, the plugin's buffers) are not zeroed,
//!   and a link typed into File ▸ Open URI comes from the renderer.
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

/// Undoes the `/` the Windows shell adds to a URL-protocol link whose
/// authority has no path. Launched through `ShellExecute` (a click, `start`,
/// `url.dll`), `dashpay://user?id=…` reaches the app as
/// `dashpay://user/?id=…` and `dashwallet://scanqr` as `dashwallet://scanqr/`
/// (electron/electron#10786). The `dashpay:` and `dashwallet:` links the app
/// reads never have a path, so a lone `/` there is dropped. `dash:` and
/// `pay:` links are opaque (no `//`) and never get one (D24).
pub fn without_shell_slash(link: String) -> String {
    let text = link.trim();
    let Some(scheme @ ("dashpay" | "dashwallet")) = link_scheme(text) else {
        return link;
    };
    let Some(after) = text[scheme.len() + 1..].strip_prefix("//") else {
        return link;
    };
    let Some(end) = after.find(['/', '?', '#']) else {
        return link;
    };
    match after[end..].strip_prefix('/') {
        Some(rest) if rest.is_empty() || rest.starts_with(['?', '#']) => {
            format!("{}//{}{rest}", &text[..=scheme.len()], &after[..end])
        }
        _ => link,
    }
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
                Queued::TooLong => Ok(LinkRoute::rejected(LinkRejected::TooLong)),
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
    /// A DashPay invitation. The link, a bearer secret, does not cross IPC;
    /// the shell drops it (zeroed), and DP5-01 adds its intake.
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

impl LinkRoute {
    fn rejected(reason: LinkRejected) -> Self {
        LinkRoute::Rejected { reason }
    }
}

/// Routes one link (Open URI, or a queued one) for `network`.
pub fn route(network: &str, text: &str) -> Result<LinkRoute, AppError> {
    if text.len() > MAX_LINK_BYTES {
        return Ok(LinkRoute::rejected(LinkRejected::TooLong));
    }
    let text = text.trim();
    if let Some("dash" | "pay") = link_scheme(text) {
        let rest = text.split_once(':').map_or("", |(_, rest)| rest);
        let username = dw_uri::ext::parse_payment_uri(text)
            .and_then(|r| r.dashpay_username)
            .filter(|u| !u.trim().is_empty());
        return pay(network, &Zeroizing::new(format!("dash:{rest}")), username);
    }
    Ok(match classify_deep_link(text) {
        DeepLink::Payment(req) => return payment_link(network, req),
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
        DeepLink::Unknown => LinkRoute::rejected(LinkRejected::NotALink),
    })
}

/// `dashwallet://pay=…` (or, from Open URI, an https payment string):
/// rebuilt as a `dash:` URI and checked as dash-qt reads it.
fn payment_link(network: &str, req: dw_uri::ext::PaymentRequest) -> Result<LinkRoute, AppError> {
    let payment = |reason| Ok(LinkRoute::rejected(LinkRejected::Payment { reason }));
    let Some(address) = req.address else {
        return payment(UriRejected::Unparsable);
    };
    // The username travels beside the URI, not in it.
    match uri::build_payment_uri(address, req.amount, req.label, req.message, None) {
        Ok(dash_uri) => pay(network, &dash_uri, req.dashpay_username),
        Err(_) => payment(UriRejected::InvalidAmount),
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
        UriParse::Rejected { reason } => LinkRoute::rejected(LinkRejected::Payment { reason }),
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
    fn the_windows_shell_slash_is_undone_for_hierarchical_links() {
        let fix = |s: &str| without_shell_slash(s.to_owned());
        let id = "US517G5965aydkZ46HS38QLi7UQiSojurfbQfKCELFx";
        let user = format!("dashpay://user?id={id}&username=bob");
        assert_eq!(fix(&format!("dashpay://user/?id={id}&username=bob")), user);
        assert_eq!(fix(&user), user);
        assert_eq!(fix("dashwallet://scanqr/"), "dashwallet://scanqr");
        assert_eq!(fix("DashPay://user/#x"), "DashPay://user#x");
        // A real path, and the opaque payment links, stay as they are.
        assert_eq!(fix("dashwallet://x/scanqr"), "dashwallet://x/scanqr");
        assert_eq!(fix("dashpay://user//?id=1"), "dashpay://user//?id=1");
        assert_eq!(fix("pay:yAddr?label=a/"), "pay:yAddr?label=a/");
        assert_eq!(fix("dash:yAddr/"), "dash:yAddr/");
        assert_eq!(fix("dashpay:user"), "dashpay:user");
        // As the router reads it: the slashed link is not a link at all.
        let r = |s: &str| route("mainnet", s).unwrap();
        assert!(matches!(
            r(&format!("dashpay://user/?id={id}&username=bob")),
            LinkRoute::Rejected { .. }
        ));
        assert!(matches!(
            r(&fix(&format!("dashpay://user/?id={id}&username=bob"))),
            LinkRoute::AddContact { .. }
        ));
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
