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
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ConnectDone {
    pub identity: String,
    pub keys_added: Vec<u32>,
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
}

#[cfg(test)]
#[path = "connect_tests.rs"]
mod tests;
