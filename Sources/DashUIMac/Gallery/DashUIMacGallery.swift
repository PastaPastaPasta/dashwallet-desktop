// Visual QA gallery: every DashUIMac component with sample values, in light and dark appearance.
// The same samples drive Tests/DashUIMacSnapshotTests.
#if os(macOS)
import DesignTokens
import SwiftUI

/// One gallery entry: a component rendered with fixed sample values.
public struct DashUIMacGallerySample: Identifiable, Sendable {
    public let id: String
    public let title: String
    /// Width the sample is laid out at.
    public let width: CGFloat
    private let make: @MainActor @Sendable () -> AnyView

    public init<Content: View>(_ id: String, title: String, width: CGFloat = 360,
                               @ViewBuilder content: @escaping @MainActor @Sendable () -> Content) {
        self.id = id
        self.title = title
        self.width = width
        self.make = { AnyView(content()) }
    }

    @MainActor public var view: AnyView { make() }
}

/// Every component in a light and a dark column.
public struct DashUIMacGallery: View {
    public init() {}

    public var body: some View {
        ScrollView {
            HStack(alignment: .top, spacing: 0) {
                column(.light)
                column(.dark)
            }
        }
        .frame(minWidth: 860, minHeight: 600)
    }

    private func column(_ scheme: ColorScheme) -> some View {
        VStack(alignment: .leading, spacing: DashSpacing.xxl) {
            Text(scheme == .light ? "Light" : "Dark")
                .dashFont(.title2)
                .foregroundStyle(Color.dash.primaryText)
            ForEach(Self.samples) { sample in
                VStack(alignment: .leading, spacing: DashSpacing.s) {
                    Text(sample.title)
                        .dashFont(.footnoteMedium)
                        .foregroundStyle(Color.dash.secondaryText)
                    sample.view
                        .frame(width: sample.width, alignment: .leading)
                }
            }
        }
        .padding(DashSpacing.xxl)
        .frame(maxWidth: .infinity, alignment: .topLeading)
        .background(Color.dash.primaryBackground)
        .environment(\.colorScheme, scheme)
    }
}

// MARK: - Samples

