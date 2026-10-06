// Component gallery for visual QA (`dash-wallet --gallery`, DESIGN-opus §5).
import DesignTokens
import SwiftCrossUI

public struct DashUICrossGallery: View {
    @State var text = ""
    @State var secret = ""
    @State var unit = 0

    public init() {}

    public var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: points(DashSpacing.l)) {
                SectionHeader("DashUICross gallery", style: .title2)
                SectionHeader("DashButton")
                ForEach(DashButtonStyle.allCases.map(GalleryStyle.init)) { item in
                    HStack(spacing: points(DashSpacing.s)) {
                        ForEach(DashButtonSize.allCases.map(GallerySize.init)) { size in
                            DashButton(item.name, style: item.style, size: size.size) {}
                        }
                        DashButton("Disabled", style: item.style, isEnabled: false) {}
                    }
                }
                SectionHeader("Fields")
                DashTextField("Pay to", placeholder: "Dash address", text: $text, error: "The recipient address is not valid.")
                DashSecureField("Passphrase", text: $secret)
                DashPicker("Unit", options: [PickerOption(0, "DASH"), PickerOption(1, "mDASH")], selection: $unit)
                SectionHeader("Cards")
                DashCard {
                    MenuItem(title: "Available:", subtitle: "Your current spendable balance", trailing: "1.2345 DASH")
                    MenuItem(title: "Pending:", trailing: "0.10 DASH")
                }
                DashCard {
                    TransactionView(direction: .incoming, title: "Received with", subtitle: "yXdemo…", amount: "+1.00 DASH")
                    TransactionView(direction: .outgoing, title: "Sent to", subtitle: "yYdemo…", amount: "-0.50 DASH", detail: "InstantSend")
                    TransactionView(direction: .internalTransfer, title: "Payment to yourself", subtitle: "", amount: "-0.0001 DASH")
                }
                SectionHeader("Feedback")
                Toast("Informational message")
                Toast("Sent successfully", kind: .success)
                Toast("Out of sync", kind: .warning, actionTitle: "Change peers") {}
                Toast("The amount exceeds your balance.", kind: .error)
                HStack(spacing: points(DashSpacing.s)) {
                    DashBadge("Testnet")
                    DashBadge("Synced", foreground: .green, background: .greenAlpha10)
                }
                SectionHeader("QRCodeView")
                QRCodeView(size: 21, modules: GallerySample.finderOnly, side: 160)
                StatusBarView(
                    syncText: "Synchronizing with network…", progress: 0.42,
                    items: [StatusBarItem(id: "net", text: "Testnet"), StatusBarItem(id: "peers", text: "8 peers")])
            }
            .padding(points(DashSpacing.xl))
        }
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
