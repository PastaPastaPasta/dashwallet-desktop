// Component gallery for visual QA (`dash-wallet --gallery`, DESIGN-opus §5,
// UX-SPEC Appendix B): every DashUICross component on sample data.
import DesignTokens
import SwiftCrossUI

public struct DashUICrossGallery: View {
    @State var text = ""
    @State var secret = ""
    @State var unit = 0
    @State var flag = true

    public init() {}

    public var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: points(DashSpacing.l)) {
                TopIntro("DashUICross gallery", description: "Every component of UX-SPEC §3 that Cross draws.")
                HeroBand {
                    BalanceHero(
                        network: "Testnet", caption: "Syncing balance", amount: "30.39", unit: .glyph,
                        cells: [
                            BalanceCell(title: "Available", subtitle: "Spendable now", amount: "30.19"),
                            BalanceCell(title: "Pending", subtitle: "Awaiting confirmation", amount: "0.20"),
                        ], cellUnit: .glyph)
                } overlap: {
                    ShortcutCard(items: [
                        ShortcutItem(id: 0, title: "Receive", icon: .receive),
                        ShortcutItem(id: 1, title: "Send", icon: .send),
                        ShortcutItem(id: 2, title: "Scan QR", icon: .scanQR, isEnabled: false),
                        ShortcutItem(id: 3, title: "Back up", icon: .backup),
                    ]) { _ in }
                }
                VStack(alignment: .leading, spacing: points(DashSpacing.l)) {
                HistoryHeader(title: "History", syncText: "Syncing 47.0%", filterTitle: "Filter", onSync: {}, onFilter: {})
                TransactionGroupCard(day: "Today", weekday: "Tuesday") {
                    TransactionView(
                        direction: .incoming, title: "Received", subtitle: "01:40", amount: "+0.20", unit: .glyph,
                        chip: TransactionChip("Pending"))
                    TransactionView(
                        direction: .outgoing, title: "Alice", subtitle: "05:46", amount: "-0.50000226", unit: .glyph)
                    TransactionView(
                        direction: .mined, title: "Mined", subtitle: "06:10", amount: "+1.25", unit: .name("mDASH"),
                        status: "Locked")
                }
                SectionHeader("DashButton")
                ForEach(DashButtonStyle.allCases.map(GalleryStyle.init)) { item in
                    HStack(spacing: points(DashSpacing.s)) {
                        ForEach(DashButtonSize.allCases.map(GallerySize.init)) { size in
                            DashButton(item.name, style: item.style, size: size.size) {}
                        }
                        DashButton("Disabled", style: item.style, isEnabled: false) {}
                        DashButton("Copy", style: item.style, size: .small, icon: .copy) {}
                    }
                    .padding(points(DashSpacing.xs))
                    .background(
                        item.style == .filledWhite || item.style == .tintedWhite
                            ? CrossRole.hero.color : CrossRole.canvas.color)
                }
                }
                VStack(alignment: .leading, spacing: points(DashSpacing.l)) {
                SectionHeader("Fields")
                DashTextField("Pay to", placeholder: "Dash address", text: $text, error: "The recipient address is not valid.")
                DashSecureField("Passphrase", text: $secret)
                DashPicker("Unit", options: [PickerOption(0, "DASH"), PickerOption(1, "mDASH")], selection: $unit)
                DashToggle("Enable coin control features", isOn: $flag)
                SectionHeader("Cards")
                DashCard(padding: points(DashSpacing.xs), spacing: points(DashSpacing.xxxs)) {
                    MenuItem(icon: .security, title: "Wallet encryption", trailing: "Encrypted")
                    MenuItem(icon: .recoveryPhrase, title: "Show recovery phrase…", subtitle: "Write it down on paper")
                    MenuItem(icon: .resetWallet, title: "Wipe wallet", destructive: true)
                }
                DashCard {
                    KeyValueRow("Status", "0/unconfirmed, in memory pool")
                    KeyValueRow("Transaction ID", "57143a02…a943469a4", monospaced: true)
                    CopyRow("Address", value: "yQkAZdnn922YucpQ9DZedU1uFY4") {}
                }
                WizardHeader(step: 2, of: 5, title: "Collateral")
                EmptyState(icon: .txAll, title: "There are no transactions to display")
                LoadingState("Loading transactions")
                }
                VStack(alignment: .leading, spacing: points(DashSpacing.l)) {
                SectionHeader("Feedback")
                Toast("Informational message")
                Toast("Sent successfully", kind: .success)
                Toast("No connection to the Dash network", kind: .warning, actionTitle: "Change peers") {}
                Toast("The amount exceeds your balance.", kind: .error)
                ToastPill("Copied")
                HStack(spacing: points(DashSpacing.s)) {
                    NetworkCapsule("Testnet")
                    DashBadge("Pending")
                    DashBadge("Funded", tone: .success)
                    DashBadge("Unfunded", tone: .warning)
                    DashBadge("Conflicted", tone: .danger)
                    DashBadge("Demo", tone: .neutral)
                }
                SectionHeader("QRCodeView")
                QRCodeView(size: 21, modules: GallerySample.finderOnly, side: 160)
                StatusBarView(
                    syncText: "Synchronizing with network…", progress: 0.42,
                    items: [StatusBarItem(id: "hd", text: "HD"), StatusBarItem(id: "lock", text: "Locked", tone: .success)])
                }
            }
            .padding(points(DashSpacing.xl))
        }
        .background(CrossRole.canvas.color)
        .toolkitThemeFromEnvironment()
    }
}

private struct GalleryStyle: Identifiable {
    let style: DashButtonStyle
    var id: String { name }
    var name: String { "\(style)" }
}

private struct GallerySize: Identifiable {
    let size: DashButtonSize
    var id: String { "\(size)" }
}

enum GallerySample {
    /// A 21x21 matrix with the three finder patterns only (not a scannable code).
    static let finderOnly: [Bool] = {
        var modules = [Bool](repeating: false, count: 21 * 21)
        for (ox, oy) in [(0, 0), (14, 0), (0, 14)] {
            for y in 0..<7 {
                for x in 0..<7 {
                    let ring = x == 0 || y == 0 || x == 6 || y == 6
                    let core = (2...4).contains(x) && (2...4).contains(y)
                    modules[(oy + y) * 21 + ox + x] = ring || core
                }
            }
        }
        return modules
    }()
}
