// Menus (QT-015…018): dash-qt's File, Settings, Window and Help menus from
// `ShellModel.menus`, placed the macOS way. "About", "Options…" (Cmd-,) and
// "Exit" (Cmd-Q) move to the app menu, as Qt does on macOS; Minimize stays
// the system's Window ▸ Minimize. Every item routes through
// `MacAppModel.perform(_:)`.
#if os(macOS)
import SwiftUI
import WalletFeatures
import WalletRuntime

struct WalletCommands: Commands {
    let model: MacAppModel

    private var main: MainViewModel? { model.main }
    private var menus: [MenuModel] { model.shell?.menus ?? [] }

    private func items(_ id: MenuID) -> [MenuItemModel] {
        menus.first { $0.id == id }?.items ?? []
    }

    var body: some Commands {
        CommandGroup(replacing: .appInfo) {
            Button(L10n.Shell.about) { model.perform(.about) }
        }
        CommandGroup(replacing: .appSettings) {
            Button(L10n.Shell.options) { model.perform(.options) }
                .keyboardShortcut(",", modifiers: .command)
                .disabled(model.features == nil)
        }

        // File (QT-015).
        CommandGroup(replacing: .newItem) {
            ShellMenuItems(items: items(.file), model: model, skip: Self.appMenuCommands)
        }

        // Settings (QT-016).
        CommandMenu(L10n.Shell.settingsMenu) {
            ShellMenuItems(items: items(.settings), model: model, skip: Self.appMenuCommands)
        }

        // Sidebar shortcuts, renumbered by the visible items (QT-012).
        CommandGroup(after: .sidebar) {
            if let main {
                Divider()
                ForEach(main.visibleSidebarItems) { item in
                    if let number = item.shortcutNumber(with: main.features), number <= 9 {
                        Button(item.title) { main.selectShortcut(number) }
                            .keyboardShortcut(KeyEquivalent(Character(String(number))), modifiers: .command)
                            .disabled(main.needsOnboarding)
                    }
                }
            }
        }

        // Window (QT-017): addresses and the Tools window tabs.
        CommandGroup(before: .windowList) {
            ShellMenuItems(items: items(.window), model: model, skip: Self.appMenuCommands)
            Divider()
        }

        // Help (QT-018).
        CommandGroup(replacing: .help) {
            ShellMenuItems(items: items(.help), model: model, skip: Self.appMenuCommands)
            Divider()
            Link(MacStrings.Menu.help, destination: AboutViewModel.documentationURL)
        }
    }

    /// Commands macOS shows in the app menu or the system Window menu.
    static let appMenuCommands: Set<ShellCommand> = [.about, .options, .exit, .minimize]
}

/// One dash-qt menu's items as SwiftUI menu content: buttons with their
/// shortcuts and help text, check marks, separators and submenus (File ▸
/// Open Wallet). Disabled items stay visible with the reason as help.
struct ShellMenuItems: View {
    let items: [MenuItemModel]
    let model: MacAppModel
    var skip: Set<ShellCommand> = []

    var body: some View {
        ForEach(visible) { item in
            if item.isSeparator {
                Divider()
            } else if !item.children.isEmpty || item.command == nil {
                Menu(item.title) {
                    ShellMenuItems(items: item.children, model: model, skip: skip)
                }
                .help(item.helpText ?? "")
            } else if let command = item.command {
                button(item, command)
            }
        }
    }

    /// Without separators at the start or end, or twice in a row, once the
    /// skipped items are gone.
    private var visible: [MenuItemModel] {
        var result: [MenuItemModel] = []
        for item in items where !(item.command.map(skip.contains) ?? false) {
            if item.isSeparator, result.isEmpty || result.last?.isSeparator == true { continue }
            result.append(item)
        }
        while result.last?.isSeparator == true { result.removeLast() }
        return result
    }

    @ViewBuilder
    private func button(_ item: MenuItemModel, _ command: ShellCommand) -> some View {
        let shortcut = item.shortcut.map(Self.keyboardShortcut)
        if let checked = item.isChecked {
            Toggle(item.title, isOn: Binding(get: { checked }, set: { _ in model.perform(command) }))
                .keyboardShortcut(shortcut)
                .disabled(!item.isEnabled)
                .help(item.helpText ?? "")
        } else {
            Button(item.title) { model.perform(command) }
                .keyboardShortcut(shortcut)
                .disabled(!item.isEnabled)
                .help(item.helpText ?? "")
        }
    }

    static func keyboardShortcut(_ shortcut: KeyShortcut) -> KeyboardShortcut {
        var modifiers: EventModifiers = []
        if shortcut.modifiers.contains(.command) { modifiers.insert(.command) }
        if shortcut.modifiers.contains(.shift) { modifiers.insert(.shift) }
        if shortcut.modifiers.contains(.option) { modifiers.insert(.option) }
        return KeyboardShortcut(KeyEquivalent(Character(shortcut.key)), modifiers: modifiers)
    }
}
#endif
