use dw_engine::platform::{ConnectError, ConnectFailure, PlatformError};
use dw_uri::connect::{AppConnectRequest, AuthBinding, Bounds, Level, Status, base64url_encode};

use crate::error::{AppError, ErrorData};
use crate::shell::links::{
    ConnectRejected, Inbox, LinkOrigin, LinkRejected, LinkRoute, MAX_CONNECT_LINKS,
};

fn body() -> Vec<u8> {
    let mut eph = [2u8; 33];
    eph[1..].copy_from_slice(&[
        0x79, 0xbe, 0x66, 0x7e, 0xf9, 0xdc, 0xbb, 0xac, 0x55, 0xa0, 0x62, 0x95, 0xce, 0x87, 0x0b,
        0x07, 0x02, 0x9b, 0xfc, 0xdb, 0x2d, 0xce, 0x28, 0xd9, 0x59, 0xf2, 0x81, 0x5b, 0x16, 0xf8,
        0x17, 0x98,
    ]);
    AppConnectRequest {
        app_eph_pub: eph,
        app_id: [4; 32],
        label: "Yappr".into(),
        identity_hint: None,
        auth_bindings: vec![AuthBinding {
            bounds: Bounds::Group([5; 32]),
            level: Level::Critical,
            budget: Some(1_000),
            expires_at_ms: Some(2_000_000_000_000),
        }],
        connection_id: None,
        enc_bindings: Vec::new(),
        renew: false,
        created_at_ms: 1_800_000_000_000,
    }
    .encode()
}

const CB: &str = "https%3A%2F%2Fyap.pr%2Fstaging%2Fconnect%2F%3Fr%3Dabc";

fn connect_uri(cb: bool) -> String {
    let mut uri = format!("dashpay://connect?v=2&n=t&r={}", base64url_encode(&body()));
    if cb {
        uri.push_str("&cb=");
        uri.push_str(CB);
    }
    uri
}

fn take(inbox: &Inbox, text: &str, origin: LinkOrigin) -> LinkRoute {
    tauri::async_runtime::block_on(inbox.intake(None, "testnet", text, origin)).unwrap()
}

fn link_id(route: &LinkRoute) -> String {
    match route {
        LinkRoute::AppConnect { link_id, .. } => link_id.clone(),
        other => panic!("{other:?}"),
    }
}

/// A request typed, pasted or scanned never opens its return link, even
/// once answered; one the OS delivered opens it only once answered.
#[test]
fn only_an_os_link_opens_its_return_link() {
    let inbox = Inbox::default();
    let entered = take(&inbox, &connect_uri(true), LinkOrigin::Entered);
    assert!(matches!(entered, LinkRoute::AppConnect { returns_to: None, .. }));
    let id = link_id(&entered);
    assert!(inbox.connect.decline(&id));
    let link = inbox.connect.get(&id).unwrap();
    assert_eq!(link.answer, Some(Status::Declined));
    assert_eq!(link.return_url(), None);

    let os = take(&inbox, &connect_uri(true), LinkOrigin::Os);
    assert_eq!(
        os,
        LinkRoute::AppConnect {
            link_id: link_id(&os),
            returns_to: Some("yap.pr".into()),
        }
    );
    let id = link_id(&os);
    assert_eq!(inbox.connect.get(&id).unwrap().return_url(), None, "not answered");
    inbox.connect.answer(&id, Status::Ok);
    assert_eq!(
        inbox.connect.get(&id).unwrap().return_url().as_deref(),
        Some("https://yap.pr/staging/connect/?r=abc&ac=ok")
    );

    let bare = take(&inbox, &connect_uri(false), LinkOrigin::Os);
    let id = link_id(&bare);
    inbox.connect.answer(&id, Status::Ok);
    assert_eq!(inbox.connect.get(&id).unwrap().return_url(), None);
}

#[test]
fn a_started_approval_cannot_be_denied() {
    let inbox = Inbox::default();
    let id = link_id(&take(&inbox, &connect_uri(true), LinkOrigin::Os));
    assert!(inbox.connect.start(&id).is_some());
    assert!(inbox.connect.start(&id).is_none(), "once");
    assert!(!inbox.connect.decline(&id));
    inbox.connect.unstart(&id);
    assert!(inbox.connect.decline(&id));
    assert!(inbox.connect.start(&id).is_none(), "answered");
    inbox.connect.answer(&id, Status::Ok);
    assert_eq!(inbox.connect.get(&id).unwrap().answer, Some(Status::Declined), "first answer kept");
}

