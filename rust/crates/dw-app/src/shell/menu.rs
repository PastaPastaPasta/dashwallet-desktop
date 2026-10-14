//! The native menus (QT-011..018, DASHPAY §4) and their shortcuts.
//!
//! Every item is a row of [`ITEMS`]: its id, label, accelerator and what
//! it does. The menu bar and the tray menu are built from the rows, and one
//! global handler ([`handle`]) maps a clicked id back to its [`Action`]
//! (Tauri sends tray menu clicks to the global handler too). Shortcuts are
//! the menus' accelerators, so they work wherever the window has focus;
//! in a browser (the fixture harness) the renderer binds the same keys.
//!
//! Layout. macOS: the app menu (About, Settings…, Services, Hide, Quit),
//! File, Edit, View, Wallet, Tools, Window. Elsewhere: File (… Exit),
//! View, Wallet (… Settings…), Tools, Help (About). Edit exists on macOS
//! only: there the menu carries the clipboard shortcuts; WebKitGTK and
//! WebView2 handle them themselves. About is the `help.about` row on every
//! OS (on macOS too, not the predefined item), so it opens the renderer's
//! dialog.

use tauri::menu::{
    CheckMenuItem, CheckMenuItemBuilder, Menu, MenuEvent, MenuItem, MenuItemBuilder, SubmenuBuilder,
};
use tauri::{AppHandle, Manager, Runtime};

use super::{Route, ShellCommand, ShellEvent, ToolsTab};

/// What a menu item does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Sent to the renderer.
    Shell(ShellCommand),
    /// Ends the app (through `RunEvent::Exit`, which shuts the engine down).
    Quit,
    /// Tray: shows the window, or hides it when it is shown (and, except on
    /// Windows, focused).
    ToggleWindow,
}

/// One menu item.
#[derive(Debug, Clone, Copy)]
pub struct Item {
    pub id: &'static str,
    pub label: &'static str,
    pub accelerator: Option<&'static str>,
    pub action: Action,
}

const fn item(
    id: &'static str,
    label: &'static str,
    accelerator: Option<&'static str>,
    action: Action,
) -> Item {
    Item {
        id,
        label,
        accelerator,
        action,
    }
}

const fn nav(route: Route) -> Action {
    Action::Shell(ShellCommand::Navigate { route })
}

const fn tools(tab: ToolsTab) -> Action {
    Action::Shell(ShellCommand::Tools { tab })
}

