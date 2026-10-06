// The state one window of the SwiftCrossUI app holds: the composition's
// environment and M2 services, the main view model, the shell (menus) and
// the view models of the tool pages, which MainViewModel does not own.
import Foundation
import Observation
import WalletFeatures
import WalletRuntime

/// Pages that replace the sidebar section in the detail area: the M1 tool
/// pages and dash-qt's M2 windows and dialogs. SwiftCrossUI has no modal
/// window that AT-SPI sees inside the main window, so each one is a page with
/// its own Close/OK buttons.
public enum ToolPage: Sendable, Hashable {
    case addressBook(AddressPurpose)
    case signVerify
    case settings
    case options
    case coinSelection
    case psbt
    case tools(ToolsTab)
    case wallets
    case security
    case about
    case commandLineOptions
    case openURI
    case createWallet

    init(_ sheet: SheetRoute) {
        switch sheet {
        case .signMessage, .verifyMessage: self = .signVerify
        case .sendingAddresses: self = .addressBook(.send)
        case .receivingAddresses: self = .addressBook(.receive)
        case .encryptWallet, .changePassphrase, .showRecoveryPhrase, .settings: self = .settings
        }
    }

    /// `--page` names of the pages a smoke test can open first.
    public init?(pageName: String) {
        switch pageName {
        case "address-book": self = .addressBook(.send)
        case "sign-verify": self = .signVerify
        case "settings": self = .settings
        case "options": self = .options
        case "coin-selection": self = .coinSelection
        case "psbt": self = .psbt
        case "tools-information": self = .tools(.information)
        case "tools-console": self = .tools(.console)
        case "tools-peers": self = .tools(.peers)
        case "tools-repair": self = .tools(.repair)
        case "wallets": self = .wallets
        case "security": self = .security
        case "about": self = .about
        default: return nil
        }
    }

    public static let pageNames = [
        "address-book", "sign-verify", "settings", "options", "coin-selection", "psbt", "tools-information",
        "tools-console", "tools-peers", "tools-repair", "wallets", "security", "about",
    ]
}

/// What the composition root knows about the OS services the M2 screens
/// should be honest about.
public struct CrossPlatformCapabilities: Sendable, Hashable {
    /// The clipboard service can read and write (Linux: `wl-copy`/`xclip`
    /// is installed). Without it, copy actions show the text instead.
    public var clipboard: Bool
    /// A tray icon backend exists (dw-desktop has none yet).
    public var tray: Bool

    public init(clipboard: Bool, tray: Bool) {
        self.clipboard = clipboard
        self.tray = tray
    }
}

@MainActor
@Observable
public final class CrossAppState {
    public let env: AppEnvironment
    public let m2: M2Services
    public let main: MainViewModel
    /// dash-qt's menus, window title and status icons (QT-011…022).
    public let shell: ShellModel
    public let capabilities: CrossPlatformCapabilities
    /// Shown in the status row (for example the demo-mode notice).
    public let notice: String?
    /// The page shown instead of the sidebar section; `nil` follows
    /// `main.sheet` (routes raised by the M1 view models), then the section.
    public var page: ToolPage?
    /// The last copy action's outcome (status line under the page title).
    var copyMessage: String?
    /// The app's own command-line usage (Help ▸ Command-line options).
    @ObservationIgnored public var appUsage = ""
    /// Exit (File menu): runs the shutdown page, then the composition's quit.
    @ObservationIgnored public var quitApplication: @MainActor () -> Void = { exit(0) }
    /// Asks the composition root for a file to open (File ▸ Load PSBT from
    /// file); the window installs the toolkit's dialog.
    @ObservationIgnored var chooseFile: (@MainActor (String) async -> URL?)?