#[test]
fn connect_links_are_bounded() {
    let inbox = Inbox::default();
    let first = link_id(&take(&inbox, &connect_uri(false), LinkOrigin::Os));
    for _ in 0..MAX_CONNECT_LINKS {
        take(&inbox, &connect_uri(false), LinkOrigin::Os);
    }
    assert!(inbox.connect.get(&first).is_none());
}

#[test]
fn a_started_approval_keeps_its_link_past_the_bound() {
    let inbox = Inbox::default();
    let active = link_id(&take(&inbox, &connect_uri(false), LinkOrigin::Os));
    let queued: Vec<String> = (1..MAX_CONNECT_LINKS)
        .map(|_| link_id(&take(&inbox, &connect_uri(false), LinkOrigin::Os)))
        .collect();
    assert!(inbox.connect.start(&active).is_some());
    take(&inbox, &connect_uri(false), LinkOrigin::Os);
    inbox.connect.answer(&active, Status::Ok);
    assert_eq!(inbox.connect.get(&active).unwrap().answer, Some(Status::Ok));
    assert!(inbox.connect.get(&queued[0]).is_none(), "the oldest not started goes");
}

/// The request whose sheet is on screen stays past the bound, before
/// Approve is pressed; showing another sheet lets it go again.
#[test]
fn the_shown_request_keeps_its_link_past_the_bound() {
    let inbox = Inbox::default();
    let shown = link_id(&take(&inbox, &connect_uri(false), LinkOrigin::Os));
    assert!(inbox.connect.show(&shown).is_some());
    let queued: Vec<String> = (1..MAX_CONNECT_LINKS)
        .map(|_| link_id(&take(&inbox, &connect_uri(false), LinkOrigin::Os)))
        .collect();
    let next = link_id(&take(&inbox, &connect_uri(false), LinkOrigin::Os));
    assert!(inbox.connect.get(&shown).is_some_and(|l| l.shown));
    assert!(inbox.connect.get(&queued[0]).is_none(), "the oldest not shown goes");
    assert!(inbox.connect.start(&shown).is_some(), "Approve still answers it");

    assert!(inbox.connect.show(&next).is_some());
    assert!(!inbox.connect.get(&shown).unwrap().shown, "one sheet at a time");
    assert!(inbox.connect.show("unknown").is_none());
    assert!(!inbox.connect.get(&next).unwrap().shown);
}

/// With every request kept on screen or being approved, a new one is
/// refused, and none of those is lost.
#[test]
fn a_new_request_is_refused_when_every_kept_one_is_shown_or_started() {
    let inbox = Inbox::default();
    let kept: Vec<String> = (0..MAX_CONNECT_LINKS)
        .map(|_| link_id(&take(&inbox, &connect_uri(false), LinkOrigin::Os)))
        .collect();
    for id in &kept[1..] {
        assert!(inbox.connect.start(id).is_some());
    }
    assert!(inbox.connect.show(&kept[0]).is_some());
    assert_eq!(
        take(&inbox, &connect_uri(true), LinkOrigin::Os),
        LinkRoute::Rejected {
            reason: LinkRejected::Connect {
                reason: ConnectRejected::Busy
            }
        }
    );
    assert!(kept.iter().all(|id| inbox.connect.get(id).is_some()));

    for id in &kept[1..] {
        inbox.connect.unstart(id);
        inbox.connect.answer(id, Status::Ok);
        assert!(inbox.connect.start(id).is_none());
    }
    take(&inbox, &connect_uri(false), LinkOrigin::Os);
    assert!(inbox.connect.get(&kept[0]).is_some(), "still shown");
    assert!(inbox.connect.get(&kept[1]).is_none(), "an answered one goes");
}

/// "Retry sending" needs the approval that parked the response: a request
/// that never started has none to send.
#[test]
fn only_a_started_request_can_resend() {
    let inbox = Inbox::default();
    let id = link_id(&take(&inbox, &connect_uri(false), LinkOrigin::Os));
    assert!(inbox.connect.started(&id).is_none());
    assert!(inbox.connect.start(&id).is_some());
    assert!(inbox.connect.started(&id).is_some());
    assert!(inbox.connect.started("unknown").is_none());
}

#[test]
fn refused_links_get_no_sheet() {
    let inbox = Inbox::default();
    let rejected = |reason| LinkRoute::Rejected {
        reason: LinkRejected::Connect { reason },
    };
    assert_eq!(
        take(&inbox, "dashpay://connect?v=2&n=t&r=A", LinkOrigin::Os),
        rejected(ConnectRejected::Invalid)
    );
    assert_eq!(
        take(&inbox, "dashpay://connect?v=3&n=t&r=AAAA", LinkOrigin::Os),
        rejected(ConnectRejected::Unsupported)
    );
    assert_eq!(
        take(&inbox, "dashpay://sign?v=1&n=t&st=AAAA", LinkOrigin::Os),
        rejected(ConnectRejected::NeedsNewerWallet)
    );
    assert!(matches!(
        take(&inbox, "dash-key:abc?n=t&v=1", LinkOrigin::Entered),
        LinkRoute::Unsupported { .. }
    ));
}

