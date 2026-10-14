//! The shell around the renderer (U-01, DASHPAY §4): deep links, the native
//! menus, the tray, one instance per data root and the window's size and
//! position across launches.
//!
//! Everything the shell tells the renderer is one Tauri event,
//! `shell://event`, with a [`ShellEvent`] payload: a menu or tray command,
//! or "links are waiting" (the renderer then takes them, [`links`]). The
//! renderer owns navigation; the shell never changes the page itself.
//!
//! The native parts (plugins, menus, tray) are added in [`native`] by `run`
//! only: tests build the app on the mock runtime without them.

pub mod links;
pub mod menu;
pub mod native;
#[cfg(desktop)]
pub mod tray;

use std::io::Write as _;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, Runtime};
use ts_rs::TS;

use self::links::{Inbox, LinkSource, link_scheme};

/// The Tauri event the shell sends, with a [`ShellEvent`] payload.
pub const EVENT: &str = "shell://event";

/// What the shell tells the renderer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ShellEvent {
    /// Links are queued: call `takeLinks`.
    LinksPending,
    /// A menu, tray or shortcut command.
    Command { command: ShellCommand },
}

/// A command from the native menus or the tray.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ShellCommand {
    Navigate {
        route: Route,
    },
    /// Tools, on this tab (dash-qt's Information/Console/Peers/Repair and
    /// the advanced tools, DASHPAY §4).
    Tools {
        tab: ToolsTab,
    },
    /// File ▸ Open URI… (QT-019).
    OpenUri,
    Lock,
    ToggleDiscreet,
    About,
}

/// A top-level page of the window (DASHPAY §4 information architecture).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum Route {
    Home,
    Pay,
    Receive,
    Contacts,
    Activity,
    CoinJoin,
    Explore,
    Notifications,
    Profile,
    Settings,
    Tools,
    JoinDashPay,
}

/// A tab of Tools.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum ToolsTab {
    Information,
    Console,
    Peers,
    Repair,
    AddressBook,
    SignVerify,
    Psbt,
    CoinSelection,
}

/// What the native menus mirror from the renderer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ShellSync {
    /// Discreet mode is on (Wallet ▸ Discreet is checked).
    pub discreet: bool,
    /// An unlocked vault exists (Wallet ▸ Lock is enabled).
    pub can_lock: bool,
}

type SyncFn = Box<dyn Fn(ShellSync) + Send + Sync>;

/// The shell's state, shared by the commands (through `AppState`) and the
/// native callbacks (through Tauri's managed state), which may run before
/// `AppState` exists.
pub struct Shell {
    pub inbox: Inbox,
    /// Apply a [`ShellSync`] to the native menus and the tray, once they exist.
    sync: Mutex<Vec<SyncFn>>,
    /// The self-test's link trace (`--selftest-link-trace`).
    trace: Option<PathBuf>,
}

impl Shell {
    pub fn new(trace: Option<PathBuf>) -> Self {
        Shell {
            inbox: Inbox::default(),
            sync: Mutex::default(),
            trace,
        }
    }

    pub fn on_sync(&self, f: SyncFn) {
        self.sync.lock().unwrap_or_else(|e| e.into_inner()).push(f);
    }

    pub fn sync(&self, state: ShellSync) {
        for f in &*self.sync.lock().unwrap_or_else(|e| e.into_inner()) {
            f(state);
        }
    }

    /// Appends `line` to the self-test's link trace, if there is one. Lines
    /// name schemes and routes, never a link's text.
    pub fn trace(&self, line: &str) {
        let Some(path) = &self.trace else { return };
        let written = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .and_then(|mut f| writeln!(f, "{}", line.escape_debug()));
        if let Err(e) = written {
            eprintln!("dw-app: link trace: {e}");
        }
    }

    /// Queues the arguments that are links (`args` may hold others: a
    /// forwarded command line does) and returns how many were queued.
    pub fn queue(&self, args: impl IntoIterator<Item = String>, source: LinkSource) -> usize {
        self.queue_from(args, source, cfg!(windows))
    }

    /// [`Shell::queue`]. `windows_shell`: a launch's links came through the
    /// Windows shell, which adds a `/` to some ([`links::without_shell_slash`]).
    fn queue_from(
        &self,
        args: impl IntoIterator<Item = String>,
        source: LinkSource,
        windows_shell: bool,
    ) -> usize {
        let mut queued = 0;
        for arg in args {
            let Some(scheme) = link_scheme(&arg) else {
                continue;
            };
            let arg = match source {
                LinkSource::Launch | LinkSource::Forwarded if windows_shell => {
                    links::without_shell_slash(arg)
                }
                _ => arg,
            };
            if self.inbox.deliver(arg) {
                queued += 1;
                self.trace(&format!("received {} {scheme}", source.as_str()));
            } else {
                eprintln!("dw-app: link queue full; a {scheme}: link was dropped");
            }
        }
        queued
    }
}

/// The shell's state from Tauri's managed state.
pub fn shell<R: Runtime>(app: &AppHandle<R>) -> Option<Arc<Shell>> {
    app.try_state::<Arc<Shell>>().map(|s| Arc::clone(&s))
}

/// Sends `event` to the renderer.
pub fn emit<R: Runtime>(app: &AppHandle<R>, event: ShellEvent) {
    if let Err(e) = app.emit(EVENT, event) {
        eprintln!("dw-app: shell event: {e}");
    }
}

/// Shows, restores and focuses the main window.
pub fn reveal<R: Runtime>(app: &AppHandle<R>) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

/// Queues the links among `args`; when there are any, brings the window
/// forward and tells the renderer.
pub fn deliver_links<R: Runtime>(
    app: &AppHandle<R>,
    args: impl IntoIterator<Item = String>,
    source: LinkSource,
) {
    let Some(shell) = shell(app) else { return };
    if shell.queue(args, source) > 0 {
        reveal(app);
        emit(app, ShellEvent::LinksPending);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_links_are_queued_and_traced_by_scheme() {
        let dir = dw_testutil::private_tempdir();
        let trace = dir.path().join("trace");
        let shell = Shell::new(Some(trace.clone()));
        let args = [
            "/usr/bin/dash-wallet",
            "--datadir",
            "/x",
            "dashpay://user?id=secretish&username=bob",
            "Pay:Xabc",
        ];
        let queued = shell.queue(args.map(String::from), LinkSource::Forwarded);
        assert_eq!(queued, 2);
        assert_eq!(shell.inbox.len(), 2);
        assert_eq!(
            std::fs::read_to_string(&trace).unwrap(),
            "received forwarded dashpay\nreceived forwarded pay\n"
        );
    }

    #[test]
    fn events_serialise_with_a_kind() {
        let json = |e: ShellEvent| serde_json::to_value(e).unwrap();
        assert_eq!(
            json(ShellEvent::LinksPending),
            serde_json::json!({"kind": "linksPending"})
        );
        assert_eq!(
            json(ShellEvent::Command {
                command: ShellCommand::Navigate {
                    route: Route::JoinDashPay
                }
            }),
            serde_json::json!({"kind": "command", "command": {"kind": "navigate", "route": "joinDashPay"}})
        );
        assert_eq!(
            json(ShellEvent::Command {
                command: ShellCommand::Tools {
                    tab: ToolsTab::SignVerify
                }
            }),
            serde_json::json!({"kind": "command", "command": {"kind": "tools", "tab": "signVerify"}})
        );
    }
}