/// Every item of the menu bar and the tray menu.
pub const ITEMS: &[Item] = &[
    // File
    item(
        "file.open-uri",
        "Open URI…",
        Some("CmdOrCtrl+O"),
        Action::Shell(ShellCommand::OpenUri),
    ),
    item(
        "file.sign-verify",
        "Sign or Verify Message…",
        None,
        tools(ToolsTab::SignVerify),
    ),
    item("file.psbt", "Load PSBT…", None, tools(ToolsTab::Psbt)),
    item("file.exit", "Exit", Some("CmdOrCtrl+Q"), Action::Quit),
    // View
    item("view.home", "Home", Some("CmdOrCtrl+1"), nav(Route::Home)),
    item("view.pay", "Pay", Some("CmdOrCtrl+2"), nav(Route::Pay)),
    item(
        "view.receive",
        "Receive",
        Some("CmdOrCtrl+3"),
        nav(Route::Receive),
    ),
    item(
        "view.contacts",
        "Contacts",
        Some("CmdOrCtrl+4"),
        nav(Route::Contacts),
    ),
    item(
        "view.activity",
        "Activity",
        Some("CmdOrCtrl+5"),
        nav(Route::Activity),
    ),
    item(
        "view.coinjoin",
        "CoinJoin",
        Some("CmdOrCtrl+6"),
        nav(Route::CoinJoin),
    ),
    item(
        "view.notifications",
        "Notifications",
        None,
        nav(Route::Notifications),
    ),
    item("view.profile", "My Profile", None, nav(Route::Profile)),
    // Wallet
    item(
        "wallet.lock",
        "Lock Wallet",
        Some("CmdOrCtrl+L"),
        Action::Shell(ShellCommand::Lock),
    ),
    item(
        "wallet.discreet",
        "Discreet Mode",
        Some("CmdOrCtrl+Shift+D"),
        Action::Shell(ShellCommand::ToggleDiscreet),
    ),
    item(
        "wallet.add",
        "Add Wallet…",
        None,
        Action::Shell(ShellCommand::AddWallet),
    ),
    item(
        "wallet.recovery-phrase",
        "Show Recovery Phrase…",
        None,
        Action::Shell(ShellCommand::RecoveryPhrase),
    ),
    item(
        "wallet.settings",
        "Settings…",
        Some("CmdOrCtrl+,"),
        nav(Route::Settings),
    ),
    // Tools (dash-qt's shortcuts)
    item(
        "tools.information",
        "Information",
        Some("CmdOrCtrl+Shift+I"),
        tools(ToolsTab::Information),
    ),
    item(
        "tools.console",
        "Console",
        Some("CmdOrCtrl+Shift+C"),
        tools(ToolsTab::Console),
    ),
    item(
        "tools.peers",
        "Peers",
        Some("CmdOrCtrl+Shift+P"),
        tools(ToolsTab::Peers),
    ),
    item(
        "tools.repair",
        "Wallet Repair",
        Some("CmdOrCtrl+Shift+R"),
        tools(ToolsTab::Repair),
    ),
    item(
        "tools.address-book",
        "Address Book",
        None,
        tools(ToolsTab::AddressBook),
    ),
    item(
        "tools.coin-selection",
        "Coin Selection",
        None,
        tools(ToolsTab::CoinSelection),
    ),
    // Help (the app menu on macOS)
    item(
        "help.about",
        "About Dash Wallet",
        None,
        Action::Shell(ShellCommand::About),
    ),
    // Tray
    item(
        "tray.toggle",
        "Show or Hide Dash Wallet",
        None,
        Action::ToggleWindow,
    ),
    item("tray.pay", "Pay…", None, nav(Route::Pay)),
    item("tray.receive", "Receive…", None, nav(Route::Receive)),
    item("tray.activity", "Activity", None, nav(Route::Activity)),
    item(
        "tray.lock",
        "Lock Wallet",
        None,
        Action::Shell(ShellCommand::Lock),
    ),
    item("tray.quit", "Quit Dash Wallet", None, Action::Quit),
];

/// The row of `id`.
pub fn find(id: &str) -> Option<&'static Item> {
    ITEMS.iter().find(|i| i.id == id)
}

fn row(id: &str) -> &'static Item {
    find(id).unwrap_or_else(|| panic!("menu item {id} is in ITEMS"))
}

pub(super) fn menu_item<R: Runtime, M: Manager<R>>(
    manager: &M,
    id: &str,
) -> tauri::Result<MenuItem<R>> {
    let item = row(id);
    let mut b = MenuItemBuilder::with_id(item.id, item.label);
    if let Some(a) = item.accelerator {
        b = b.accelerator(a);
    }
    b.build(manager)
}

/// One entry of a menu in the bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Entry {
    /// A row of [`ITEMS`], by id.
    Item(&'static str),
    /// A row of [`ITEMS`] shown as a check item (Discreet Mode).
    Check(&'static str),
    Separator,
    /// One of the platform's predefined items.
    Native(Native),
}

/// The predefined items the bar uses. There is no About: ours is the
/// `help.about` row on every OS, since the predefined one opens the native
/// panel without a menu event, bypassing the renderer's dialog and its
/// rules (D20: none while locked or behind a sheet).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Native {
    Services,
    Hide,
    HideOthers,
    ShowAll,
    Quit,
    Fullscreen,
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    SelectAll,
    Minimize,
    Maximize,
    BringAllToFront,
}