/// An engine refusal reaches the renderer as its code, with no text of
/// the engine's or Platform's; `connect.failed` names its step from the
/// fixed list.
#[test]
fn connect_errors_carry_codes_only() {
    let e: AppError = ConnectError::Failed {
        failure: ConnectFailure::NotConfirmed,
    }
    .into();
    assert_eq!(e.code.as_str(), "connect.failed");
    assert_eq!(e.message, "connect.failed");
    assert_eq!(
        e.data,
        Some(ErrorData::ConnectFailure {
            failure: crate::error::ConnectFailureName::NotConfirmed
        })
    );
    let e: AppError = ConnectError::Credits {
        needed: 5,
        available: 1,
    }
    .into();
    assert_eq!(e.code.as_str(), "connect.credits");
    assert!(matches!(e.data, Some(ErrorData::Credits { needed: 5, available: 1 })));
    let e: AppError = ConnectError::Platform(PlatformError::Timeout).into();
    assert_eq!(e.code.as_str(), "platform.timeout");
    for c in [ConnectError::Network, ConnectError::MainnetOff, ConnectError::SendFailed] {
        let code = c.code();
        assert_eq!(AppError::from(c).message, code);
    }
}

/// `connect.network` names the network the request is for, and answers
/// the app `failed`.
#[test]
fn a_network_refusal_names_the_requested_network() {
    let inbox = Inbox::default();
    let id = link_id(&take(&inbox, &connect_uri(true), LinkOrigin::Os));
    let link = inbox.connect.get(&id).unwrap();
    let e = super::answered::<()>(&inbox.connect, &id, &link, Err(ConnectError::Network))
        .unwrap_err();
    assert_eq!(e.code.as_str(), "connect.network");
    assert_eq!(
        e.data,
        Some(ErrorData::ConnectNetwork {
            requested: crate::error::ConnectNetworkName::Testnet
        })
    );
    assert_eq!(inbox.connect.get(&id).unwrap().answer, Some(Status::Failed));
}

/// What the user can still fix or retry leaves the request open: a credit
/// shortfall, a bad argument, a Platform error, a send that failed.
#[test]
fn a_recoverable_refusal_leaves_the_request_open() {
    let inbox = Inbox::default();
    let id = link_id(&take(&inbox, &connect_uri(true), LinkOrigin::Os));
    let link = inbox.connect.get(&id).unwrap();
    for e in [
        ConnectError::Credits {
            needed: 2,
            available: 1,
        },
        ConnectError::Platform(PlatformError::InvalidArgument {
            detail: "funding limit".into(),
        }),
        ConnectError::Platform(PlatformError::Timeout),
        ConnectError::SendFailed,
        ConnectError::PlanChanged,
        ConnectError::Failed {
            failure: ConnectFailure::NotConfirmed,
        },
    ] {
        assert!(super::answered::<()>(&inbox.connect, &id, &link, Err(e)).is_err());
    }
    assert_eq!(inbox.connect.get(&id).unwrap().answer, None);
    assert!(inbox.connect.start(&id).is_some());
}

/// An approval that stops where the user can retry re-opens the request;
/// one that kept a response for "Retry sending" keeps it started.
#[test]
fn only_a_retryable_refusal_reopens_a_started_request() {
    let not_confirmed = ConnectError::Failed {
        failure: ConnectFailure::NotConfirmed,
    };
    let scope = ConnectError::Failed {
        failure: ConnectFailure::ScopeChanged,
    };
    assert!(not_confirmed.keeps_open());
    assert!(ConnectError::Platform(PlatformError::Timeout).keeps_open());
    assert!(!ConnectError::SendFailed.keeps_open());
    assert!(!scope.keeps_open());
    let inbox = Inbox::default();
    let id = link_id(&take(&inbox, &connect_uri(true), LinkOrigin::Os));
    for (e, reopens) in [(not_confirmed, true), (ConnectError::SendFailed, false)] {
        assert!(inbox.connect.start(&id).is_some());
        if e.keeps_open() {
            inbox.connect.unstart(&id);
        }
        assert_eq!(inbox.connect.start(&id).is_some(), reopens, "{e:?}");
        inbox.connect.unstart(&id);
    }
}
