// Sidebar items, routes and sheets (DESIGN-opus §1.10, QT-012).
import Foundation
import WalletRuntime

/// Sidebar sections in display order. CoinJoin, Masternodes, Governance,
/// Contacts and Explore arrive with later milestones and stay hidden until
/// their feature flag is on.
public enum SidebarItem: String, Sendable, Hashable, CaseIterable, Identifiable {
    case overview, send, receive, transactions, coinJoin, masternodes, governance, contacts, explore

    public var id: String { rawValue }

    /// Whether the section is shown with `features` enabled.
    public func isVisible(with features: FeatureFlags) -> Bool {
        switch self {
        case .overview, .send, .receive, .transactions: true
        case .coinJoin: features.coinJoin
        case .masternodes: features.masternodes
        case .governance: features.governance
        case .contacts: features.contacts
        case .explore: features.explore
        }
    }

    public var title: String {
        switch self {
        case .overview: L10n.Navigation.overview
        case .send: L10n.Navigation.send
        case .receive: L10n.Navigation.receive
        case .transactions: L10n.Navigation.transactions
        case .coinJoin: L10n.Navigation.coinJoin
        case .masternodes: L10n.Navigation.masternodes
        case .governance: L10n.Navigation.governance
        case .contacts: L10n.Navigation.contacts
        case .explore: L10n.Navigation.explore
        }
    }

    /// Visible items in order; Cmd/Alt+1…N follow this order (QT-012).
    public static func visible(with features: FeatureFlags) -> [SidebarItem] {
        allCases.filter { $0.isVisible(with: features) }
    }

    /// The Cmd/Alt+N number of this item, renumbered by the visible items.
    public func shortcutNumber(with features: FeatureFlags) -> Int? {
        Self.visible(with: features).firstIndex(of: self).map { $0 + 1 }
    }
}

/// Sections that later milestones switch on. All are off in M1.
public struct FeatureFlags: Sendable, Hashable {
    public var coinJoin: Bool
    public var masternodes: Bool
    public var governance: Bool
    public var contacts: Bool
    public var explore: Bool

    public init(
        coinJoin: Bool = false, masternodes: Bool = false, governance: Bool = false, contacts: Bool = false,
        explore: Bool = false
    ) {
        self.coinJoin = coinJoin
        self.masternodes = masternodes
        self.governance = governance
        self.contacts = contacts
        self.explore = explore
    }

    public static let m1 = FeatureFlags()
}

/// Navigation requests view models raise; the UI layer performs them.
public enum AppRoute: Sendable, Hashable {
    case section(SidebarItem)
    /// Transactions with this transaction selected (QT-038, QT-063).
    case transaction(txid: String)
    /// Send pre-filled from a payment URI (QT-019, QT-054).
    case send(PaymentURI)
}

/// Windows and sheets opened from menus (QT-015…017).
public enum SheetRoute: Sendable, Hashable {
    case signMessage
    case verifyMessage
    case sendingAddresses
    case receivingAddresses
    case encryptWallet
    case changePassphrase
    case showRecoveryPhrase
    case settings
}