extension DashUIMacGallery {
    /// The gallery entries in display order. Nonisolated so test arguments can list them; the
    /// views themselves are still built on the main actor.
    public nonisolated static let samples: [DashUIMacGallerySample] = [
        DashUIMacGallerySample("dash-button", title: "DashButton") {
            VStack(alignment: .leading, spacing: DashSpacing.s) {
                HStack {
                    DashButton(text: "Send", size: .large, style: .filledBlue)
                    DashButton(text: "Receive", size: .medium, style: .tintedBlue)
                    DashButton(text: "Cancel", size: .medium, style: .strokeGray)
                }
                HStack {
                    DashButton(text: "Delete", size: .small, style: .filledRed)
                    DashButton(text: "Disabled", isEnabled: false, size: .small, style: .filledBlue)
                    DashButton(text: "Plain", size: .small, style: .plainBlue)
                }
            }
        },
        DashUIMacGallerySample("menu-item", title: "MenuItem") {
            VStack(spacing: 0) {
                MenuItem(leadingIcon: DashIcon.Menu.send.source, title: "Send", helpText: "To an address or contact")
                MenuItem(leadingIcon: DashIcon.Menu.localCurrency.source, title: "Local currency",
                         accessory: .text("USD"))
                MenuItem(leadingIcon: DashIcon.Menu.receive.source, isEnabled: false,
                         disabledLeadingIcon: DashIcon.Menu.receiveDisabled.source, title: "Receive (disabled)")
            }
            .modifier(MenuViewModifier())
        },
        DashUIMacGallerySample("transaction-row", title: "TransactionRow (TransactionView)") {
            VStack(spacing: 0) {
                TransactionRow(icon: DashIcon.Transaction.received.source, title: "Received",
                               subtitle: "10:42", details: "InstantSend", dashAmount: 125_000_000,
                               amountSign: .always, fiat: "$37.50")
                TransactionRow(icon: DashIcon.Transaction.sent.source, title: "Sent", subtitle: "Yesterday",
                               dashAmount: -4_200_000, amountSign: .always, fiat: "$1.26")
                TransactionRow(icon: DashIcon.Transaction.mining.source, title: "Mined", subtitle: "Oct 1",
                               dashAmount: 89_000_000, trailingStatusText: "Locked")
            }
            .modifier(MenuViewModifier())
        },
        DashUIMacGallerySample("toast", title: "Toast") {
            VStack(alignment: .leading, spacing: DashSpacing.s) {
                Toast(style: .success, message: "Transaction sent", onDismiss: {})
                Toast(style: .copied, message: "Address copied")
                Toast(style: .warning, message: "Some coins are not available")
                Toast(style: .loading, message: "Syncing…")
            }
        },
        DashUIMacGallerySample("bottom-sheet", title: "BottomSheet (sheet chrome)") {
            BottomSheet.selfSizing(title: "Send", showBackButton: .constant(true)) {
                Text("Sheet content")
                    .dashFont(.subhead)
                    .foregroundStyle(Color.dash.primaryText)
                    .padding(DashSpacing.xl)
            }
        },
        DashUIMacGallerySample("switches-radios", title: "DashSwitch, SwitchView, RadioButtonRow") {
            VStack(alignment: .leading, spacing: DashSpacing.s) {
                HStack(spacing: DashSpacing.m) {
                    DashSwitch(isOn: .constant(true), accessibilityLabel: "On")
                    DashSwitch(isOn: .constant(false), accessibilityLabel: "Off")
                    SwitchView(isOn: .constant(true))
                    SwitchView(isOn: .constant(false))
                }
                VStack(spacing: 0) {
                    RadioButtonRow(title: "Standard fee", subtitle: "Confirms in about 2.5 minutes",
                                   isSelected: true, action: {})
                    RadioButtonRow(title: "Custom fee", isSelected: false, action: {})
                }
                .modifier(MenuViewModifier())
            }
        },
        DashUIMacGallerySample("dash-amount", title: "DashAmount") {
            VStack(alignment: .leading, spacing: DashSpacing.s) {
                DashAmount(amount: 125_000_000, fontSize: 24, weight: .semibold, sign: .always)
                DashAmount(amount: -4_200_000, sign: .always)
                DashAmount(amount: 1)
            }
        },
        DashUIMacGallerySample("system-message", title: "SystemMessageView") {
            SystemMessageView(title: "Back up your wallet", subtitle: "Write down the recovery phrase.",
                              buttonName: "Back up", onAction: {})
        },
        DashUIMacGallerySample("badge", title: "Badge") {
            HStack(spacing: DashSpacing.s) {
                Badge.instantSend()
                Badge.chainLocked()
                Badge.unconfirmed()
                Badge("Testnet", tone: .error)
                Badge("Mixed", tone: .neutral, icon: .token(.coinjoinProtected))
            }
        },
        DashUIMacGallerySample("sidebar-row", title: "SidebarRow", width: 240) {
            VStack(spacing: 2) {
                SidebarRow(title: "Home", icon: .token(.tabHome), isSelected: true)
                SidebarRow(title: "Transactions", icon: .token(.txAll), badgeText: "3")
                SidebarRow(title: "CoinJoin", icon: .token(.coinjoin))
                SidebarRow(title: "Governance", icon: .token(.voting))
            }
        },
        DashUIMacGallerySample("balance-header", title: "BalanceHeader", width: 520) {
            VStack(spacing: DashSpacing.m) {
                BalanceHeader(
                    title: "Balance", amount: "12.34567", unit: "DASH", fiat: "$3,703.70",
                    onToggleHidden: {},
                    breakdown: [
                        .init(label: "Available", amount: "12.30000"),
                        .init(label: "Pending", amount: "0.04567"),
                        .init(label: "Mixed", amount: "8.00000"),
                    ])
                BalanceHeader(title: "Balance", amount: "12.34567", unit: "DASH", fiat: "$3,703.70",
                              isHidden: true, onToggleHidden: {})
            }
        },
        DashUIMacGallerySample("amount-field", title: "AmountField") {
            VStack(spacing: DashSpacing.m) {
                AmountField(label: "Amount", text: .constant("1.5"), unit: "DASH", secondaryText: "≈ $450.00",
                            onMax: {})
                AmountField(label: "Amount", text: .constant("abc"), unit: "DASH",
                            errorText: "Enter a valid amount")
            }
        },
        DashUIMacGallerySample("passphrase-field", title: "PassphraseField") {
            VStack(spacing: DashSpacing.m) {
                PassphraseField(label: "Passphrase", text: .constant("correct horse"), strength: .fair)
                PassphraseField(label: "Confirm passphrase", text: .constant("x"),
                                errorText: "Passphrases do not match", isRevealable: false)
                ForEach(PassphraseStrength.allCases, id: \.self) { PassphraseStrengthMeter(strength: $0) }
            }
        },
        DashUIMacGallerySample("address-field", title: "AddressFieldView") {
            AddressFieldView(text: .constant("XpESxaUmonkq8RaLLp46Brx2K39ggQe226"), label: "Pay to",
                             placeholder: "Dash address", hasError: false, onScanQR: {})
        },
        DashUIMacGallerySample("search-bar", title: "SearchBar") {
            SearchBar(text: .constant(""), placeholder: "Search transactions")
        },
        DashUIMacGallerySample("qr-view", title: "QRView", width: 200) {
            HStack(spacing: DashSpacing.m) {
                QRView(size: GallerySampleData.qrSize, modules: GallerySampleData.qrModules,
                       accessibilityLabel: "Receive address QR code")
                    .frame(width: 120, height: 120)
                QRView(size: 3, modules: [true], accessibilityLabel: "Invalid QR code")
                    .frame(width: 60, height: 60)
            }
        },
        DashUIMacGallerySample("status-bar", title: "StatusBar", width: 640) {
            StatusBar(message: "Synchronizing with network… 58%", progress: 0.58, items: [
                StatusBarItem(id: "unit", text: "DASH", accessibilityLabel: "Unit: DASH"),
                StatusBarItem(id: "hd", text: "HD", accessibilityLabel: "HD wallet"),
                StatusBarItem(id: "lock", icon: .token(.security), accessibilityLabel: "Wallet is locked",
                              tone: .success),
                StatusBarItem(id: "peers", icon: .token(.connections), text: "8",
                              accessibilityLabel: "8 active connections"),
                StatusBarItem(id: "sync", icon: .token(.syncing), accessibilityLabel: "Syncing",
                              tone: .warning),
            ])
        },
        DashUIMacGallerySample("data-table", title: "DataTable", width: 560) {
            // Not scrollable: ImageRenderer draws scroll views blank.
            GalleryDataTable()
        },
        DashUIMacGallerySample("illustrations", title: "Illustrations") {
            HStack(spacing: DashSpacing.m) {
                SuccessIllustration()
                ErrorIllustration()
            }
        },
    ]
}

