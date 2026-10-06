// dash-qt's menu bar (File, Settings, Window, Help; research 02 §2.1) from
// ShellModel.menus as SwiftCrossUI commands. GtkBackend renders them as the
// window's GtkPopoverMenuBar, AppKitBackend as the global menu bar and
// WinUIBackend as the window's MenuBar; every 0.10 backend supports
// application menus, so no toolbar fallback is needed.
//
// SwiftCrossUI 0.10 limits:
// - menu items have no keyboard shortcuts, so dash-qt's accelerators
//   (Ctrl+Q, Ctrl+Shift+D, …) are not bound;
// - menu items have no tooltips, so a disabled item's reason (`helpText`)
//   is not shown in the menu;
// - `ForEach` produces no menu items, so the items are built as `MenuItem`
//   values and handed over by `ShellMenuItems`.
import Foundation
import SwiftCrossUI
import WalletFeatures
import WalletRuntime

public enum ShellMenus {
    /// The menu bar for `state`; `nil` (live mode before the engine opened)
    /// shows File ▸ Exit only.
    @MainActor
    public static func commands(for state: CrossAppState?, quit: @escaping @MainActor @Sendable () -> Void) -> Commands {
        guard let state else {
            return CommandsBuilder.buildBlock(
                CommandMenu(L10n.Shell.fileMenu) { Button(L10n.Shell.exit) { quit() } })
        }
        let menus = state.shell.menus
        return CommandsBuilder.buildBlock(
            menu(menus, .file, state), menu(menus, .settings, state), menu(menus, .window, state),
            menu(menus, .help, state))
    }

    @MainActor
    private static func menu(_ menus: [MenuModel], _ id: MenuID, _ state: CrossAppState) -> CommandMenu {
        let model = menus.first { $0.id == id }
        return CommandMenu(model?.title ?? id.rawValue) {
            ShellMenuItems(items: items(model?.items ?? [], state))
        }
    }

    /// Menu items in order. Separators at the start, at the end or next to
    /// each other are dropped (GTK draws one per section break).
    @MainActor
    static func items(_ models: [MenuItemModel], _ state: CrossAppState) -> [SwiftCrossUI.MenuItem] {
        var result: [SwiftCrossUI.MenuItem] = []
        var pendingSeparator = false
        for model in models where shows(model) {
            if model.isSeparator {
                pendingSeparator = !result.isEmpty
                continue
            }
            if pendingSeparator {
                result.append(.separator(Divider()))
                pendingSeparator = false
            }
            result.append(item(model, state))
        }
        return result
    }

    @MainActor
    private static func item(_ model: MenuItemModel, _ state: CrossAppState) -> SwiftCrossUI.MenuItem {
        if !model.children.isEmpty {
            var submenu = Menu(model.title) { EmptyView() }
            submenu.items = items(model.children, state)
            return .submenu(submenu)
        }
        guard let command = model.command else {
            return .button(Button(model.title) {}).disabled()
        }
        if let checked = model.isChecked {
            let toggle = Toggle(
                model.title, isOn: bind({ checked }, { _ in Task { await state.perform(command) } }))
            return model.isEnabled ? .toggle(toggle) : .toggle(toggle).disabled()
        }
        let button = SwiftCrossUI.MenuItem.button(Button(model.title) { Task { await state.perform(command) } })
        return model.isEnabled ? button : button.disabled()
    }

    /// Items with no meaning in this app: Minimize (SwiftCrossUI 0.10 cannot
    /// minimize a window) and the tray's Show/Hide.
    private static func shows(_ model: MenuItemModel) -> Bool {
        switch model.command {
        case .minimize?, .showHideWindow?: false
        default: true
        }
    }
}

extension SwiftCrossUI.MenuItem {
    /// The item with the environment's `isEnabled` off; GtkBackend greys its
    /// action out.
    @MainActor
    func disabled() -> SwiftCrossUI.MenuItem {
        .modifiedEnvironment({ self }, { { $0.with(\.isEnabled, false) } })
    }
}

/// Hands prebuilt menu items to `CommandMenu`'s view builder.
struct ShellMenuItems: View {
    let items: [SwiftCrossUI.MenuItem]

    var body: some View { EmptyView() }

    var _asMenuItems: [SwiftCrossUI.MenuItem] { items }
}
