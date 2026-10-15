//! DashPay: contacts, contact payments, the notification bell, profiles and
//! avatars (`DashPay`, DP2–DP4).
//!
//! Every command takes the wallet id first and acts on one of the wallet's
//! identities (Base58). The writes that cost credits take a `grant`: a lease
//! id (`beginFlow`, in the identity domain) or a vault grant id, as every
//! DashPay write does (m4 §1).

mod contacts;
mod events;
mod payments;
mod profile;

use dw_engine::platform::BearerSecret;

use self::contacts::{
    ContactDetailInfo, ContactList, ContactQueryArgs, ContactRequestOutcome, ContactSummaryInfo,
    PrivateContactDetails, PublishStateName, ScannedContactInfo,
};
pub(crate) use self::events::DashPayEventInfo;
use self::events::DashPayEventPage;
use self::payments::{
    ActivityFilterName, ContactActivityPage, LockResolutionName, PaymentLockInfo,
};
use self::profile::{
    AvatarCandidateInfo, AvatarSizeName, AvatarSourceArgs, AvatarThumbnail, DashPayProfile,
    DashPayProfileLimits, ProfileEditArgs,
};
use crate::dto::platform::EligibilityName;
use crate::ipc::commands;

pub(crate) const EXCLUDED: &[(&str, &str)] = &[];

