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
//!   self-test trace records the scheme only), and an invitation goes into
//!   the engine's link stash at intake ([`intake`], DP5-01): the renderer
//!   gets [`LinkRoute::Invitation`] with the stash id only. Zeroing is best
//!   effort: the copies `classify_link` makes and the forwarded command line
//!   (D-Bus, the plugin's buffers) are not zeroed, and a link typed into
//!   File ▸ Open URI comes from the renderer.
//! - **Kept for after onboarding** (DP5-01 D2). While the window's network
//!   has no wallet, a payment or contact link goes into the stash too and
//!   comes back as [`LinkRoute::Saved`]; the renderer offers it once a
//!   wallet exists (`stashed_links`, `open_stashed_link`).
//! - **Payment links are Core's.** `dash:` and `pay:` go through dash-qt's
//!   URI handler (`api::send::uri`) for the window's network, with `pay:`
//!   read as `dash:` and the iOS `user=` key kept as the DashPay username.
//!   `dashwallet://pay=…` is rebuilt as a `dash:` URI and checked the same
//!   way.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use dw_engine::NetworkSession;
use dw_engine::platform::{BearerSecret, StashedLink, StashedLinkKind};
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

    /// Takes every queued link, in arrival order, for `network` (a
    /// directory name, checked first so a bad name leaves the queue as it
    /// was): its text, or `None` for one longer than [`MAX_LINK_BYTES`].
    pub fn drain(&self, network: &str) -> Result<Vec<Option<Zeroizing<String>>>, AppError> {
        crate::dto::parse_network(network)?;
        let mut queue = self.queue.lock().unwrap_or_else(|e| e.into_inner());
        Ok(queue
            .drain(..)
            .map(|q| match q {
                Queued::Link(text) => Some(text),
                Queued::TooLong => None,
            })
            .collect())
    }

    /// Takes every queued link, routed for `network`, as
    /// [`Inbox::drain`] and [`intake`] do: `session` is the window's.
    pub async fn take(
        &self,
        session: Option<Arc<NetworkSession>>,
        network: &str,
    ) -> Result<Vec<LinkRoute>, AppError> {
        let mut routes = Vec::new();
        for link in self.drain(network)? {
            routes.push(match link {
                Some(text) => intake(session.clone(), network, &text).await?,
                None => LinkRoute::rejected(LinkRejected::TooLong),
            });
        }
        Ok(routes)
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
    /// A DashPay invitation, kept in the engine's link stash under
    /// `link_id`. The link, a bearer secret, does not cross IPC.
    Invitation {
        link_id: String,
    },
    /// A payment or contact link that arrived while the network has no
    /// wallet: kept in the link stash and offered once a wallet exists.
    Saved {
        link: StashedLinkInfo,
    },
    /// A link the desktop wallet recognises but does not act on.
    Unsupported {
        what: UnsupportedLink,
    },
    Rejected {
        reason: LinkRejected,
    },
}

impl LinkRoute {
    /// The route's `kind`, as the JSON tags it: for logs.
    pub fn kind_name(&self) -> &'static str {
        match self {
            LinkRoute::Pay { .. } => "pay",
            LinkRoute::ScanQr => "scanQr",
            LinkRoute::AddContact { .. } => "addContact",
            LinkRoute::Invitation { .. } => "invitation",
            LinkRoute::Saved { .. } => "saved",
            LinkRoute::Unsupported { .. } => "unsupported",
            LinkRoute::Rejected { .. } => "rejected",
        }
    }
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
    /// An invitation that could not be kept (no network open, or the stash
    /// failed): open it again.
    NotSaved,
}

/// A link in the stash, without its text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct StashedLinkInfo {
    pub id: String,
    pub kind: StashedLinkKindName,
    /// UNIX seconds.
    #[ts(type = "number")]
    pub stashed_at: u64,
    /// UNIX seconds: the link is dropped from then on.
    #[ts(type = "number")]
    pub expires_at: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum StashedLinkKindName {
    Payment,
    Contact,
    Invitation,
}

impl From<StashedLink> for StashedLinkInfo {
    fn from(l: StashedLink) -> Self {
        StashedLinkInfo {
            id: l.id,
            kind: match l.kind {
                StashedLinkKind::Payment => StashedLinkKindName::Payment,
                StashedLinkKind::Contact => StashedLinkKindName::Contact,
                StashedLinkKind::Invitation => StashedLinkKindName::Invitation,
            },
            stashed_at: l.stashed_at,
            expires_at: l.expires_at,
        }
    }
}

impl LinkRoute {
    fn rejected(reason: LinkRejected) -> Self {
        LinkRoute::Rejected { reason }
    }
}

