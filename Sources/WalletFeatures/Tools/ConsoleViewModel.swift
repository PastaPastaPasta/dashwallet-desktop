// Tools ▸ Console (QT-145): dash-qt's local RPC console with redacted
// history, wallet selector, font size and the authorize-and-retry loop for
// commands that spend, sign or reveal.
import Foundation
import Observation
import WalletRuntime

public enum ConsoleEntryKind: Sendable, Hashable {
    case welcome, warning, command, reply, error, info
}

public struct ConsoleEntry: Sendable, Hashable, Identifiable {
    public let id: Int
    public let kind: ConsoleEntryKind
    public let text: String
    public let isJSON: Bool
}

/// The console's flow: one line runs at a time; a line that needs a grant
/// waits for the passphrase, then runs again with the grant.
public enum ConsoleState: Sendable, Hashable {
    case idle
    case executing
    case awaitingPassphrase(GrantPurpose, wallet: WalletID?)
}

@MainActor
@Observable
public final class ConsoleViewModel {
    public static let historyLimit = 50
    public static let fontSizeRange = 4...40
    public nonisolated static let defaultFontSize = 12

    public private(set) var entries: [ConsoleEntry] = []
    /// Redacted lines, oldest first (memory only, as dash-qt).
    public private(set) var history: [String] = []
    public private(set) var state: ConsoleState = .idle
    public private(set) var commands: [ConsoleCommandInfo] = []
    public private(set) var selectedWalletID: WalletID?
    public private(set) var fontSize: Int
    public private(set) var errorMessage: String?

    /// Loaded wallets, for the selector.
    public var wallets: [WalletInfo] { walletState.wallets ?? [] }
    /// The selector shows with two or more wallets (dash-qt).
    public var showsWalletSelector: Bool { wallets.count >= 2 }

    private let console: any ConsoleExecuting
    private let auth: any AuthenticationGating
    private let vault: any VaultProviding
    private let walletState: any WalletStateProviding
    private let desktopPreferences: any DesktopPreferencesStoring
    private var nextID = 0
    private var historyCursor: Int?
    /// The line waiting for a grant; a zeroing buffer, dropped after use.
    private var pendingLine: (any SecretBuffer)?

    public init(
        console: any ConsoleExecuting, auth: any AuthenticationGating, vault: any VaultProviding,
        walletState: any WalletStateProviding, desktopPreferences: any DesktopPreferencesStoring
    ) {
        self.console = console
        self.auth = auth
        self.vault = vault
        self.walletState = walletState
        self.desktopPreferences = desktopPreferences
        let stored = desktopPreferences.desktop.consoleFontSize
        fontSize = Self.fontSizeRange.contains(stored) ? stored : Self.defaultFontSize
        selectedWalletID = walletState.wallets?.first?.id
        clear()
    }

    public convenience init(env: AppEnvironment, m2: M2Services) {
        self.init(
            console: m2.console, auth: env.auth, vault: env.vault, walletState: env.walletState,
            desktopPreferences: m2.desktopPreferences)
    }

    /// Command names for tab completion.
    public func load() async {
        do {
            commands = try await console.commands()
        } catch {
            errorMessage = ErrorText.m2(error.code)
        }
        if let id = selectedWalletID, !wallets.contains(where: { $0.id == id }) {
            selectedWalletID = wallets.first?.id
        }
        if selectedWalletID == nil { selectedWalletID = wallets.first?.id }
    }

    /// Ctrl+L: the welcome text and the anti-scam warning only.
    public func clear() {
        entries = []
        #if os(macOS)
        let clearKey = "⌘L", bigger = "⌘+", smaller = "⌘-"
        #else
        let clearKey = "Ctrl+L", bigger = "Ctrl++", smaller = "Ctrl+-"
        #endif
        append(.welcome, L10n.Tools.welcome(clear: clearKey, bigger: bigger, smaller: smaller))
        append(.warning, L10n.Tools.scamWarning)
    }

    // MARK: Running lines

    /// Echoes and stores the redacted line, then runs it. A line that does
    /// not parse opens dash-qt's error and is not kept.
    public func run(line text: String) async {
        guard state == .idle else { return }
        let trimmed = text.trimmingCharacters(in: .whitespaces)
        guard !trimmed.isEmpty else { return }
        let line = vault.makeSecret(utf8: trimmed)
        let redacted: String
        do {
            redacted = try console.redact(line)
        } catch {
            errorMessage = error.code == .consoleParseError ? L10n.Tools.invalidCommandLine : ErrorText.m2(error.code)
            return
        }
        errorMessage = nil
        history.append(redacted)
        if history.count > Self.historyLimit { history.removeFirst(history.count - Self.historyLimit) }
        historyCursor = nil
        append(.command, redacted)
        state = .executing
        await execute(line, grant: nil, authorizedOnce: false)
    }

