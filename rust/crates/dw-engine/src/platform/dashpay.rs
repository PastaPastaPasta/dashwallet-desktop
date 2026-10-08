//! The DashPay facade (DASHPAY §2.4, §3.6): plain Rust that `dwcli` drives
//! directly and the UI binding (E0-13) wraps one to one. The contract is
//! `docs/contracts/m4-dashpay-engine.md`.
//!
//! E0-08 is the skeleton: every call returns `platform.not_implemented`
//! with its name. The DP tasks fill in the bodies without changing a
//! signature, or update the contract in the same change. Sync calls read
//! in-memory state; async calls touch the network or persistence.

use std::sync::Arc;

use super::errors::{
    AvatarError, ContactError, CreditsError, InvitationError, NameError, PlatformError,
    RegistrationError,
};
use super::records::*;
use crate::{NetworkSession, WalletId};

/// DashPay for one wallet of a network session. A stateless handle: state
/// that outlives a call (avatar candidates, scan proofs, caches) lives in
/// the session, so any number of handles may exist for one wallet.
pub struct DashPay {
    #[expect(dead_code, reason = "read by the bodies the DP tasks write")]
    session: Arc<NetworkSession>,
    wallet_id: WalletId,
}

impl NetworkSession {
    /// The DashPay facade for `wallet_id`. Cheap; the calls check the wallet.
    pub fn dashpay(self: &Arc<Self>, wallet_id: WalletId) -> Arc<DashPay> {
        Arc::new(DashPay {
            session: Arc::clone(self),
            wallet_id,
        })
    }

    /// Stores an invitation link in the network's vault as
    /// `invitation/<id>` and returns the id. Per network, not per wallet: a
    /// link may arrive before any wallet exists (F18).
    #[expect(unused_variables, reason = "stub until DP5-01")]
    pub async fn stash_invitation(&self, link: BearerSecret) -> Result<String, InvitationError> {
        stub("NetworkSession.stash_invitation")
    }

    #[expect(unused_variables, reason = "stub until DP5-02")]
    pub async fn invitation_status(
        &self,
        link_id: String,
    ) -> Result<InvitationStatus, InvitationError> {
        stub("NetworkSession.invitation_status")
    }
}

/// The skeleton's body for every call.
fn stub<T, E: From<PlatformError>>(call: &str) -> Result<T, E> {
    Err(PlatformError::NotImplemented { call: call.into() }.into())
}

/// Validates a username label offline (F4). Pure; needs no session.
#[expect(unused_variables, reason = "stub until DP1-03")]
pub fn check_username(label: &str) -> Result<UsernameCheck, NameError> {
    stub("check_username")
}

#[expect(unused_variables, reason = "stubs until the DP tasks")]
impl DashPay {
    pub fn wallet_id(&self) -> WalletId {
        self.wallet_id
    }

    // ---- status and identity ----

    pub fn status(&self) -> Result<DashPayStatus, PlatformError> {
        stub("DashPay.status")
    }

    pub fn sync_status(&self) -> Result<DashPaySyncStatus, PlatformError> {
        stub("DashPay.sync_status")
    }

    pub async fn sync_now(&self) -> Result<SyncPassReport, PlatformError> {
        stub("DashPay.sync_now")
    }

    pub fn identities(&self) -> Result<Vec<IdentitySummary>, PlatformError> {
        stub("DashPay.identities")
    }

    pub async fn set_main_identity(&self, identity: String) -> Result<(), PlatformError> {
        stub("DashPay.set_main_identity")
    }

    pub async fn identity_detail(&self, identity: String) -> Result<IdentityDetail, PlatformError> {
        stub("DashPay.identity_detail")
    }

    pub async fn refresh_balance(&self, identity: String) -> Result<Option<u64>, PlatformError> {
        stub("DashPay.refresh_balance")
    }

    pub async fn discover_identities(&self, grant: String) -> Result<u32, PlatformError> {
        stub("DashPay.discover_identities")
    }

    // ---- registration ----

    pub async fn registration_quote(
        &self,
        req: RegistrationRequest,
    ) -> Result<RegistrationQuote, RegistrationError> {
        stub("DashPay.registration_quote")
    }

    pub async fn start_registration(
        &self,
        req: RegistrationRequest,
        grant: String,
    ) -> Result<String, RegistrationError> {
        stub("DashPay.start_registration")
    }

    pub async fn registrations(&self) -> Result<Vec<RegistrationStatus>, RegistrationError> {
        stub("DashPay.registrations")
    }

    pub async fn resume_registration(
        &self,
        draft: String,
        grant: Option<String>,
    ) -> Result<(), RegistrationError> {
        stub("DashPay.resume_registration")
    }

    pub async fn discard_registration(&self, draft: String) -> Result<(), RegistrationError> {
        stub("DashPay.discard_registration")
    }

    pub async fn finish_asset_locks(
        &self,
        grant: String,
    ) -> Result<FinishReport, RegistrationError> {
        stub("DashPay.finish_asset_locks")
    }

    // ---- names ----

    pub async fn name_availability(&self, label: String) -> Result<NameAvailability, NameError> {
        stub("DashPay.name_availability")
    }

    pub async fn register_name(
        &self,
        identity: String,
        label: String,
        grant: String,
    ) -> Result<NameOutcome, NameError> {
        stub("DashPay.register_name")
    }

    pub async fn contest_status(
        &self,
        identity: String,
        label: String,
    ) -> Result<ContestStatus, NameError> {
        stub("DashPay.contest_status")
    }

