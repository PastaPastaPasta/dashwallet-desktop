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
        DashUIMacGallerySample("amount-text", title: "AmountText, NetworkCapsule", width: 360) {
            VStack(alignment: .leading, spacing: DashSpacing.s) {
                AmountText("30.39698287", unit: .glyph(spoken: "DASH"), size: 34, weight: .bold, glyphFactor: 0.7)
                AmountText("+0.20", unit: .glyph(spoken: "tDASH"))
                AmountText("250.00374", unit: .name("mDASH"), weight: .regular)
                NetworkCapsule("Testnet")
            }
            .foregroundStyle(Color.role.textPrimary)
        },
        DashUIMacGallerySample("balance-hero", title: "BalanceHero, ShortcutCard", width: 720) {
            BalanceHero(
                network: "Testnet", caption: BalanceHeroCaption(text: "Syncing Balance"), amount: "30.39698287",
                unit: .glyph(spoken: "tDASH"), isHidden: false, unavailableText: "Balance unavailable",
                breakdown: [
                    BalanceBreakdownCell(
                        id: "available", title: "Available", subtitle: "Spendable now", amount: "30.19698287",
                        unit: .glyph(spoken: "tDASH"), help: nil),
                    BalanceBreakdownCell(
                        id: "pending", title: "Pending", subtitle: "Awaiting confirmation", amount: nil,
                        unit: .none, help: "Not available until sync completes"),
                ],
                onToggleHidden: {}, toggleLabel: "Hide balance"
            ) {
                ShortcutCard {
                    ShortcutItem(title: "Receive", icon: .token(.shortcutReceive)) {}
                    ShortcutItem(title: "Send", icon: .token(.shortcutSend)) {}
                    ShortcutItem(title: "Scan QR", icon: .token(.shortcutScanQR)) {}
                    ShortcutItem(title: "Back up", icon: .token(.shortcutBackup)) {}
                }
            }
            .background(Color.role.canvas)
        },
        DashUIMacGallerySample("history", title: "HistoryHeader, TransactionGroupCard, DashTransactionRow", width: 560) {
            VStack(spacing: DashSpacing.m) {
                HistoryHeader(title: "History", syncText: "Syncing 47.0%", onSync: {}, filterTitle: "Filter", onFilter: {})
                TransactionGroupCard(day: "Today", weekday: "Tuesday") {
                    DashTransactionRow(
                        icon: .token(.txReceived), title: "Received", subtitle: "01:40",
                        chip: .init(text: "Pending"), amount: "+0.20", unit: .glyph(spoken: "DASH"))
                    DashTransactionRow(
                        icon: .token(.txSent), title: "Alice", subtitle: "00:12", amount: "-0.50000226",
                        unit: .glyph(spoken: "DASH"), isSelected: true)
                    DashTransactionRow(
                        icon: .token(.txError), title: "Sent", subtitle: "00:02",
                        chip: .init(text: "Conflicted", isProblem: true), amount: "[-1.00]",
                        unit: .glyph(spoken: "DASH"), isDimmed: true)
                    DashTransactionRow(
                        icon: .token(.txMining), title: "Mined", subtitle: "23:59", amount: "+2.25",
                        unit: .name("mDASH"), trailingStatus: "Locked")
                }
            }
            .padding(DashSpacing.m)
            .background(Color.role.canvas)
        },
        DashUIMacGallerySample("menu-card", title: "MenuCard, MenuRow, DetailRow, CopyRow", width: 520) {
            VStack(spacing: DashSpacing.m) {
                MenuCard(title: "Security", footer: "Rows are 56 pt high with 30 pt icons.") {
                    MenuRow(icon: .token(.security), title: "Wallet encryption") {
                        Text("Encrypted").dashFont(.subhead).foregroundStyle(Color.role.textSecondary)
                    }
                    MenuRow(icon: .token(.autohideBalance), title: "Autohide balance", help: "Hide amounts on start") {
                        Toggle("Autohide balance", isOn: .constant(true)).toggleStyle(.switch).labelsHidden()
                    }
                    MenuActionRow(icon: .token(.resetWallet), title: "Wipe wallet", isDestructive: true) {}
                }
                MenuCard {
                    DetailRow("Status", value: "0/unconfirmed, in memory pool")
                    CopyRow(
                        "Transaction ID", value: "57143a022c2d67ba52c6251f28b96e4834786fa5bbb0abffacab6a7a943469a4",
                        isTechnical: true, copyLabel: "Copy transaction ID")
                }
            }
            .padding(DashSpacing.m)
            .background(Color.role.canvas)
        },
        DashUIMacGallerySample("states", title: "SegmentedControl, EmptyState, LoadingState, ProgressBar, WizardHeader", width: 520) {
            VStack(alignment: .leading, spacing: DashSpacing.m) {
                DashSegmentedControl([(0, "List"), (1, "Table")], selection: .constant(0))
                EmptyState(icon: .token(.txAll), title: "There are no transactions to display") {
                    Button("Receive Dash") {}.buttonStyle(.dash(.filledBlue, .medium))
                }
                .dashCard(padding: nil)
                LoadingState("Loading transactions")
                DashProgressBar(value: 0.47)
                WizardHeader(stepText: "Step 2 of 9 · Collateral", title: "Choose the collateral", step: 2, count: 9)
                KeyValueGrid([
                    .init("Quantity", value: "3"),
                    .init("PoSe score", value: nil, reason: "Requires full-node data source"),
                ])
            }
            .padding(DashSpacing.m)
            .background(Color.role.canvas)
        },
        DashUIMacGallerySample("phrase-grid", title: "PhraseGrid", width: 520) {
            PhraseGrid(words: ["galaxy", "rocket", "velvet", "harbor", "tiny", "maple", "oyster", "crane", "sunset",
                               "ribbon", "empty", "above"])
                .padding(DashSpacing.m)
                .background(Color.role.canvas)
        },
        DashUIMacGallerySample("button-styles", title: "Dash button styles on SwiftUI Button") {
            VStack(alignment: .leading, spacing: DashSpacing.s) {
                HStack {
                    Button("Send") {}.buttonStyle(.dash(.filledBlue, .large))
                    Button("Cancel") {}.buttonStyle(.dash(.tintedGray, .large))
                }
                HStack {
                    Button("Add Recipient", systemImage: "plus") {}.buttonStyle(.dash(.tintedBlue, .medium))
                    Button("Export…") {}.buttonStyle(.dash(.plainBlue, .small))
                    Button("Wipe") {}.buttonStyle(.dash(.plainRed, .small))
                    Button("Disabled") {}.buttonStyle(.dash(.filledBlue, .small)).disabled(true)
                }
                Text("Field").padding(DashSpacing.s).modifier(DashFieldModifier())
            }
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
                        .foregroundStyle(Color.role.textPrimary)
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
