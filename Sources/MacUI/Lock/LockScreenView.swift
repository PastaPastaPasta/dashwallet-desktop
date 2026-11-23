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
    /// Names the network in a capsule off mainnet.
    var network: DashNetwork?
    @State private var passphrase = ""
    @State private var mixingOnly = false
    @State private var showsQuickReceive = false
    @FocusState private var focused: Bool

    /// The iOS lock screen on the hero blue (UX-SPEC §4.4): white wordmark,
    /// dash-qt's passphrase copy, a white field and white actions.
    var body: some View {
        ZStack {
            Color.role.hero.ignoresSafeArea()
            VStack(spacing: DashSpacing.l) {
                DashIconImage(.token(.dashLogo), template: true)
                    .scaledToFit()
                    .frame(height: 32)
                    .foregroundStyle(Color.role.textOnHero)
                    .accessibilityLabel(L10n.Navigation.appName)
                if let network, network != .mainnet {
                    NetworkCapsule(L10n.Settings.networkName(network))
                }
                VStack(spacing: DashSpacing.xs) {
                    Text(L10n.Lock.title)
                        .dashFont(.title2)
                        .foregroundStyle(Color.role.textOnHero)
                    Text(L10n.Lock.prompt)
                        .dashFont(.subhead)
                        .foregroundStyle(Color.role.textOnHero.opacity(0.8))
                        .multilineTextAlignment(.center)
                        .fixedSize(horizontal: false, vertical: true)
                }
                .padding(.top, DashSpacing.s)
                VStack(alignment: .leading, spacing: DashSpacing.s) {
                    SecureField(MacStrings.Common.passphrase, text: $passphrase)
                        .textFieldStyle(.plain)
                        .font(DesignTokens.DashTextStyle.callout.font)
                        .foregroundStyle(Color.role.textPrimary)
                        .padding(.horizontal, DashSpacing.l)
                        .frame(height: 48)
                        .background(RoundedRectangle(cornerRadius: DashRadius.textField, style: .continuous).fill(Color.white))
                        .environment(\.colorScheme, .light)
                        .focused($focused)
                        .disabled(lock.isDisabled || lock.retryAfter != nil)
                        .onSubmit(unlock)
                        .accessibilityIdentifier("lock.passphrase")
                    Toggle(isOn: $mixingOnly) {
                        Text(MacStrings.Lock.mixingOnly)
                            .dashFont(.footnote)
                            .foregroundStyle(Color.role.textOnHero)
                    }
                    .toggleStyle(.checkbox)
                    if let message = lock.message {
                        Text(message)
                            .dashFont(.footnoteMedium)
                            .foregroundStyle(Color.role.textOnHero)
                            .padding(.horizontal, DashSpacing.m)
                            .padding(.vertical, DashSpacing.xs)
                            .background(RoundedRectangle(cornerRadius: DashRadius.standard).fill(Color.role.danger))
                            .fixedSize(horizontal: false, vertical: true)
                            .accessibilityIdentifier("lock.message")
                    }
                }
                .frame(width: 360)
                Button(action: unlock) {
                    if lock.isWorking {
                        ProgressView().controlSize(.small)
                    } else {
                        Text(MacStrings.Lock.unlock)
                    }
                }
                .buttonStyle(.dash(.filledWhiteBlue, .large, fillsWidth: true))
                .frame(width: 360)
                .disabled(passphrase.isEmpty || lock.isDisabled || lock.isWorking)
                .accessibilityIdentifier("lock.unlock")
                if lock.isDisabled {
                    Text(L10n.Lock.restoreWithPhrase)
                        .dashFont(.footnoteMedium)
                        .foregroundStyle(Color.role.textOnHero)
                }
                if receive?.qr != nil {
                    Button(MacStrings.Lock.quickReceive, systemImage: "arrow.down.left") { showsQuickReceive = true }
                        .buttonStyle(.dash(.tintedWhite, .medium))
                        .padding(.top, DashSpacing.xl)
                        .accessibilityIdentifier("lock.quickReceive")
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
                    .dashFont(.footnote)
                    .foregroundStyle(Color.role.textPrimary)
                    .textSelection(.enabled)
                    .lineLimit(1)
                    .truncationMode(.middle)
                    .help(address)
                    .frame(maxWidth: 220)
            } else {
                Text(MacStrings.MenuBar.noAddress)
                    .dashFont(.footnote)
                    .foregroundStyle(Color.role.textSecondary)
            }
        }
    }
}
#endif
