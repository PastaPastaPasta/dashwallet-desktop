//! The shell's commands (U-01): the queued deep links, Open URI's routing
//! and the state the native menus mirror. The shell's own events are in
//! `shell`.

use crate::ipc::commands;
use crate::shell::ShellSync;
use crate::shell::links::{self, LinkRejected, LinkRoute, MAX_LINK_BYTES};

pub(crate) const EXCLUDED: &[(&str, &str)] = &[];

commands! {
    /// Takes the queued deep links (after `linksPending`, and once at
    /// start-up), routed for `network` (a directory name: the network the
    /// window shows), in arrival order. An invitation's link stays in the
    /// shell.
    #[facade()]
    fn take_links(state, network: String) -> Vec<LinkRoute> {
        state.shell.inbox.take(&network)
    }

    /// Where `text` leads, for File ▸ Open URI… (QT-019): the routing a
    /// queued link gets.
    #[facade()]
    fn route_link(state, network: String, text: String) -> LinkRoute {
        let text = zeroize::Zeroizing::new(text);
        if text.len() > MAX_LINK_BYTES {
            return Ok(LinkRoute::Rejected {
                reason: LinkRejected::TooLong,
            });
        }
        links::route(&network, &text)
    }

    /// What the native menus and the tray mirror: Discreet Mode's check
    /// and whether Lock Wallet is enabled.
    #[facade()]
    fn shell_sync(state, sync: ShellSync) -> () {
        state.shell.sync(sync);
        Ok(())
    }
}
