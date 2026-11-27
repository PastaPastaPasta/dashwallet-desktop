//! App Connect sign-ins (DASHPAY F24): the sheet, the quote, the approval
//! and the answer to the app, for a request the inbox keeps by id
//! (`shell::links::ConnectLinks`). The request itself never crosses IPC
//! from the renderer: every command names its link id, and the return link
//! is built and opened here.

use dw_engine::platform as engine;
use dw_uri::connect::Status;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::error::{AppCode, AppError, ErrorData};
use crate::ipc::commands;
use crate::shell::links::ConnectLink;

pub(crate) const EXCLUDED: &[(&str, &str)] = &[];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum ConnectLevel {
    Critical,
    High,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ConnectDocumentType {
    pub contract: String,
    pub document_type: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ConnectToken {
    pub contract: String,
    pub position: u16,
}

/// A contract group as Platform proves it now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ConnectGroup {
    pub group: String,
    pub owner: String,
    pub admins: Vec<String>,
    pub contracts: Vec<String>,
    pub document_types: Vec<ConnectDocumentType>,
    pub tokens: Vec<ConnectToken>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ConnectScope {
    Contract { contract: String, tokens: bool },
    DocumentType { contract: String, document_type: String },
    Profile,
    Group { group: ConnectGroup },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ConnectBinding {
    pub index: u8,
    pub primary: bool,
    pub level: ConnectLevel,
    pub scope: ConnectScope,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ConnectIdentity {
    pub identity: String,
    pub username: String,
    #[ts(type = "string | null")]
    #[serde(with = "crate::dto::credits::option")]
    pub balance: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ConnectTerms {
    #[ts(type = "string")]
    #[serde(with = "crate::dto::credits")]
    pub funding_limit: u64,
    /// UNIX milliseconds.
    #[ts(type = "number")]
    pub expires_at_ms: u64,
}

/// The approval sheet. `label` is the app's own claim: nothing verifies
/// it. `returnsTo` is the host the app is returned to, for a request the
/// OS delivered with a return link.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ConnectSheet {
    pub request: String,
    pub app_id: String,
    pub label: String,
    pub app_owner: String,
    pub returns_to: Option<String>,
    pub identities: Vec<ConnectIdentity>,
    pub bindings: Vec<ConnectBinding>,
    pub encryption: bool,
    pub terms: ConnectTerms,
    #[ts(type = "string")]
    #[serde(with = "crate::dto::credits")]
    pub max_funding_limit: u64,
    /// An approval of this request went to Platform from this wallet and no
    /// response to it is published: its keys may be on the identity, and
    /// approving again with this choice reuses them.
    pub earlier_approval: Option<ConnectChoice>,
    /// UNIX milliseconds: Platform's block time the sheet was read at.
    #[ts(type = "number")]
    pub block_time_ms: u64,
    /// The digest of the group facts the bindings show, hex.
    pub groups: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ConnectChoice {
    pub identity: String,
    pub selection: Vec<u8>,
    pub terms: ConnectTerms,
}

/// What an approval costs and the `appConnect` grant to ask for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ConnectQuote {
    pub grant: crate::api::vault::GrantPurposeSpec,
    #[ts(type = "string")]
    #[serde(with = "crate::dto::credits")]
    pub fee: u64,
    #[ts(type = "Array<string>")]
    pub budgets: Vec<String>,
    #[ts(type = "string")]
    #[serde(with = "crate::dto::credits")]
    pub balance: u64,
    /// The digest of the group facts the quote read, hex: the sheet reads
    /// the request again when it shows others.
    pub groups: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ConnectDone {
    pub identity: String,
    pub keys_added: Vec<u32>,
}

/// One still-enabled AUTHENTICATION session key of a connected app.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ConnectedAppKey {
    pub key_id: u32,
    pub level: ConnectLevel,
    pub scope: ConnectScope,
    #[ts(type = "string | null")]
    #[serde(with = "crate::dto::credits::option")]
    pub budget: Option<u64>,
    #[ts(type = "number | null")]
    pub expires_at_ms: Option<u64>,
}

/// One app this wallet approved for an identity that still has enabled
/// session keys on Platform. `label` is the app's own claim: nothing
/// verifies it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ConnectedApp {
    pub request: String,
    pub app_id: String,
    pub label: String,
    pub returns_to: Option<String>,
    pub identity: String,
    pub keys: Vec<ConnectedAppKey>,
    #[ts(type = "string")]
    #[serde(with = "crate::dto::credits")]
    pub funding_limit: u64,
    #[ts(type = "number | null")]
    pub expires_at_ms: Option<u64>,
    #[ts(type = "number")]
    pub approved_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ConnectedApps {
    pub identity: String,
    pub apps: Vec<ConnectedApp>,
    #[ts(type = "number")]
    pub block_time_ms: u64,
}

/// What disconnecting one app costs and the `appDisconnect` grant to ask for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct DisconnectQuote {
    pub grant: crate::api::vault::GrantPurposeSpec,
    pub identity: String,
    pub keys: Vec<u32>,
    #[ts(type = "string")]
    #[serde(with = "crate::dto::credits")]
    pub fee: u64,
    #[ts(type = "string")]
    #[serde(with = "crate::dto::credits")]
    pub balance: u64,
    /// True when an earlier Disconnect attempt for this app was retired after
    /// a revision or nonce conflict and may already have been charged a fee.
    pub prior_attempt_conflict: bool,
    /// Observed identity balance debit (in credits) from that retired attempt,
    /// or `null` when no balance drop was proved.
    #[ts(type = "string | null")]
    #[serde(with = "crate::dto::credits::option")]
    pub prior_attempt_debit: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct DisconnectDone {
    pub identity: String,
    pub keys_disabled: Vec<u32>,
    /// True when an earlier sign-in response document for this approval was
    /// still in flight or confirmed on Platform, so a decryption key may
    /// already have reached the app despite the session keys being disabled.
    pub response_may_have_landed: bool,
}

impl From<engine::ConnectLevel> for ConnectLevel {
    fn from(l: engine::ConnectLevel) -> Self {
        match l {
            engine::ConnectLevel::Critical => Self::Critical,
            engine::ConnectLevel::High => Self::High,
        }
    }
}

impl From<engine::ConnectScope> for ConnectScope {
    fn from(s: engine::ConnectScope) -> Self {
        use engine::ConnectScope as S;
        match s {
            S::Contract { contract, tokens } => Self::Contract { contract, tokens },
            S::DocumentType {
                contract,
                document_type,
            } => Self::DocumentType {
                contract,
                document_type,
            },
            S::Profile => Self::Profile,
            S::Group(g) => Self::Group {
                group: ConnectGroup {
                    group: g.group,
                    owner: g.owner,
                    admins: g.admins,
                    contracts: g.contracts,
                    document_types: (g.document_types.into_iter())
                        .map(|d| ConnectDocumentType {
                            contract: d.contract,
                            document_type: d.document_type,
                        })
                        .collect(),
                    tokens: (g.tokens.into_iter())
                        .map(|t| ConnectToken {
                            contract: t.contract,
                            position: t.position,
                        })
                        .collect(),
                },
            },
        }
    }
}

impl From<engine::ConnectTerms> for ConnectTerms {
    fn from(t: engine::ConnectTerms) -> Self {
        Self {
            funding_limit: t.funding_limit,
            expires_at_ms: t.expires_at_ms,
        }
    }
}

impl From<ConnectTerms> for engine::ConnectTerms {
    fn from(t: ConnectTerms) -> Self {
        Self {
            funding_limit: t.funding_limit,
            expires_at_ms: t.expires_at_ms,
        }
    }
}

impl From<engine::ConnectChoice> for ConnectChoice {
    fn from(c: engine::ConnectChoice) -> Self {
        Self {
            identity: c.identity,
            selection: c.selection,
            terms: c.terms.into(),
        }
    }
}

impl From<ConnectChoice> for engine::ConnectChoice {
    fn from(c: ConnectChoice) -> Self {
        Self {
            identity: c.identity,
            selection: c.selection,
            terms: c.terms.into(),
        }
    }
}

fn sheet_of(s: engine::ConnectSheet, link: &ConnectLink) -> ConnectSheet {
    ConnectSheet {
        request: s.request,
        app_id: s.app_id,
        label: s.label,
        app_owner: s.app_owner,
        returns_to: link.returns_to(),
        identities: (s.identities.into_iter())
            .map(|i| ConnectIdentity {
                identity: i.identity,
                username: i.username,
                balance: i.balance,
            })
            .collect(),
        bindings: (s.bindings.into_iter())
            .map(|b| ConnectBinding {
                index: b.index,
                primary: b.primary,
                level: b.level.into(),
                scope: b.scope.into(),
            })
            .collect(),
        encryption: s.encryption,
        terms: s.terms.into(),
        max_funding_limit: s.max_funding_limit,
        earlier_approval: s.earlier_approval.map(Into::into),
        block_time_ms: s.block_time_ms,
        groups: hex::encode(s.groups),
    }
}

impl From<engine::ConnectDone> for ConnectDone {
    fn from(d: engine::ConnectDone) -> Self {
        ConnectDone {
            identity: d.identity,
            keys_added: d.keys_added,
        }
    }
}

impl From<engine::ConnectedAppKey> for ConnectedAppKey {
    fn from(k: engine::ConnectedAppKey) -> Self {
        Self {
            key_id: k.key_id,
            level: k.level.into(),
            scope: k.scope.into(),
            budget: k.budget,
            expires_at_ms: k.expires_at_ms,
        }
    }
}

impl From<engine::ConnectedApp> for ConnectedApp {
    fn from(a: engine::ConnectedApp) -> Self {
        Self {
            request: a.request,
            app_id: a.app_id,
            label: a.label,
            returns_to: a.returns_to,
            identity: a.identity,
            keys: a.keys.into_iter().map(Into::into).collect(),
            funding_limit: a.funding_limit,
            expires_at_ms: a.expires_at_ms,
            approved_at_ms: a.approved_at_ms,
        }
    }
}

impl From<engine::ConnectedApps> for ConnectedApps {
    fn from(c: engine::ConnectedApps) -> Self {
        Self {
            identity: c.identity,
            apps: c.apps.into_iter().map(Into::into).collect(),
            block_time_ms: c.block_time_ms,
        }
    }
}

impl From<engine::DisconnectDone> for DisconnectDone {
    fn from(d: engine::DisconnectDone) -> Self {
        Self {
            identity: d.identity,
            keys_disabled: d.keys_disabled,
            response_may_have_landed: d.response_may_have_landed,
        }
    }
}

fn unknown_link() -> AppError {
    AppError::invalid_argument("no such sign-in request")
}

/// Records the status a refusal answers the app with, then passes it on.
fn answered<T>(
    links: &crate::shell::links::ConnectLinks,
    link_id: &str,
    link: &ConnectLink,
    result: Result<T, engine::ConnectError>,
) -> Result<T, AppError> {
    let e = match result {
        Ok(v) => return Ok(v),
        Err(e) => e,
    };
    if let Some(status) = e.status() {
        links.answer(link_id, status);
    }
    let network = matches!(e, engine::ConnectError::Network);
    let mut e = AppError::from(e);
    if network {
        e.data = Some(ErrorData::ConnectNetwork {
            requested: link.uri.network.into(),
        });
    }
    Err(e)
}

commands! {
    /// The approval sheet of the App Connect request `link_id` for the
    /// wallet `wallet_id`, which the inbox then keeps as the one on screen.
    /// A refusal answers the request (its status goes back with the return
    /// link).
    #[facade("DashPay::app_connect_sheet")]
    fn app_connect_sheet(state, wallet_id: String, link_id: String) -> ConnectSheet {
        let links = &state.shell.inbox.connect;
        let link = links.show(&link_id).ok_or_else(unknown_link)?;
        let dashpay = state.dashpay(&wallet_id).await?;
        let sheet = answered(links, &link_id, &link, dashpay.app_connect_sheet(link.uri.clone()).await)?;
        Ok(sheet_of(sheet, &link))
    }

    /// What approving `choice` costs and the `appConnect` grant to ask
    /// for, bound to what this quote read.
    #[facade("DashPay::app_connect_quote")]
    fn app_connect_quote(
        state,
        wallet_id: String,
        link_id: String,
        choice: ConnectChoice,
    ) -> ConnectQuote {
        let links = &state.shell.inbox.connect;
        let link = links.get(&link_id).ok_or_else(unknown_link)?;
        let dashpay = state.dashpay(&wallet_id).await?;
        let q = dashpay.app_connect_quote(link.uri.clone(), choice.into()).await;
        let q = answered(links, &link_id, &link, q)?;
        Ok(ConnectQuote {
            grant: crate::api::vault::GrantPurposeSpec::AppConnect {
                max_credits: q.max_credits,
                request: q.request,
                plan: q.plan,
            },
            fee: q.fee,
            budgets: q.budgets.iter().map(u64::to_string).collect(),
            balance: q.balance,
            groups: hex::encode(q.groups),
        })
    }

    /// Approves `choice` under `grant` (an `appConnect` grant from this
    /// request's quote): adds the keys and sends the app its response.
    #[facade("DashPay::app_connect_approve")]
    fn app_connect_approve(
        state,
        wallet_id: String,
        link_id: String,
        choice: ConnectChoice,
        grant: String,
    ) -> ConnectDone {
        let links = &state.shell.inbox.connect;
        let dashpay = state.dashpay(&wallet_id).await?;
        let link = links.start(&link_id).ok_or(engine::ConnectError::Answered)?;
        let done = dashpay.app_connect_approve(link.uri.clone(), choice.into(), grant).await;
        if done.as_ref().is_err_and(engine::ConnectError::keeps_open) {
            links.unstart(&link_id);
        }
        let done = answered(links, &link_id, &link, done)?;
        links.answer(&link_id, Status::Ok);
        Ok(done.into())
    }

    /// Sends the response an approval of `link_id` could not send; a
    /// request with no approval started has none (`invalid_argument`).
    #[facade("DashPay::app_connect_retry_send")]
    fn app_connect_retry_send(state, wallet_id: String, link_id: String) -> ConnectDone {
        let links = &state.shell.inbox.connect;
        let link = links.started(&link_id).ok_or_else(unknown_link)?;
        let dashpay = state.dashpay(&wallet_id).await?;
        let done = dashpay.app_connect_retry_send(link.uri.clone()).await;
        let done = answered(links, &link_id, &link, done)?;
        links.answer(&link_id, Status::Ok);
        Ok(done.into())
    }

    /// The user denied the request: nothing is read, derived or signed.
    /// Refused once an approval started (`connect.answered`).
    #[facade()]
    fn app_connect_deny(state, link_id: String) -> () {
        let links = &state.shell.inbox.connect;
        links.get(&link_id).ok_or_else(unknown_link)?;
        if !links.decline(&link_id) {
            return Err(engine::ConnectError::Answered.into());
        }
        Ok(())
    }

    /// Opens the return link of an answered request the OS delivered, with
    /// its answer, in the system browser, after the user's click. A request
    /// typed, pasted or scanned, or not answered yet, opens nothing:
    /// `invalid_argument`.
    #[facade()]
    fn app_connect_return(state, link_id: String) -> () {
        let link = (state.shell.inbox.connect).get(&link_id).ok_or_else(unknown_link)?;
        let url = link
            .return_url()
            .ok_or_else(|| AppError::invalid_argument("no return link to open"))?;
        tauri::async_runtime::spawn_blocking(move || open::that_detached(&url))
            .await
            .map_err(|e| AppError::app(AppCode::BrowserOpen, e.to_string()))?
            .map_err(|e| AppError::app(AppCode::BrowserOpen, e.to_string()))?;
        Ok(())
    }

    /// Forgets the request: the sheet closed.
    #[facade()]
    fn app_connect_dismiss(state, link_id: String) -> () {
        state.shell.inbox.connect.forget(&link_id);
        Ok(())
    }

    /// Lists each app this wallet approved for `identity` that still has
    /// enabled session keys on Platform. Refused on mainnet.
    #[facade("DashPay::app_connect_list")]
    fn app_connect_list(state, wallet_id: String, identity: String) -> ConnectedApps {
        let dashpay = state.dashpay(&wallet_id).await?;
        let apps = dashpay.app_connect_list(identity).await?;
        Ok(apps.into())
    }

    /// What disconnecting `request` on `identity` costs and the
    /// `appDisconnect` grant to ask for. Refused on mainnet.
    #[facade("DashPay::app_connect_disconnect_quote")]
    fn app_connect_disconnect_quote(
        state,
        wallet_id: String,
        identity: String,
        request: String,
    ) -> DisconnectQuote {
        let dashpay = state.dashpay(&wallet_id).await?;
        let q = dashpay.app_connect_disconnect_quote(identity, request).await?;
        Ok(DisconnectQuote {
            grant: crate::api::vault::GrantPurposeSpec::AppDisconnect {
                max_credits: q.max_credits,
                identity: q.identity_id,
                request: q.request,
            },
            identity: q.identity,
            keys: q.keys,
            fee: q.fee,
            balance: q.balance,
            prior_attempt_conflict: q.prior_attempt_conflict,
            prior_attempt_debit: q.prior_attempt_debit,
        })
    }

    /// Disables the verified AUTHENTICATION session keys of `request` on
    /// `identity` under `grant` (an `appDisconnect` grant). Refused on mainnet.
    #[facade("DashPay::app_connect_disconnect")]
    fn app_connect_disconnect(
        state,
        wallet_id: String,
        identity: String,
        request: String,
        grant: String,
    ) -> DisconnectDone {
        let dashpay = state.dashpay(&wallet_id).await?;
        let done = dashpay.app_connect_disconnect(identity, request, grant).await?;
        Ok(done.into())
    }
}

#[cfg(test)]
#[path = "connect_tests.rs"]
mod tests;