    @ObservationIgnored private var addressBookModel: (network: DashNetwork, model: AddressBookViewModel)?
    @ObservationIgnored private var signVerifyModel: (network: DashNetwork, model: SignVerifyViewModel)?
    @ObservationIgnored private var transactionsModel: (network: DashNetwork, model: TransactionsViewModel)?
    @ObservationIgnored private var coinControlModel: (network: DashNetwork, model: CoinControlViewModel)?
    @ObservationIgnored private var onboardingModel: OnboardingViewModel?
    @ObservationIgnored private(set) lazy var options = OptionsViewModel(env: env, m2: m2)
    @ObservationIgnored private(set) lazy var psbt = PSBTViewModel(env: env, m2: m2)
    @ObservationIgnored private(set) lazy var information = InformationViewModel(env: env, m2: m2)
    @ObservationIgnored private(set) lazy var console = ConsoleViewModel(env: env, m2: m2)
    @ObservationIgnored private(set) lazy var peers = PeersViewModel(sync: env.sync, moderation: m2.peerModeration)
    @ObservationIgnored private(set) lazy var repair = RepairViewModel(env: env, m2: m2)
    @ObservationIgnored private(set) lazy var wallets = WalletManagementViewModel(env: env, m2: m2)
    @ObservationIgnored private(set) lazy var security = SecurityViewModel(env: env, m2: m2)
    @ObservationIgnored private(set) lazy var about = AboutViewModel(env: env, m2: m2)
    @ObservationIgnored private(set) lazy var shutdown = ShutdownViewModel(coordinator: m2.shutdown)

    public init(
        env: AppEnvironment, m2: M2Services, main: MainViewModel, capabilities: CrossPlatformCapabilities,
        notice: String? = nil
    ) {
        self.env = env
        self.m2 = m2
        self.main = main
        self.capabilities = capabilities
        self.notice = notice
        shell = ShellModel(env: env, m2: m2, features: main.features)
    }

    /// The page in the detail area: an explicit page, else the M1 sheet route.
    var currentPage: ToolPage? {
        page ?? main.sheet.map(ToolPage.init)
    }

    /// Shows `page` in the detail area.
    public func open(_ page: ToolPage) {
        main.sheet = nil
        copyMessage = nil
        self.page = page
    }

    /// Back to the selected sidebar section.
    public func closePage() {
        main.sheet = nil
        copyMessage = nil
        page = nil
        onboardingModel = nil
    }

    // MARK: Page view models

    /// The address book of the active network, created on first use with
    /// `purpose`; afterwards the page's own picker sets the purpose.
    func addressBook(purpose: AddressPurpose) -> AddressBookViewModel? {
        guard let network = main.network else { return nil }
        if let cached = addressBookModel, cached.network == network { return cached.model }
        let model = AddressBookViewModel(env: env, network: network, purpose: purpose)
        addressBookModel = (network, model)
        return model
    }

    /// Sign/verify of the active network, created on first use.
    func signVerify() -> SignVerifyViewModel? {
        guard let network = main.network else { return nil }
        if let cached = signVerifyModel, cached.network == network { return cached.model }
        let model = SignVerifyViewModel(env: env, network: network)
        signVerifyModel = (network, model)
        return model
    }

    /// The Transactions page with the M2 actions (abandon, resend, extras,
    /// engine CSV, explorer links), per network.
    func transactions() -> TransactionsViewModel? {
        guard let network = main.network else { return nil }
        if let cached = transactionsModel, cached.network == network { return cached.model }
        transactionsModel?.model.stop()
        let model = TransactionsViewModel(env: env, m2: m2, network: network, features: main.features)
        model.start()
        transactionsModel = (network, model)
        return model
    }

    /// Coin control of the active network (QT-068…074), shared by the Send
    /// page's panel and the Coin Selection page.
    func coinControl() -> CoinControlViewModel? {
        guard let network = main.network else { return nil }
        if let cached = coinControlModel, cached.network == network { return cached.model }
        let model = CoinControlViewModel(env: env, m2: m2)
        coinControlModel = (network, model)
        return model
    }