// MARK: - Sample data

/// Fixed values used by the gallery and the snapshot tests.
enum GallerySampleData {
    struct TxRow: Identifiable {
        let id: Int
        let date: String
        let type: String
        let label: String
        let amount: String
        let duffs: Int64
        let lock: String
    }

    static let rows: [TxRow] = [
        TxRow(id: 1, date: "2026-10-05 10:42", type: "Received", label: "Salary", amount: "+1.25000", duffs: 125_000_000,
              lock: "IS"),
        TxRow(id: 2, date: "2026-10-04 18:03", type: "Sent", label: "Coffee", amount: "-0.04200", duffs: -4_200_000,
              lock: "CL"),
        TxRow(id: 3, date: "2026-10-03 09:15", type: "Mixing", label: "", amount: "-0.00001", duffs: -1_000,
              lock: "CL"),
        TxRow(id: 4, date: "2026-10-01 22:47", type: "Received", label: "Refund", amount: "+0.50000",
              duffs: 50_000_000, lock: ""),
    ]

    /// A 21×21 version-1-shaped pattern: finder squares in three corners, timing lines, and a
    /// deterministic fill. It is not a decodable QR code; it only exercises the renderer.
    static let qrSize = 21
    static let qrModules: [Bool] = {
        let n = qrSize
        var m = [Bool](repeating: false, count: n * n)
        func finder(_ ox: Int, _ oy: Int) {
            for y in 0..<7 {
                for x in 0..<7 {
                    let edge = x == 0 || x == 6 || y == 0 || y == 6
                    let core = (2...4).contains(x) && (2...4).contains(y)
                    m[(oy + y) * n + ox + x] = edge || core
                }
            }
        }
        finder(0, 0)
        finder(n - 7, 0)
        finder(0, n - 7)
        for i in 8..<(n - 8) {
            m[6 * n + i] = i.isMultiple(of: 2)
            m[i * n + 6] = i.isMultiple(of: 2)
        }
        for y in 9..<n {
            for x in 9..<n {
                m[y * n + x] = (x * 7 + y * 3 + x * y).isMultiple(of: 3)
            }
        }
        return m
    }()
}

private struct GalleryDataTable: View {
    @State private var selection: Set<Int> = [2]
    @State private var sortOrder: DataTableSortOrder? = DataTableSortOrder(columnID: "date", ascending: false)

    var body: some View {
        DataTable(
            rows: GallerySampleData.rows,
            columns: [
                DataTableColumn("Status", id: "lock", width: .fixed(84)) { (row: GallerySampleData.TxRow) in
                    switch row.lock {
                    case "IS": Badge.instantSend("IS")
                    case "CL": Badge.chainLocked("CL")
                    default: Badge.unconfirmed("0/6")
                    }
                },
                DataTableColumn("Date", id: "date", width: .fixed(130), value: \.date),
                DataTableColumn("Type", id: "type", width: .fixed(70), value: \.type),
                DataTableColumn("Label", id: "label", value: \.label),
                DataTableColumn("Amount (DASH)", id: "amount", width: .fixed(100), alignment: .trailing,
                                sortBy: { $0.duffs < $1.duffs }) { row in
                    Text(row.amount)
                        .font(DashTextStyle.footnote.font.monospacedDigit())
                        .foregroundStyle(row.duffs < 0 ? Color.dash.primaryText : Color.dash.successText)
                },
            ],
            selection: $selection,
            sortOrder: $sortOrder,
            contextMenu: { ids in
                [DataTableMenuAction(title: ids.count == 1 ? "Copy transaction ID" : "Copy \(ids.count) IDs") {}]
            },
            isScrollable: false
        )
    }
}
#endif
