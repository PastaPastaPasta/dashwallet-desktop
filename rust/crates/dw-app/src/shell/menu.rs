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
//! WebView2 handle them themselves.

use tauri::menu::{
    AboutMetadataBuilder, CheckMenuItem, CheckMenuItemBuilder, IsMenuItem, Menu, MenuEvent,
    MenuItem, MenuItemBuilder, PredefinedMenuItem, Submenu, SubmenuBuilder,
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
    /// Tray: shows the window, or hides it when it is shown and focused.
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

fn refs<R: Runtime>(v: &[Box<dyn IsMenuItem<R>>]) -> Vec<&dyn IsMenuItem<R>> {
    v.iter().map(|b| b.as_ref()).collect()
}

fn submenu<R: Runtime>(
    app: &AppHandle<R>,
    label: &str,
    items: &[&dyn IsMenuItem<R>],
) -> tauri::Result<Submenu<R>> {
    SubmenuBuilder::new(app, label).items(items).build()
}

/// The menu bar. Wallet ▸ Lock and Discreet Mode follow the renderer
/// (`shell_sync`).
pub fn build<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<Menu<R>> {
    let it = |id| menu_item(app, id);
    let sep = || PredefinedMenuItem::separator(app);
    let mac = cfg!(target_os = "macos");

    let lock = it("wallet.lock")?;
    lock.set_enabled(false)?;
    let discreet_row = row("wallet.discreet");
    let discreet: CheckMenuItem<R> =
        CheckMenuItemBuilder::with_id(discreet_row.id, discreet_row.label)
            .checked(false)
            .accelerator(discreet_row.accelerator.unwrap_or_default())
            .build(app)?;
    if let Some(shell) = super::shell(app) {
        let (lock, discreet) = (lock.clone(), discreet.clone());
        shell.on_sync(Box::new(move |s| {
            let _ = lock.set_enabled(s.can_lock);
            let _ = discreet.set_checked(s.discreet);
        }));
    }

    let mut file: Vec<Box<dyn IsMenuItem<R>>> = vec![
        Box::new(it("file.open-uri")?),
        Box::new(sep()?),
        Box::new(it("file.sign-verify")?),
        Box::new(it("file.psbt")?),
    ];
    if !mac {
        file.push(Box::new(sep()?));
        file.push(Box::new(it("file.exit")?));
    }
    let mut view: Vec<Box<dyn IsMenuItem<R>>> = Vec::new();
    for id in [
        "view.home",
        "view.pay",
        "view.receive",
        "view.contacts",
        "view.activity",
        "view.coinjoin",
    ] {
        view.push(Box::new(it(id)?));
    }
    view.push(Box::new(sep()?));
    view.push(Box::new(it("view.notifications")?));
    view.push(Box::new(it("view.profile")?));
    if mac {
        view.push(Box::new(sep()?));
        view.push(Box::new(PredefinedMenuItem::fullscreen(app, None)?));
    }
    let mut wallet: Vec<Box<dyn IsMenuItem<R>>> = vec![Box::new(lock), Box::new(discreet)];
    if !mac {
        wallet.push(Box::new(sep()?));
        wallet.push(Box::new(it("wallet.settings")?));
    }
    let tools: Vec<Box<dyn IsMenuItem<R>>> = vec![
        Box::new(it("tools.information")?),
        Box::new(it("tools.console")?),
        Box::new(it("tools.peers")?),
        Box::new(it("tools.repair")?),
        Box::new(sep()?),
        Box::new(it("tools.address-book")?),
        Box::new(it("tools.coin-selection")?),
    ];

    let menu = Menu::new(app)?;
    if mac {
        let about = AboutMetadataBuilder::new()
            .name(Some("Dash Wallet"))
            .version(Some(env!("CARGO_PKG_VERSION")))
            .build();
        menu.append(&submenu(
            app,
            "Dash Wallet",
            &[
                &PredefinedMenuItem::about(app, Some("About Dash Wallet"), Some(about))?,
                &sep()?,
                &it("wallet.settings")?,
                &sep()?,
                &PredefinedMenuItem::services(app, None)?,
                &sep()?,
                &PredefinedMenuItem::hide(app, None)?,
                &PredefinedMenuItem::hide_others(app, None)?,
                &PredefinedMenuItem::show_all(app, None)?,
                &sep()?,
                &PredefinedMenuItem::quit(app, None)?,
            ],
        )?)?;
    }
    menu.append(&submenu(app, "File", &refs(&file))?)?;
    if mac {
        menu.append(&submenu(
            app,
            "Edit",
            &[
                &PredefinedMenuItem::undo(app, None)?,
                &PredefinedMenuItem::redo(app, None)?,
                &sep()?,
                &PredefinedMenuItem::cut(app, None)?,
                &PredefinedMenuItem::copy(app, None)?,
                &PredefinedMenuItem::paste(app, None)?,
                &PredefinedMenuItem::select_all(app, None)?,
            ],
        )?)?;
    }
    menu.append(&submenu(app, "View", &refs(&view))?)?;
    menu.append(&submenu(app, "Wallet", &refs(&wallet))?)?;
    menu.append(&submenu(app, "Tools", &refs(&tools))?)?;
    if mac {
        menu.append(&submenu(
            app,
            "Window",
            &[
                &PredefinedMenuItem::minimize(app, None)?,
                &PredefinedMenuItem::maximize(app, None)?,
                &sep()?,
                &PredefinedMenuItem::bring_all_to_front(app, None)?,
            ],
        )?)?;
    } else {
        menu.append(&submenu(app, "Help", &[&it("help.about")?])?)?;
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
            let shown = window.is_visible().unwrap_or(false)
                && !window.is_minimized().unwrap_or(false)
                && window.is_focused().unwrap_or(false);
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
    fn ids_map_to_their_actions() {
        assert_eq!(find("view.contacts").unwrap().action, nav(Route::Contacts));
        assert_eq!(find("tray.quit").unwrap().action, Action::Quit);
        assert_eq!(
            find("wallet.lock").unwrap().action,
            Action::Shell(ShellCommand::Lock)
        );
        assert!(find("nope").is_none());
    }
}
