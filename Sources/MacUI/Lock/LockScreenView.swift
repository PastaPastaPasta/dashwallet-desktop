// Full-window lock screen (QT-111/112, IOS-012/013) with Quick Receive,
// which works while locked because addresses are not secret.
#if os(macOS)
import DashUIMac
import DesignTokens
import SwiftUI
import WalletFeatures
import WalletRuntime

struct LockScreenView: View {
    let lock: LockViewModel
    let receive: ReceiveViewModel?
    @State private var passphrase = ""
    @State private var mixingOnly = false
    @State private var showsQuickReceive = false
    @FocusState private var focused: Bool

    var body: some View {
        ZStack {
            Color.dash.primaryBackground.ignoresSafeArea()
            VStack(spacing: DashSpacing.xl) {
                Image(systemName: "lock.fill")
                    .font(.system(size: 44))
                    .foregroundStyle(Color.dash.blue)
                    .accessibilityHidden(true)
                Text(L10n.Lock.title)
                    .dashFont(.title2)
                    .foregroundStyle(Color.dash.primaryText)
                Text(L10n.Lock.prompt)
                    .dashFont(.subhead)
                    .foregroundStyle(Color.dash.secondaryText)
                    .multilineTextAlignment(.center)
                VStack(alignment: .leading, spacing: DashSpacing.s) {
                    SecureField(MacStrings.Common.passphrase, text: $passphrase)
                        .textFieldStyle(.roundedBorder)
                        .focused($focused)
                        .disabled(lock.isDisabled || lock.retryAfter != nil)
                        .onSubmit(unlock)
                        .accessibilityIdentifier("lock.passphrase")
                    Toggle(MacStrings.Lock.mixingOnly, isOn: $mixingOnly)
                        .toggleStyle(.checkbox)
                    if let message = lock.message {
                        Text(message)
                            .dashFont(.footnote)
                            .foregroundStyle(Color.dash.errorText)
                            .accessibilityIdentifier("lock.message")
                    }
                }
                .frame(width: 320)
                HStack(spacing: DashSpacing.m) {
                    if receive?.qr != nil {
                        DashButton(
                            text: MacStrings.Lock.quickReceive, size: .medium, style: .strokeGray,
                            action: { showsQuickReceive = true })
                    }
                    DashButton(
                        text: MacStrings.Lock.unlock, isEnabled: !passphrase.isEmpty && !lock.isDisabled,
                        isLoading: lock.isWorking, size: .medium, style: .filledBlue, action: unlock)
                    .accessibilityIdentifier("lock.unlock")
                }
                if lock.isDisabled {
                    Text(L10n.Lock.restoreWithPhrase)
                        .dashFont(.footnoteMedium)
                        .foregroundStyle(Color.dash.blueText)
                }
            }
            .padding(DashSpacing.xxxl)
        }
        .accessibilityIdentifier("lock.screen")
        .onAppear { focused = true }
        .task(id: lock.retryDeadline) {
            // Ticks the throttle countdown while one is running.
            while lock.retryDeadline != nil, !Task.isCancelled {
                lock.refreshCountdown()
                try? await Task.sleep(for: .seconds(1))
            }
        }
        .popover(isPresented: $showsQuickReceive) {
            if let receive { QuickReceiveView(receive: receive).padding(DashSpacing.xl) }
        }
    }

    private func unlock() {
        let text = passphrase
        passphrase = ""
        Task { await lock.unlock(passphrase: text, mixingOnly: mixingOnly) }
    }
}

/// QR and address of the current receiving address (lock screen, menu bar).
struct QuickReceiveView: View {
    let receive: ReceiveViewModel

    var body: some View {
        VStack(spacing: DashSpacing.s) {
            if let qr = receive.qr {
                QRView(size: qr.size, modules: qr.modules, accessibilityLabel: MacStrings.Receive.qrLabel)
                    .frame(width: 180, height: 180)
            }
            if let address = receive.copyAddress() {
                Text(address)
                    .font(.system(.footnote, design: .monospaced))
                    .textSelection(.enabled)
                    .lineLimit(1)
                    .truncationMode(.middle)
                    .frame(maxWidth: 220)
            } else {
                Text(MacStrings.MenuBar.noAddress)
                    .dashFont(.footnote)
                    .foregroundStyle(Color.dash.secondaryText)
            }
        }
    }
}
#endif
