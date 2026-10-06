// AppKit panels and window actions the M2 screens need: open panels for
// files and directories, a save panel that only picks a destination, the
// main window's show/hide and minimize, and the Dock menu (QT-028/029).
#if os(macOS)
import AppKit
import Foundation
import UniformTypeIdentifiers

/// `NSOpenPanel` for imports, PSBT files and the data directory.
@MainActor
public enum MacOpenPanel {
    /// Asks for one existing file; `nil` when cancelled. Without content
    /// types every file can be chosen.
    public static func chooseFile(title: String, contentTypes: [UTType] = []) async -> URL? {
        let panel = NSOpenPanel()
        panel.title = title
        panel.canChooseFiles = true
        panel.canChooseDirectories = false
        panel.allowsMultipleSelection = false
        if !contentTypes.isEmpty { panel.allowedContentTypes = contentTypes }
        return await run(panel)
    }

    /// Asks for a directory (the data-directory chooser, QT-004).
    public static func chooseDirectory(title: String) async -> URL? {
        let panel = NSOpenPanel()
        panel.title = title
        panel.canChooseFiles = false
        panel.canChooseDirectories = true
        panel.canCreateDirectories = true
        panel.allowsMultipleSelection = false
        return await run(panel)
    }

    private static func run(_ panel: NSOpenPanel) async -> URL? {
        let response: NSApplication.ModalResponse
        if let window = NSApplication.shared.keyWindow {
            response = await panel.beginSheetModal(for: window)
        } else {
            response = panel.runModal()
        }
        return response == .OK ? panel.url : nil
    }
}

extension MacSavePanel {
    /// Asks for a destination without writing anything: the engine writes
    /// backups, Dash Core exports and logs itself. `nil` when cancelled.
    public static func chooseDestination(suggestedName: String, title: String, contentType: UTType? = nil) async -> URL? {
        let panel = NSSavePanel()
        panel.title = title
        panel.nameFieldStringValue = suggestedName
        if let contentType { panel.allowedContentTypes = [contentType] }
        panel.canCreateDirectories = true
        let response: NSApplication.ModalResponse
        if let window = NSApplication.shared.keyWindow {
            response = await panel.beginSheetModal(for: window)
        } else {
            response = panel.runModal()
        }
        return response == .OK ? panel.url : nil
    }

    /// Asks for a destination and writes `data` there (PSBT binary).
    public static func save(data: Data, suggestedName: String, title: String) async -> MacSaveOutcome {
        guard let url = await chooseDestination(suggestedName: suggestedName, title: title) else { return .cancelled }
        do {
            try data.write(to: url, options: .atomic)
            return .saved(url)
        } catch {
            return .failed(error.localizedDescription)
        }
    }
}

extension MacApplication {
    /// Window ▸ Minimize for the key window.
    public static func minimizeKeyWindow() {
        NSApplication.shared.keyWindow?.miniaturize(nil)
    }

    /// The Dock / menu bar "Show / Hide" (QT-029): hides the app when one of
    /// its windows is visible and active, otherwise brings it forward.
    public static func toggleVisibility() {
        let app = NSApplication.shared
        if app.isActive, app.windows.contains(where: { $0.isVisible && !$0.isMiniaturized }) {
            app.hide(nil)
        } else {
            app.unhide(nil)
            app.activate()
        }
    }
}

/// One entry of the Dock menu.
public struct MacDockMenuItem {
    public let title: String
    public let isEnabled: Bool
    public let isSeparator: Bool
    public let action: @MainActor () -> Void

    public init(title: String, isEnabled: Bool = true, action: @escaping @MainActor () -> Void) {
        self.title = title
        self.isEnabled = isEnabled
        self.isSeparator = false
        self.action = action
    }

    public static var separator: MacDockMenuItem {
        MacDockMenuItem(separatorItem: ())
    }

    private init(separatorItem: Void) {
        title = ""
        isEnabled = false
        isSeparator = true
        action = {}
    }
}

/// Builds the `NSMenu` for `applicationDockMenu(_:)` (dash-qt's dock menu,
/// QT-029). The menu keeps its targets alive.
@MainActor
public final class MacDockMenu: NSObject {
    private var actions: [@MainActor () -> Void] = []

    public override init() {}

    public func menu(_ items: [MacDockMenuItem]) -> NSMenu {
        actions = []
        let menu = NSMenu()
        menu.autoenablesItems = false
        for item in items {
            if item.isSeparator {
                menu.addItem(.separator())
                continue
            }
            let entry = NSMenuItem(title: item.title, action: #selector(run(_:)), keyEquivalent: "")
            entry.target = self
            entry.tag = actions.count
            entry.isEnabled = item.isEnabled
            actions.append(item.action)
            menu.addItem(entry)
        }
        return menu
    }

    @objc private func run(_ sender: NSMenuItem) {
        guard actions.indices.contains(sender.tag) else { return }
        actions[sender.tag]()
    }
}
#endif