commands! {
    /// The contacts of `identity` in the sections `query` selects.
    #[facade("DashPay::contacts")]
    fn dashpay_contacts(
        state,
        wallet_id: String,
        identity: String,
        query: ContactQueryArgs,
    ) -> ContactList {
        let dashpay = state.dashpay(&wallet_id).await?;
        Ok(dashpay.contacts(identity, query.into())?.into())
    }

    /// One contact with its profile and private details; `null` when
    /// `contact` has no relation to `identity`.
    #[facade("DashPay::contact")]
    fn dashpay_contact(
        state,
        wallet_id: String,
        identity: String,
        contact: String,
    ) -> Option<ContactDetailInfo> {
        let dashpay = state.dashpay(&wallet_id).await?;
        Ok(dashpay.contact(identity, contact)?.map(Into::into))
    }

    /// How many contacts wait for an unlock to finish their crypto.
    #[facade("DashPay::pending_setup_count")]
    fn dashpay_pending_setup_count(state, wallet_id: String) -> u32 {
        let dashpay = state.dashpay(&wallet_id).await?;
        Ok(dashpay.pending_setup_count()?)
    }

    /// Whether `identity` may send a contact request to `contact`.
    #[facade("DashPay::eligibility")]
    fn dashpay_eligibility(
        state,
        wallet_id: String,
        identity: String,
        contact: String,
    ) -> EligibilityName {
        let dashpay = state.dashpay(&wallet_id).await?;
        Ok(dashpay.eligibility(identity, contact).await?.into())
    }

    /// Sends a contact request to `to` under `grant` (a `contactRequest`
    /// flow). `scan` is the proof id of `dashpayVerifyScanned`, when the
    /// request answers a scanned code.
    #[facade("DashPay::send_request")]
    fn dashpay_send_request(
        state,
        wallet_id: String,
        identity: String,
        to: String,
        scan: Option<String>,
        grant: String,
    ) -> ContactRequestOutcome {
        let dashpay = state.dashpay(&wallet_id).await?;
        Ok(dashpay.send_request(identity, to, scan, grant).await?.into())
    }

    /// Accepts the request `from` sent, under `grant` (an `accept` or
    /// `acceptAndPay` flow).
    #[facade("DashPay::accept_request")]
    fn dashpay_accept_request(
        state,
        wallet_id: String,
        identity: String,
        from: String,
        grant: String,
    ) -> ContactRequestOutcome {
        let dashpay = state.dashpay(&wallet_id).await?;
        Ok(dashpay.accept_request(identity, from, grant).await?.into())
    }

    /// Ignores `contact` (moves it to the hidden section).
    #[facade("DashPay::ignore")]
    fn dashpay_ignore_contact(
        state,
        wallet_id: String,
        identity: String,
        contact: String,
    ) -> () {
        let dashpay = state.dashpay(&wallet_id).await?;
        Ok(dashpay.ignore(identity, contact).await?)
    }

    /// Undoes `dashpayIgnoreContact`.
    #[facade("DashPay::unignore")]
    fn dashpay_unignore_contact(
        state,
        wallet_id: String,
        identity: String,
        contact: String,
    ) -> () {
        let dashpay = state.dashpay(&wallet_id).await?;
        Ok(dashpay.unignore(identity, contact).await?)
    }

    /// Sets the alias, note and hidden flag only `identity` sees. With a
    /// `grant` (a `privateDetails` flow) they are published; the result says
    /// whether they were.
    #[facade("DashPay::set_private_details")]
    fn dashpay_set_private_details(
        state,
        wallet_id: String,
        identity: String,
        contact: String,
        details: PrivateContactDetails,
        grant: Option<String>,
    ) -> PublishStateName {
        let dashpay = state.dashpay(&wallet_id).await?;
        let published = dashpay
            .set_private_details(identity, contact, details.into(), grant)
            .await?;
        Ok(published.into())
    }

    /// Adds the DashPay keys to `identity`, under `grant` (an
    /// `enableDashPayKeys` flow).
    #[facade("DashPay::enable_dashpay_keys")]
    fn dashpay_enable_keys(state, wallet_id: String, identity: String, grant: String) -> () {
        let dashpay = state.dashpay(&wallet_id).await?;
        Ok(dashpay.enable_dashpay_keys(identity, grant).await?)
    }

    /// The user link of `identity`, which others scan or paste to add it.
    #[facade("DashPay::my_user_link")]
    fn dashpay_my_user_link(state, wallet_id: String, identity: String) -> String {
        let dashpay = state.dashpay(&wallet_id).await?;
        Ok(dashpay.my_user_link(identity)?)
    }

    /// Verifies a scanned or pasted contact payload against Platform. The
    /// payload may carry an auto-accept proof, so it is held as a secret.
    #[facade("DashPay::verify_scanned")]
    fn dashpay_verify_scanned(state, wallet_id: String, text: String) -> ScannedContactInfo {
        let text = BearerSecret::new(text);
        let dashpay = state.dashpay(&wallet_id).await?;
        Ok(dashpay.verify_scanned(text).await?.into())
    }

    /// The send lock `contact` is under after an ambiguous payment; `null`
    /// when none.
    #[facade("DashPay::payment_lock")]
    fn contact_payment_lock(
        state,
        wallet_id: String,
        identity: String,
        contact: String,
    ) -> Option<PaymentLockInfo> {
        let dashpay = state.dashpay(&wallet_id).await?;
        Ok(dashpay.payment_lock(identity, contact)?.map(Into::into))
    }

    /// Resolves `contact`'s payment lock from the evidence the engine has;
    /// the lock is cleared when the result is `sent` or `notSent`.
    #[facade("DashPay::resolve_payment_lock")]
    fn resolve_contact_payment_lock(
        state,
        wallet_id: String,
        identity: String,
        contact: String,
    ) -> LockResolutionName {
        let dashpay = state.dashpay(&wallet_id).await?;
        Ok(dashpay.resolve_payment_lock(identity, contact).await?.into())
    }

    /// The payments with `contact`, newest first; `cursor` is the previous
    /// page's `nextCursor`.
    #[facade("DashPay::contact_activity")]
    fn contact_activity(
        state,
        wallet_id: String,
        identity: String,
        contact: String,
        cursor: Option<String>,
        filter: ActivityFilterName,
    ) -> ContactActivityPage {
        let dashpay = state.dashpay(&wallet_id).await?;
        let page = dashpay
            .contact_activity(identity, contact, cursor, filter.into())
            .await?;
        Ok(page.into())
    }

    /// At most `limit` contacts `identity` pays often.
    #[facade("DashPay::frequent_contacts")]
    fn frequent_contacts(
        state,
        wallet_id: String,
        identity: String,
        limit: u32,
    ) -> Vec<ContactSummaryInfo> {
        let dashpay = state.dashpay(&wallet_id).await?;
        let rows = dashpay.frequent_contacts(identity, limit)?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    /// The bell: pending requests, then unread and read events, `limit` per
    /// page; `cursor` is the previous page's `nextCursor`.
    #[facade("DashPay::events")]
    fn dashpay_events(
        state,
        wallet_id: String,
        identity: String,
        cursor: Option<u64>,
        limit: u32,
    ) -> DashPayEventPage {
        let dashpay = state.dashpay(&wallet_id).await?;
        Ok(dashpay.events(identity, cursor, limit).await?.into())
    }

    /// Marks the events up to id `upTo` read.
    #[facade("DashPay::mark_read")]
    fn dashpay_mark_read(state, wallet_id: String, identity: String, up_to: u64) -> () {
        let dashpay = state.dashpay(&wallet_id).await?;
        Ok(dashpay.mark_read(identity, up_to).await?)
    }

    /// Unread events of `identity`, for the bell's badge.
    #[facade("DashPay::unread_count")]
    fn dashpay_unread_count(state, wallet_id: String, identity: String) -> u32 {
        let dashpay = state.dashpay(&wallet_id).await?;
        Ok(dashpay.unread_count(identity)?)
    }

    /// The profile of `identity` (the wallet's own or a contact's);
    /// `null` when it has none.
    #[facade("DashPay::profile")]
    fn dashpay_profile(
        state,
        wallet_id: String,
        identity: String,
    ) -> Option<DashPayProfile> {
        let dashpay = state.dashpay(&wallet_id).await?;
        Ok(dashpay.profile(identity)?.map(Into::into))
    }

    /// The contract's character limits for a profile.
    #[facade("DashPay::profile_limits")]
    fn dashpay_profile_limits(state, wallet_id: String) -> DashPayProfileLimits {
        let dashpay = state.dashpay(&wallet_id).await?;
        Ok(dashpay.profile_limits()?.into())
    }

    /// Publishes the whole new profile under `grant` (a `profileEdit`
    /// flow); `null` fields are cleared.
    #[facade("DashPay::update_profile")]
    fn dashpay_update_profile(
        state,
        wallet_id: String,
        identity: String,
        edit: ProfileEditArgs,
        grant: String,
    ) -> () {
        let dashpay = state.dashpay(&wallet_id).await?;
        Ok(dashpay.update_profile(identity, edit.into(), grant).await?)
    }

    /// Turns a file, URL or Gravatar into an avatar candidate. The engine
    /// enforces the size limits and re-encodes the image.
    #[facade("DashPay::prepare_avatar")]
    fn dashpay_prepare_avatar(
        state,
        wallet_id: String,
        source: AvatarSourceArgs,
    ) -> AvatarCandidateInfo {
        let dashpay = state.dashpay(&wallet_id).await?;
        Ok(dashpay.prepare_avatar(source.into()).await?.into())
    }

    /// Whether `dashpayUploadAvatar` can run (an upload host is configured).
    #[facade("DashPay::avatar_upload_available")]
    fn dashpay_avatar_upload_available(state, wallet_id: String) -> bool {
        let dashpay = state.dashpay(&wallet_id).await?;
        Ok(dashpay.avatar_upload_available()?)
    }

    /// Uploads a `file` candidate; returns the URL to publish.
    #[facade("DashPay::upload_avatar")]
    fn dashpay_upload_avatar(state, wallet_id: String, candidate: String) -> String {
        let dashpay = state.dashpay(&wallet_id).await?;
        Ok(dashpay.upload_avatar(candidate).await?)
    }

    /// The avatar of `identity` at `size`; `null` when it has none.
    #[facade("DashPay::avatar")]
    fn dashpay_avatar(
        state,
        wallet_id: String,
        identity: String,
        size: AvatarSizeName,
    ) -> Option<AvatarThumbnail> {
        let dashpay = state.dashpay(&wallet_id).await?;
        Ok(dashpay.avatar(identity, size.into()).await?.map(Into::into))
    }
}

#[cfg(test)]
mod tests {
    use dw_engine::platform::{
        AvatarChange, AvatarSource, ContactQuery, ContactSection, ContactSort, PrivateDetails,
        RequestOutcome,
    };
    use serde_json::json;

    use super::contacts::*;
    use super::profile::*;

    #[test]
    fn a_contact_query_parses_camel_case_and_maps_every_field() {
        let q: ContactQueryArgs = serde_json::from_value(json!({
            "sections": ["requests", "hidden"],
            "sort": "lastActivity",
            "text": "ann",
        }))
        .unwrap();
        assert_eq!(
            ContactQuery::from(q),
            ContactQuery {
                sections: vec![ContactSection::Requests, ContactSection::Hidden],
                sort: ContactSort::LastActivity,
                text: Some("ann".into()),
            }
        );
    }

    #[test]
    fn a_request_outcome_is_a_tagged_object() {
        let o = ContactRequestOutcome::from(RequestOutcome::Established {
            request: "r1".into(),
        });
        assert_eq!(
            serde_json::to_value(o).unwrap(),
            json!({ "kind": "established", "request": "r1" })
        );
    }

    #[test]
    fn private_details_round_trip() {
        let d = PrivateDetails {
            alias: Some("Ann".into()),
            note: None,
            hidden: true,
        };
        let dto = PrivateContactDetails::from(d.clone());
        assert_eq!(
            serde_json::to_value(&dto).unwrap(),
            json!({ "alias": "Ann", "note": null, "hidden": true })
        );
        assert_eq!(PrivateDetails::from(dto), d);
    }

    #[test]
    fn avatar_sources_carry_file_bytes_as_base64() {
        let s: AvatarSourceArgs = serde_json::from_value(json!({
            "kind": "file",
            "bytes": "AQID",
            "crop": { "x": 1, "y": 2, "width": 3, "height": 4 },
        }))
        .unwrap();
        assert_eq!(format!("{s:?}"), "File { bytes: 3, crop: Some(CropArgs { x: 1, y: 2, width: 3, height: 4 }) }");
        match AvatarSource::from(s) {
            AvatarSource::File { bytes, crop } => {
                assert_eq!(bytes, [1, 2, 3]);
                assert_eq!(crop.map(|c| (c.x, c.y, c.width, c.height)), Some((1, 2, 3, 4)));
            }
            other => panic!("{other:?}"),
        }
        let g: AvatarSourceArgs =
            serde_json::from_value(json!({ "kind": "gravatar", "email": "a@b.c" })).unwrap();
        assert_eq!(format!("{g:?}"), "Gravatar { .. }");
        assert!(serde_json::from_value::<AvatarSourceArgs>(json!({ "kind": "file", "bytes": "!" })).is_err());
    }

    #[test]
    fn a_profile_edit_maps_the_avatar_change() {
        let e: ProfileEditArgs = serde_json::from_value(json!({
            "displayName": "Ann",
            "publicMessage": null,
            "avatar": { "kind": "set", "candidate": "c1" },
        }))
        .unwrap();
        let e = dw_engine::platform::ProfileEdit::from(e);
        assert_eq!(e.display_name.as_deref(), Some("Ann"));
        assert_eq!(e.public_message, None);
        assert_eq!(e.avatar, AvatarChange::Set { candidate: "c1".into() });
        assert_eq!(e.baseline, None);
        let e: ProfileEditArgs = serde_json::from_value(json!({
            "displayName": "Ann",
            "publicMessage": null,
            "avatar": { "kind": "keep" },
            "baseline": { "profile": null },
        }))
        .unwrap();
        let e = dw_engine::platform::ProfileEdit::from(e);
        assert_eq!(e.baseline.map(|b| b.profile), Some(None));
    }

    #[test]
    fn a_thumbnail_serialises_png_as_base64() {
        let t = AvatarThumbnail::from(dw_engine::platform::AvatarImage {
            png: vec![1, 2, 3],
            size: dw_engine::platform::AvatarSize::Large,
        });
        assert_eq!(
            serde_json::to_value(t).unwrap(),
            json!({ "png": "AQID", "size": "large" })
        );
    }
}