    pub async fn search_users(
        &self,
        prefix: String,
        limit: u32,
    ) -> Result<Vec<UserHit>, NameError> {
        stub("DashPay.search_users")
    }

    pub async fn resolve_user(&self, username: String) -> Result<Option<UserHit>, NameError> {
        stub("DashPay.resolve_user")
    }

    // ---- contacts ----

    pub fn contacts(
        &self,
        identity: String,
        q: ContactQuery,
    ) -> Result<ContactsPage, ContactError> {
        stub("DashPay.contacts")
    }

    pub fn contact(
        &self,
        identity: String,
        contact: String,
    ) -> Result<Option<ContactDetail>, ContactError> {
        stub("DashPay.contact")
    }

    pub fn pending_setup_count(&self) -> Result<u32, ContactError> {
        stub("DashPay.pending_setup_count")
    }

    pub async fn eligibility(
        &self,
        identity: String,
        contact: String,
    ) -> Result<Eligibility, ContactError> {
        stub("DashPay.eligibility")
    }

    pub async fn send_request(
        &self,
        identity: String,
        to: String,
        scan: Option<String>,
        grant: String,
    ) -> Result<RequestOutcome, ContactError> {
        stub("DashPay.send_request")
    }

    pub async fn accept_request(
        &self,
        identity: String,
        from: String,
        grant: String,
    ) -> Result<RequestOutcome, ContactError> {
        stub("DashPay.accept_request")
    }

    pub async fn ignore(&self, identity: String, contact: String) -> Result<(), ContactError> {
        stub("DashPay.ignore")
    }

    pub async fn unignore(&self, identity: String, contact: String) -> Result<(), ContactError> {
        stub("DashPay.unignore")
    }

    pub async fn set_private_details(
        &self,
        identity: String,
        contact: String,
        d: PrivateDetails,
        grant: Option<String>,
    ) -> Result<PublishState, ContactError> {
        stub("DashPay.set_private_details")
    }

    pub async fn enable_dashpay_keys(
        &self,
        identity: String,
        grant: String,
    ) -> Result<(), ContactError> {
        stub("DashPay.enable_dashpay_keys")
    }

    pub fn my_user_link(&self, identity: String) -> Result<String, ContactError> {
        stub("DashPay.my_user_link")
    }

    pub async fn verify_scanned(&self, text: BearerSecret) -> Result<ScannedContact, ContactError> {
        stub("DashPay.verify_scanned")
    }

    // ---- payments and activity ----

    pub fn payment_lock(
        &self,
        identity: String,
        contact: String,
    ) -> Result<Option<PaymentLock>, ContactError> {
        stub("DashPay.payment_lock")
    }

    pub async fn resolve_payment_lock(
        &self,
        identity: String,
        contact: String,
    ) -> Result<LockResolution, ContactError> {
        stub("DashPay.resolve_payment_lock")
    }

    pub async fn contact_activity(
        &self,
        identity: String,
        contact: String,
        cursor: Option<String>,
        f: ActivityFilter,
    ) -> Result<ActivityPage, ContactError> {
        stub("DashPay.contact_activity")
    }

    pub fn frequent_contacts(
        &self,
        identity: String,
        limit: u32,
    ) -> Result<Vec<ContactSummary>, ContactError> {
        stub("DashPay.frequent_contacts")
    }

    // ---- notifications ----

    pub async fn events(
        &self,
        identity: String,
        cursor: Option<u64>,
        limit: u32,
    ) -> Result<EventPage, PlatformError> {
        stub("DashPay.events")
    }

    pub fn unread_count(&self, identity: String) -> Result<u32, PlatformError> {
        stub("DashPay.unread_count")
    }

    pub async fn mark_read(&self, identity: String, up_to: u64) -> Result<(), PlatformError> {
        stub("DashPay.mark_read")
    }

    // ---- profile and avatars ----

    pub fn profile(&self, identity: String) -> Result<Option<Profile>, PlatformError> {
        stub("DashPay.profile")
    }

    pub fn profile_limits(&self) -> Result<ProfileLimits, PlatformError> {
        stub("DashPay.profile_limits")
    }

    pub async fn prepare_avatar(&self, src: AvatarSource) -> Result<AvatarCandidate, AvatarError> {
        stub("DashPay.prepare_avatar")
    }

    pub fn avatar_upload_available(&self) -> Result<bool, AvatarError> {
        stub("DashPay.avatar_upload_available")
    }

    pub async fn upload_avatar(&self, candidate: String) -> Result<String, AvatarError> {
        stub("DashPay.upload_avatar")
    }

    pub async fn update_profile(
        &self,
        identity: String,
        edit: ProfileEdit,
        grant: String,
    ) -> Result<(), PlatformError> {
        stub("DashPay.update_profile")
    }

    pub async fn avatar(
        &self,
        identity: String,
        size: AvatarSize,
    ) -> Result<Option<AvatarImage>, AvatarError> {
        stub("DashPay.avatar")
    }

    // ---- credits ----

    pub fn cost_table(&self) -> Result<CostTable, CreditsError> {
        stub("DashPay.cost_table")
    }

    pub async fn top_up(
        &self,
        identity: String,
        duffs: u64,
        grant: String,
    ) -> Result<TopUpOutcome, CreditsError> {
        stub("DashPay.top_up")
    }

    pub async fn withdraw(
        &self,
        identity: String,
        to: String,
        amount: WithdrawAmount,
        grant: String,
    ) -> Result<WithdrawOutcome, CreditsError> {
        stub("DashPay.withdraw")
    }
}
