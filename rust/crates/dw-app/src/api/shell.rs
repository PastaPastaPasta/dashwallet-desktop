//! The shell's commands (U-01): the queued deep links, Open URI's routing
//! and the state the native menus mirror. The shell's own events are in
//! `shell`.

use crate::ipc::commands;
use crate::shell::ShellSync;
use crate::shell::links::{self, LinkRoute};

pub(crate) const EXCLUDED: &[(&str, &str)] = &[];

commands! {
    /// Takes the queued deep links (after `linksPending`, and once at
    /// start-up), routed for `network` (a directory name: the network the
    /// window shows), in arrival order. An invitation comes as a bare
    /// `invitation`: its link never crosses IPC.
    #[facade()]
    fn take_links(state, network: String) -> Vec<LinkRoute> {
        state.shell.inbox.take(&network)
    }

    /// Where `text` leads, for File ▸ Open URI… (QT-019): the routing a
    /// queued link gets, a text over `MAX_LINK_BYTES` included.
    #[facade()]
    fn route_link(state, network: String, text: String) -> LinkRoute {
        links::route(&network, &zeroize::Zeroizing::new(text))
    }

    /// What the native menus and the tray mirror: Discreet Mode's check
    /// and whether Lock Wallet is enabled.
    #[facade()]
    fn shell_sync(state, sync: ShellSync) -> () {
        state.shell.sync(sync);
        Ok(())
    }
}