/// The menu bar as data, in the macOS layout when `mac`: each menu's title
/// and entries. Tests read it; [`layout`] makes the native menus from it
/// (native menus can be made on the main thread only, which a test is not
/// on macOS).
fn bar(mac: bool) -> Vec<(&'static str, Vec<Entry>)> {
    use Entry::{Check, Item, Native as N, Separator};
    use Native::*;
    let mut menus = Vec::new();
    if mac {
        menus.push((
            "Dash Wallet",
            vec![
                Item("help.about"),
                Separator,
                Item("wallet.settings"),
                Separator,
                N(Services),
                Separator,
                N(Hide),
                N(HideOthers),
                N(ShowAll),
                Separator,
                N(Quit),
            ],
        ));
    }
    let mut file = vec![
        Item("file.open-uri"),
        Separator,
        Item("file.sign-verify"),
        Item("file.psbt"),
    ];
    if !mac {
        file.extend([Separator, Item("file.exit")]);
    }
    menus.push(("File", file));
    if mac {
        menus.push((
            "Edit",
            vec![
                N(Undo),
                N(Redo),
                Separator,
                N(Cut),
                N(Copy),
                N(Paste),
                N(SelectAll),
            ],
        ));
    }
    let mut view = vec![
        Item("view.home"),
        Item("view.pay"),
        Item("view.receive"),
        Item("view.contacts"),
        Item("view.activity"),
        Item("view.coinjoin"),
        Separator,
        Item("view.notifications"),
        Item("view.profile"),
    ];
    if mac {
        view.extend([Separator, N(Fullscreen)]);
    }
    menus.push(("View", view));
    let mut wallet = vec![
        Item("wallet.lock"),
        Check("wallet.discreet"),
        Separator,
        Item("wallet.add"),
        Item("wallet.recovery-phrase"),
    ];
    if !mac {
        wallet.extend([Separator, Item("wallet.settings")]);
    }
    menus.push(("Wallet", wallet));
    menus.push((
        "Tools",
        vec![
            Item("tools.information"),
            Item("tools.console"),
            Item("tools.peers"),
            Item("tools.repair"),
            Separator,
            Item("tools.address-book"),
            Item("tools.coin-selection"),
        ],
    ));
    if mac {
        menus.push((
            "Window",
            vec![N(Minimize), N(Maximize), Separator, N(BringAllToFront)],
        ));
    } else {
        menus.push(("Help", vec![Item("help.about")]));
    }
    menus
}

/// The menu bar. Wallet ▸ Lock and Discreet Mode follow the renderer
/// (`shell_sync`).
pub fn build<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<Menu<R>> {
    layout(app, cfg!(target_os = "macos"))
}

/// The native menus of [`bar`].
fn layout<R: Runtime>(app: &AppHandle<R>, mac: bool) -> tauri::Result<Menu<R>> {
    let menu = Menu::new(app)?;
    let (mut lock, mut discreet) = (None, None);
    for (title, entries) in bar(mac) {
        let mut sub = SubmenuBuilder::new(app, title);
        for entry in entries {
            sub = match entry {
                Entry::Item(id) => {
                    let item = menu_item(app, id)?;
                    if id == "wallet.lock" {
                        // Enabled once the renderer says it can lock.
                        item.set_enabled(false)?;
                        lock = Some(item.clone());
                    }
                    sub.item(&item)
                }
                Entry::Check(id) => {
                    let r = row(id);
                    let item: CheckMenuItem<R> = CheckMenuItemBuilder::with_id(r.id, r.label)
                        .checked(false)
                        .accelerator(r.accelerator.unwrap_or_default())
                        .build(app)?;
                    discreet = Some(item.clone());
                    sub.item(&item)
                }
                Entry::Separator => sub.separator(),
                Entry::Native(n) => match n {
                    Native::Services => sub.services(),
                    Native::Hide => sub.hide(),
                    Native::HideOthers => sub.hide_others(),
                    Native::ShowAll => sub.show_all(),
                    Native::Quit => sub.quit(),
                    Native::Fullscreen => sub.fullscreen(),
                    Native::Undo => sub.undo(),
                    Native::Redo => sub.redo(),
                    Native::Cut => sub.cut(),
                    Native::Copy => sub.copy(),
                    Native::Paste => sub.paste(),
                    Native::SelectAll => sub.select_all(),
                    Native::Minimize => sub.minimize(),
                    Native::Maximize => sub.maximize(),
                    Native::BringAllToFront => sub.bring_all_to_front(),
                },
            };
        }
        menu.append(&sub.build()?)?;
    }
    if let (Some(shell), Some(lock), Some(discreet)) = (super::shell(app), lock, discreet) {
        shell.on_sync(Box::new(move |s| {
            let _ = lock.set_enabled(s.can_lock);
            let _ = discreet.set_checked(s.discreet);
        }));
    }
    Ok(menu)
}

/// The global menu handler: the menu bar's and the tray menu's clicks.
pub fn handle<R: Runtime>(app: &AppHandle<R>, event: MenuEvent) {
    let Some(item) = find(event.id().as_ref()) else {
        return;
    };
    run(app, item.action);
}