/// A link routed before intake: an invitation still needs its stash id.
#[derive(Debug, PartialEq)]
pub(crate) enum Routed {
    Route(LinkRoute),
    Invitation,
}

/// Takes one link in (DP5-01 D2, D3): routes it for `network`, then keeps
/// it in the engine's link stash when it must wait. An invitation always
/// waits there, and its route names the stash id; without the window's
/// session it is [`LinkRejected::NotSaved`]. A payment or contact link
/// waits while the session has no wallet ([`LinkRoute::Saved`]); otherwise,
/// or when keeping it fails, it routes as before. `session` is the
/// window's; one of another network counts as none.
pub async fn intake(
    session: Option<Arc<NetworkSession>>,
    network: &str,
    text: &str,
) -> Result<LinkRoute, AppError> {
    let routed = route_text(network, text)?;
    let session = session.filter(|s| s.network().dir_name() == network);
    let stash = |s: Arc<NetworkSession>| async move {
        s.stash_link(BearerSecret::new(text.trim().to_owned()))
            .await
    };
    let route = match (routed, session) {
        (Routed::Invitation, None) => LinkRoute::rejected(LinkRejected::NotSaved),
        (Routed::Invitation, Some(s)) => match stash(s).await {
            Ok(link) => LinkRoute::Invitation { link_id: link.id },
            Err(e) => {
                tracing::warn!(code = e.code(), "an invitation link could not be kept");
                LinkRoute::rejected(LinkRejected::NotSaved)
            }
        },
        (Routed::Route(r @ (LinkRoute::Pay { .. } | LinkRoute::AddContact { .. })), Some(s))
            if s.wallet_infos().is_ok_and(|w| w.is_empty()) =>
        {
            match stash(s).await {
                Ok(link) => LinkRoute::Saved { link: link.into() },
                Err(e) => {
                    tracing::warn!(
                        code = e.code(),
                        "a link could not be kept for after onboarding"
                    );
                    r
                }
            }
        }
        (Routed::Route(r), _) => r,
    };
    // The route's kind only: never the link or what it carries (D14).
    tracing::debug!(route = route.kind_name(), "link taken in");
    Ok(route)
}

/// Routes one link for `network` without intake: an invitation is
/// [`LinkRejected::NotSaved`] here (tests, and the stash replay of a link
/// that is not an invitation).
pub fn route(network: &str, text: &str) -> Result<LinkRoute, AppError> {
    Ok(match route_text(network, text)? {
        Routed::Route(r) => r,
        Routed::Invitation => LinkRoute::rejected(LinkRejected::NotSaved),
    })
}

/// Routes one link (Open URI, or a queued one) for `network`.
pub(crate) fn route_text(network: &str, text: &str) -> Result<Routed, AppError> {
    if text.len() > MAX_LINK_BYTES {
        return Ok(Routed::Route(LinkRoute::rejected(LinkRejected::TooLong)));
    }
    let text = text.trim();
    if let Some("dash" | "pay") = link_scheme(text) {
        let rest = text.split_once(':').map_or("", |(_, rest)| rest);
        let username = dw_uri::ext::parse_payment_uri(text)
            .and_then(|r| r.dashpay_username)
            .filter(|u| !u.trim().is_empty());
        return pay(network, &Zeroizing::new(format!("dash:{rest}")), username).map(Routed::Route);
    }
    Ok(Routed::Route(match classify_deep_link(text) {
        DeepLink::Payment(req) => return payment_link(network, req).map(Routed::Route),
        DeepLink::ScanQr => LinkRoute::ScanQr,
        DeepLink::DashPayUser(user) => LinkRoute::AddContact {
            identity: user.identity_base58(),
            username: user.username,
        },
        DeepLink::Invitation(secret) => {
            drop(Zeroizing::new(secret));
            return Ok(Routed::Invitation);
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
    }))
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
        // An invitation needs the stash (intake); routed alone it names
        // nothing of the link.
        for invite in [
            "dashpay://invite?du=alice&assetlocktx=00&pk=secret&islock=00",
            "https://invitations.dashpay.io/applink?du=alice&pk=secret",
        ] {
            assert_eq!(route_text("mainnet", invite).unwrap(), Routed::Invitation);
            let json = serde_json::to_string(&r(invite)).unwrap();
            assert_eq!(json, r#"{"kind":"rejected","reason":{"kind":"notSaved"}}"#);
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
        assert!(inbox.drain("moon").is_err());
        assert_eq!(inbox.len(), MAX_QUEUED);
        let routes: Vec<LinkRoute> = inbox
            .drain("mainnet")
            .unwrap()
            .into_iter()
            .map(|l| match l {
                Some(text) => r(&text),
                None => LinkRoute::rejected(LinkRejected::TooLong),
            })
            .collect();
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