    /// The passphrase for a pending `.authorizationRequired` line.
    public func authorize(passphrase: String) async {
        guard case .awaitingPassphrase(let purpose, let wallet) = state, let line = pendingLine else { return }
        state = .executing
        do {
            let grant = try await auth.authorize(
                purpose, wallet: wallet, credential: .passphrase(vault.makeSecret(utf8: passphrase)))
            pendingLine = nil
            await execute(line, grant: grant, authorizedOnce: true)
        } catch {
            // A wrong passphrase keeps the line waiting for another try.
            state = .awaitingPassphrase(purpose, wallet: wallet)
            errorMessage = ErrorText.m2(error.code)
        }
    }

    public func cancelAuthorization() {
        guard case .awaitingPassphrase = state else { return }
        pendingLine = nil
        state = .idle
        append(.error, L10n.Tools.authorizationCancelled)
    }

    private func execute(_ line: any SecretBuffer, grant: AuthGrant?, authorizedOnce: Bool) async {
        do {
            let result = try await console.execute(line, wallet: selectedWalletID, grant: grant)
            switch result {
            case .output(let text, let isJSON):
                append(.reply, text, isJSON: isJSON)
                state = .idle
            case .authorizationRequired(let purpose, let wallet):
                // A grant the engine refused once is not retried in a loop.
                guard !authorizedOnce else {
                    append(.error, L10n.M2Errors.grantInvalid)
                    state = .idle
                    return
                }
                switch auth.requirement(for: purpose) {
                case .none:
                    let grant = try await auth.authorize(purpose, wallet: wallet, credential: .unencrypted)
                    await execute(line, grant: grant, authorizedOnce: true)
                case .passphrase, .quickUnlockOrPassphrase:
                    pendingLine = line
                    state = .awaitingPassphrase(purpose, wallet: wallet)
                }
            }
        } catch {
            append(.error, Self.text(for: error))
            state = .idle
        }
    }

    /// RPC errors print Core's "message (code N)" line; the rest use our copy.
    static func text(for error: ServiceError) -> String {
        switch error.code {
        case .consoleRPCError: error.detail
        case .consoleParseError: L10n.Tools.invalidCommandLine
        case .consoleNotAvailable: L10n.Tools.notAvailableInSPV
        case .consoleWalletRequired: L10n.Tools.walletRequired
        default: ErrorText.m2(error.code)
        }
    }

    // MARK: History, completion, wallet, font

    /// Up arrow: the previous history line.
    public func historyUp() -> String? {
        guard !history.isEmpty else { return nil }
        let index = max((historyCursor ?? history.count) - 1, 0)
        historyCursor = index
        return history[index]
    }

    /// Down arrow: the next line, or empty past the newest.
    public func historyDown() -> String? {
        guard let cursor = historyCursor else { return nil }
        let index = cursor + 1
        guard index < history.count else {
            historyCursor = nil
            return ""
        }
        historyCursor = index
        return history[index]
    }

    /// Tab completion over every command name plus `help-console`.
    public func completions(for prefix: String) -> [String] {
        let names = Set(commands.map(\.name) + ["help", "help-console"])
        let lowered = prefix.lowercased()
        return names.filter { $0.hasPrefix(lowered) }.sorted()
    }

    /// The selector; `nil` runs without any wallet.
    public func selectWallet(_ id: WalletID?) {
        guard id != selectedWalletID else { return }
        selectedWalletID = id
        if let id, let wallet = wallets.first(where: { $0.id == id }) {
            append(.info, L10n.Tools.executingWithWallet(wallet.name))
        } else {
            append(.info, L10n.Tools.executingWithoutWallet)
        }
    }

    public func increaseFontSize() { setFontSize(fontSize + 1) }
    public func decreaseFontSize() { setFontSize(fontSize - 1) }

    private func setFontSize(_ size: Int) {
        let clamped = min(max(size, Self.fontSizeRange.lowerBound), Self.fontSizeRange.upperBound)
        guard clamped != fontSize else { return }
        fontSize = clamped
        var stored = desktopPreferences.desktop
        stored.consoleFontSize = clamped
        do {
            try desktopPreferences.update(stored)
        } catch {
            errorMessage = L10n.Settings.settingsNotSaved
        }
    }

    private func append(_ kind: ConsoleEntryKind, _ text: String, isJSON: Bool = false) {
        entries.append(ConsoleEntry(id: nextID, kind: kind, text: text, isJSON: isJSON))
        nextID += 1
    }
}
