// AppKit services the macOS UI calls: pasteboard, save panel, app
// activation and termination, opening folders in Finder.
#if os(macOS)
import AppKit
import Foundation
import PlatformServices
import UniformTypeIdentifiers

/// The general pasteboard, as plain text.
@MainActor
public enum MacPasteboard {
    public static func copy(_ text: String) {
        let pasteboard = NSPasteboard.general
        pasteboard.clearContents()
        pasteboard.setString(text, forType: .string)
    }

    public static func string() -> String? {
        NSPasteboard.general.string(forType: .string)
    }
}

/// Outcome of a save panel.
public enum MacSaveOutcome: Sendable, Equatable {
    case saved(URL)
    case cancelled
    case failed(String)
}

/// `NSSavePanel` for exports (QT-093 CSV).
@MainActor
public enum MacSavePanel {
    /// Asks for a destination and writes `text` as UTF-8 there.
    public static func save(
        text: String, suggestedName: String, contentType: UTType = .commaSeparatedText, title: String
    ) async -> MacSaveOutcome {
        let panel = NSSavePanel()
        panel.title = title
        panel.nameFieldStringValue = suggestedName
        panel.allowedContentTypes = [contentType]
        panel.canCreateDirectories = true
        let response: NSApplication.ModalResponse
        if let window = NSApplication.shared.keyWindow {
            response = await panel.beginSheetModal(for: window)
        } else {
            response = panel.runModal()
        }
        guard response == .OK, let url = panel.url else { return .cancelled }
        do {
            try PrivateFileSystem.writeFile(Data(text.utf8), to: url, replacing: true)
            return .saved(url)
        } catch {
            return .failed(error.localizedDescription)
        }
    }
}

/// Application-level actions SwiftUI does not expose.
@MainActor
public enum MacApplication {
    /// Brings the app to the front (menu bar extra "Open Dash Wallet").
    public static func activate() {
        NSApplication.shared.activate()
    }

    public static func terminate() {
        NSApplication.shared.terminate(nil)
    }

    /// Reveals a folder in Finder ("Open data folder").
    public static func reveal(_ url: URL) {
        NSWorkspace.shared.activateFileViewerSelecting([url])
    }

    /// Short version string from the main bundle, e.g. "0.1.0 (1)".
    public static var versionString: String {
        let info = Bundle.main.infoDictionary ?? [:]
        let version = info["CFBundleShortVersionString"] as? String ?? "0.0.0"
        let build = info["CFBundleVersion"] as? String
        return build.map { "\(version) (\($0))" } ?? version
    }
}
#endif
