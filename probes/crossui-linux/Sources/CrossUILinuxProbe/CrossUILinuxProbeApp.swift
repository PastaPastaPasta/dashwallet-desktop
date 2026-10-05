import GtkBackend
import SwiftCrossUI
import WalletProbeCore

/// G2 probe window: a sidebar of wallet pages and a detail pane, all driven by
/// one `WalletProbeViewModel`.
@main
struct CrossUILinuxProbeApp: App {
    @State var model = WalletProbeViewModel()

    var backend: GtkBackend {
        GtkBackend(appIdentifier: "org.dash.WalletDesktop.CrossUIProbe")
    }

    var body: some Scene {
        WindowGroup("Dash Wallet Probe") {
            WalletWindow(model: model)
        }
        .defaultSize(width: 900, height: 640)
    }
}

struct WalletWindow: View {
    @Bindable var model: WalletProbeViewModel

    var body: some View {
        NavigationSplitView {
            List(SidebarItem.allCases, selection: $model.selectedSidebarItem)
                .frame(minWidth: 180)
        } detail: {
            VStack(alignment: .leading, spacing: 12) {
                Text(model.detailTitle)
                    .font(.title)
                switch model.selectedSidebarItem {
                case .overview, nil:
                    OverviewPage(model: model)
                case .send:
                    SendPage(model: model)
                case .receive:
                    Text("Receive is not implemented in this probe.")
                case .transactions:
                    TransactionsPage(model: model)
                }
            }
            .padding(16)
        }
    }
}

struct OverviewPage: View {
    @Bindable var model: WalletProbeViewModel

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text(model.balanceText)
            Toggle("Hide balance", isOn: $model.hideBalance)
                .toggleStyle(.switch)
            Text("\(model.transactions.count) sample transactions")
        }
    }
}

struct SendPage: View {
    @Bindable var model: WalletProbeViewModel

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("Pay to")
            TextField("Dash address", text: $model.sendAddress)
                .frame(minWidth: 360)
            Toggle("Use only mixed funds", isOn: $model.mixedFundsOnly)
                .toggleStyle(.checkbox)
            Button("Send") { model.send() }
            Text(model.sendStatusText)
        }
    }
}

struct TransactionsPage: View {
    @Bindable var model: WalletProbeViewModel

    var body: some View {
        // SwiftCrossUI's List does not scroll by itself on GtkBackend. Without the
        // ScrollView the window grows to fit all 50 rows, and under Xvfb that ends in
        // an X BadAlloc that kills the app (see RESULTS.md, run 1).
        ScrollView {
            List(model.transactions, selection: $model.selectedTransactionID) { tx in
                Text(tx.summary)
            }
        }
    }
}