/// Carries out `action`.
pub fn run<R: Runtime>(app: &AppHandle<R>, action: Action) {
    match action {
        Action::Shell(command) => {
            super::reveal(app);
            super::emit(app, ShellEvent::Command { command });
        }
        Action::Quit => app.exit(0),
        Action::ToggleWindow => {
            let Some(window) = app.get_webview_window("main") else {
                return;
            };
            let shown =
                window.is_visible().unwrap_or(false) && !window.is_minimized().unwrap_or(false);
            // A click in the Windows notification area focuses the taskbar
            // first, so there an unfocused window counts as shown too.
            let shown = shown && (cfg!(windows) || window.is_focused().unwrap_or(false));
            if shown {
                let _ = window.hide();
            } else {
                super::reveal(app);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    #[test]
    fn ids_and_accelerators_are_unique() {
        let mut ids = BTreeSet::new();
        let mut keys = BTreeSet::new();
        for item in ITEMS {
            assert!(ids.insert(item.id), "duplicate id {}", item.id);
            if let Some(a) = item.accelerator {
                assert!(keys.insert(a), "duplicate accelerator {a}");
                // The webview's own editing keys stay with it.
                let plain = a.strip_prefix("CmdOrCtrl+").unwrap_or(a);
                assert!(
                    !["A", "C", "V", "X", "Z", "F"].contains(&plain),
                    "{a} shadows an editing key"
                );
            }
        }
    }

    #[test]
    fn every_route_and_tab_has_a_menu_way_in() {
        let actions: Vec<Action> = ITEMS.iter().map(|i| i.action).collect();
        for route in [
            Route::Home,
            Route::Pay,
            Route::Receive,
            Route::Contacts,
            Route::Activity,
            Route::CoinJoin,
            Route::Notifications,
            Route::Profile,
            Route::Settings,
        ] {
            assert!(actions.contains(&nav(route)), "{route:?}");
        }
        for tab in [
            ToolsTab::Information,
            ToolsTab::Console,
            ToolsTab::Peers,
            ToolsTab::Repair,
            ToolsTab::AddressBook,
            ToolsTab::SignVerify,
            ToolsTab::Psbt,
            ToolsTab::CoinSelection,
        ] {
            assert!(actions.contains(&tools(tab)), "{tab:?}");
        }
    }

    #[test]
    fn about_is_our_item_in_both_layouts() {
        // The bar as data: native menus need the main thread, which a test
        // is not on (macOS).
        for mac in [true, false] {
            let ids: Vec<&str> = bar(mac)
                .into_iter()
                .flat_map(|(_, entries)| entries)
                .filter_map(|e| match e {
                    Entry::Item(id) | Entry::Check(id) => Some(id),
                    _ => None,
                })
                .collect();
            // Every entry is a row, so `layout` cannot panic on a missing one.
            for id in &ids {
                assert!(find(id).is_some(), "mac={mac}: {id} is not in ITEMS");
            }
            // About sends ShellCommand::About (the renderer's dialog, D20):
            // on macOS too, where the predefined item would send nothing.
            let about = ids.iter().filter(|id| **id == "help.about").count();
            assert_eq!(about, 1, "mac={mac}: {ids:?}");
        }
        assert_eq!(
            find("help.about").unwrap().action,
            Action::Shell(ShellCommand::About)
        );
    }

    #[test]
    fn wallet_menu_has_add_and_recovery_phrase_in_both_layouts() {
        for mac in [true, false] {
            let (_, wallet) = bar(mac)
                .into_iter()
                .find(|(title, _)| *title == "Wallet")
                .expect("a Wallet menu");
            for id in ["wallet.lock", "wallet.add", "wallet.recovery-phrase"] {
                assert!(
                    wallet
                        .iter()
                        .any(|e| matches!(e, Entry::Item(i) if *i == id)),
                    "mac={mac}: {id} is not in the Wallet menu"
                );
            }
        }
    }

    #[test]
    fn ids_map_to_their_actions() {
        assert_eq!(find("view.contacts").unwrap().action, nav(Route::Contacts));
        assert_eq!(find("tray.quit").unwrap().action, Action::Quit);
        assert_eq!(
            find("wallet.lock").unwrap().action,
            Action::Shell(ShellCommand::Lock)
        );
        assert_eq!(
            find("wallet.add").unwrap().action,
            Action::Shell(ShellCommand::AddWallet)
        );
        assert_eq!(
            find("wallet.recovery-phrase").unwrap().action,
            Action::Shell(ShellCommand::RecoveryPhrase)
        );
        assert!(find("nope").is_none());
    }
}
