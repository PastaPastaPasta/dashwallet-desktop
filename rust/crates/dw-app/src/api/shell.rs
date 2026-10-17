//! The shell's commands (U-01): the queued deep links, Open URI's routing
//! and the state the native menus mirror. The shell's own events are in
//! `shell`.

use dw_engine::platform::StashedLinkKind;

use crate::error::{AppCode, AppError};
use crate::ipc::commands;
use crate::shell::ShellSync;
use crate::shell::links::{self, LinkRoute, Routed, StashedLinkInfo};

pub(crate) const EXCLUDED: &[(&str, &str)] = &[(
    "NetworkSession::stash_link",
    "internal: links reach the stash through `take_links` and `route_link` (shell::links::intake), so a link's text never comes from the renderer as its own command",
)];

commands! {
    /// Takes the queued deep links (after `linksPending`, and once at
    /// start-up), routed for `network` (a directory name: the network the
    /// window shows), in arrival order. An invitation goes into the link
    /// stash and comes as `invitation` with its stash id: its link never
    /// crosses IPC. While the network has no wallet, a payment or contact
    /// link is kept there too and comes as `saved` (DP5-01).
    #[facade()]
    fn take_links(state, network: String) -> Vec<LinkRoute> {
        let session = state.session().await.ok();
        state.shell.inbox.take(session, &network).await
    }

    /// Where `text` leads, for File ▸ Open URI… (QT-019): the routing and
    /// intake a queued link gets, a text over `MAX_LINK_BYTES` included.
    #[facade()]
    fn route_link(state, network: String, text: String) -> LinkRoute {
        let session = state.session().await.ok();
        links::intake(session, &network, &zeroize::Zeroizing::new(text)).await
    }

    /// The links in the stash, oldest first, without their text (DP5-01):
    /// what the renderer offers after onboarding and after an unlock.
    #[facade("NetworkSession::stashed_links")]
    fn stashed_links(state) -> Vec<StashedLinkInfo> {
        let links = state.session().await?.stashed_links().await?;
        Ok(links.into_iter().map(Into::into).collect())
    }

    /// Opens a stashed link: a payment or contact link leaves the stash
    /// and comes back routed for the window's network (once: `null` when
    /// it is gone); an invitation stays and comes back as `invitation`.
    /// A locked vault is `platform.signer_unavailable` and keeps the link.
    #[facade("NetworkSession::take_stashed_link")]
    fn open_stashed_link(state, link_id: String) -> Option<LinkRoute> {
        let session = state.session().await?;
        let invitation = session
            .stashed_links()
            .await?
            .iter()
            .any(|l| l.id == link_id && l.kind == StashedLinkKind::Invitation);
        if invitation {
            return Ok(Some(LinkRoute::Invitation { link_id }));
        }
        let Some(text) = session.take_stashed_link(link_id).await? else {
            return Ok(None);
        };
        let network = session.network().dir_name();
        Ok(Some(match links::route_text(&network, text.expose())? {
            Routed::Route(route) => route,
            // Not stashed as a payment or contact link: nothing to open.
            Routed::Invitation => return Ok(None),
        }))
    }

    /// Drops a stashed link unread: the user dismissed it. Gone already is
    /// not an error.
    #[facade()]
    fn dismiss_stashed_link(state, link_id: String) -> () {
        let session = state.session().await?;
        let invitation = session
            .stashed_links()
            .await?
            .iter()
            .any(|l| l.id == link_id && l.kind == StashedLinkKind::Invitation);
        if invitation {
            session.forget_invitation(link_id).await?;
        } else {
            drop(session.take_stashed_link(link_id).await?);
        }
        Ok(())
    }

    /// What the native menus and the tray mirror: Discreet Mode's check
    /// and whether Lock Wallet is enabled; and whether a secret is on
    /// screen (the window's content protection). Answers once the
    /// protection change has run on the window (U-02 D27);
    /// `app.screen_protection` when it could not run.
    #[facade()]
    fn shell_sync(state, sync: ShellSync) -> () {
        state.shell.sync(sync);
        state
            .shell
            .protect(sync.secret_shown)
            .await
            .map_err(|e| AppError::new(AppCode::ScreenProtection, e))
    }
}