    /// File ▸ Create Wallet: the onboarding flow for one more wallet.
    func createWalletFlow() -> OnboardingViewModel {
        if let onboardingModel { return onboardingModel }
        let model = OnboardingViewModel(env: env)
        onboardingModel = model
        return model
    }

    // MARK: Shell commands

    /// Runs a menu command: the shell performs what it owns (sections, open
    /// and close wallet, lock, discreet mode, automatic backups) and hands
    /// back the ones that open UI, which become pages here.
    public func perform(_ command: ShellCommand) async {
        await shell.perform(command)
        guard let presented = shell.pendingPresentation else {
            if case .section(let item) = command {
                closePage()
                main.selection = item
            }
            return
        }
        shell.presentationHandled()
        switch presented {
        case .createWallet: open(.createWallet)
        case .backupWallet, .restoreWallet: open(.wallets)
        case .openURI: open(.openURI)
        case .signMessage: show(.signMessage)
        case .verifyMessage: show(.verifyMessage)
        case .loadPSBTFromFile:
            open(.psbt)
            if let chooseFile, let url = await chooseFile(L10n.Shell.loadPSBTFromFile) {
                await psbt.load(file: url)
            }
        case .loadPSBTFromClipboard:
            open(.psbt)
            await psbt.loadFromClipboard()
        case .exit: await quit()
        case .encryptWallet, .changePassphrase, .showRecoveryPhrase, .unlockWallet: show(.settings)
        case .options: open(.options)
        case .sendingAddresses: show(.sendingAddresses)
        case .receivingAddresses: show(.receivingAddresses)
        case .tools(let tab): open(.tools(tab))
        case .commandLineOptions: open(.commandLineOptions)
        case .about: open(.about)
        // Handled by the shell itself, disabled there (no feature yet), or
        // without meaning in a single-window app without a tray.
        case .section, .openWallet, .closeWallet, .closeAllWallets, .migrateWallet, .openDebugLog,
            .openConfigurationFile, .showAutomaticBackups, .lockWallet, .toggleDiscreetMode, .minimize,
            .coinJoinInformation, .showHideWindow:
            break
        }
    }

    /// Shows an M1 tool page through MainViewModel's sheet route.
    private func show(_ sheet: SheetRoute) {
        page = nil
        copyMessage = nil
        main.sheet = sheet
    }

    /// File ▸ Exit: the shutdown page (QT-008) while the engine stops, then
    /// the composition's quit.
    public func quit() async {
        main.stop()
        shell.stop()
        await shutdown.quit()
        quitApplication()
    }

    // MARK: Helpers

    /// An amount in the display unit, with the unit name (detail panes).
    func format(_ amount: Amount) -> String {
        env.amounts.format(amount, unit: env.settings.display.unit, style: .withUnit(plusSign: false, separators: .always))
    }

    /// Puts `text` on the clipboard, or says that there is none and shows
    /// the text to copy by hand (Linux without `wl-copy`/`xclip`).
    func copy(_ text: String, what: String) {
        guard !text.isEmpty else {
            copyMessage = CrossStrings.nothingToCopy(what)
            return
        }
        if capabilities.clipboard {
            m2.clipboard.setString(text)
            copyMessage = CrossStrings.copied(what)
        } else {
            copyMessage = CrossStrings.noClipboard(what, text)
        }
    }

    /// Performs a route a page raised and clears it. Transaction routes also
    /// open the transaction on the M2 Transactions page.
    func follow(_ route: AppRoute?) async {
        guard let route else { return }
        closePage()
        await main.navigate(route)
        if case .transaction(let txid) = route { await transactions()?.reveal(txid: txid) }
    }
}

/// Holds the window state once it exists, for the scene's menu bar and
/// window title: the demo builds it at launch, the live app after the data
/// directory and the engine opened.
@MainActor
@Observable
public final class CrossAppHost {
    public var state: CrossAppState?

    public init(state: CrossAppState? = nil) {
        self.state = state
    }
}
