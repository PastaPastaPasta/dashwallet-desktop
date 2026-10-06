// IOS-057: the CoinJoin recovery scan and "Move mixed coins" (a chunked
// sweep of the CoinJoin account to a regular address of the same wallet; the
// shielded destination is M4). Entries: Overview banner, Options/Security and
// Tools. A sweep may stop part-way; the result says what moved.
import Foundation
import Observation
import WalletRuntime

public enum MixedCoinsFlow: Sendable, Hashable {
    case idle
    case scanning
    case scanned(CoinJoinRecoveryReport)
    case planning
    case planned(MixedCoinsSweepPlan)
    /// The move waits for the wallet passphrase.
    case needsPassphrase(MixedCoinsSweepPlan)
    case moving
    case moved(MixedCoinsSweepResult)
    /// The engine does not offer this yet ("Not available yet").
    case unavailable
}

@MainActor
@Observable
public final class MixedCoinsViewModel {
    public private(set) var flow: MixedCoinsFlow = .idle
    public var destination: MixedCoinsDestination = .wallet
    public private(set) var errorMessage: String?

    /// The fully mixed balance of the selected wallet.
    public var mixedBalance: Amount { walletState.balances?.coinjoin ?? .zero }

    /// The Overview banner: mixed coins exist and "Later" was not chosen at
    /// this balance (iOS shows it again once the balance changes).
    public var showsMoveBanner: Bool {
        guard let wallet = walletState.selectedWalletID, mixedBalance > .zero else { return false }
        return desktopPreferences.desktop.m3.mixedCoinsDismissedAt[wallet.hex] != mixedBalance.duffs
    }

    public var bannerText: String { L10n.CoinJoin.moveBanner(amountText(mixedBalance)) }

    /// The plan line: total, transactions and fees.
    public var planText: String? {
        let plan: MixedCoinsSweepPlan
        switch flow {
        case .planned(let p), .needsPassphrase(let p): plan = p
        default: return nil
        }
        let fee = Amount(duffs: plan.chunks.reduce(0) { $0 + $1.fee.duffs })
        return L10n.CoinJoin.movePlan(total: amountText(plan.total), transactions: plan.chunks.count, fee: amountText(fee))
    }

    /// What the scan or the move did.
    public var resultText: String? {
        switch flow {
        case .scanned(let report):
            return L10n.CoinJoin.recoveryResult(
                addresses: report.coinJoinAddressesScanned + report.bip44AddressesScanned,
                balance: amountText(report.coinJoinBalance), transactions: report.newTransactions)
        case .moved(let result):
            guard let code = result.failureCode else { return L10n.CoinJoin.moved(amountText(result.moved)) }
            return L10n.CoinJoin.movedPartially(
                moved: amountText(result.moved), remaining: amountText(result.remaining),
                reason: ErrorText.m3(ServiceError(code: code), amount: amountText.callAsFunction))
        default:
            return nil
        }
    }

    public var isBusy: Bool {
        switch flow {
        case .scanning, .planning, .moving: true
        default: false
        }
    }

    private let mixedCoins: any MixedCoinsMoving
    private let walletState: any WalletStateProviding
    private let grants: GrantRequester
    private let amountText: AmountText
    private let desktopPreferences: any DesktopPreferencesStoring

    public init(
        mixedCoins: any MixedCoinsMoving, walletState: any WalletStateProviding, auth: any AuthenticationGating,
        vault: any VaultProviding, amounts: any AmountFormatting, settings: any SettingsProviding,
        desktopPreferences: any DesktopPreferencesStoring
    ) {
        self.mixedCoins = mixedCoins
        self.walletState = walletState
        grants = GrantRequester(auth: auth, vault: vault)
        amountText = AmountText(amounts: amounts, settings: settings)
        self.desktopPreferences = desktopPreferences
    }

    public convenience init(env: AppEnvironment, m2: M2Services, m3: M3Services) {
        self.init(
            mixedCoins: m3.mixedCoins, walletState: env.walletState, auth: env.auth, vault: env.vault,
            amounts: env.amounts, settings: env.settings, desktopPreferences: m2.desktopPreferences)
    }

    /// "Scan for CoinJoin funds": a rescan with the wide lookahead.
    public func scan() async {
        guard let wallet = walletState.selectedWalletID, !isBusy else { return }
        errorMessage = nil
        flow = .scanning
        do {
            flow = .scanned(try await mixedCoins.recoveryScan(wallet: wallet))
        } catch {
            fail(error)
        }
    }

    /// The chunks the move would broadcast, with fees.
    public func preparePlan() async {
        guard let wallet = walletState.selectedWalletID, !isBusy else { return }
        errorMessage = nil
        flow = .planning
        do {
            flow = .planned(try await mixedCoins.plan(wallet: wallet, destination: destination))
        } catch {
            fail(error)
        }
    }

    /// "Move" with a `.spend(max: total)` grant. An encrypted vault needs
    /// `passphrase`; without it the flow becomes `.needsPassphrase`.
    public func move(passphrase: String? = nil) async {
        let plan: MixedCoinsSweepPlan
        switch flow {
        case .planned(let p), .needsPassphrase(let p): plan = p
        default: return
        }
        guard let wallet = walletState.selectedWalletID else { return }
        errorMessage = nil
        let grant: AuthGrant
        do {
            guard let issued = try await grants.authorize(.spend(max: plan.total), wallet: wallet, passphrase: passphrase)
            else {
                flow = .needsPassphrase(plan)
                return
            }
            grant = issued
        } catch {
            errorMessage = ErrorText.m3(error, amount: amountText.callAsFunction)
            flow = error.code == .vaultWrongPassphrase ? .needsPassphrase(plan) : .planned(plan)
            return
        }
        flow = .moving
        do {
            let result = try await mixedCoins.move(wallet: wallet, destination: plan.destination, grant: grant)
            if !result.txids.isEmpty {
                desktopPreferences.updateM3 { $0.coinJoinWithdrawals[wallet.hex, default: []] += result.txids }
            }
            flow = .moved(result)
        } catch {
            grants.revoke(grant)
            fail(error)
        }
    }

    /// "Later": hide the banner until the mixed balance changes.
    public func later() {
        guard let wallet = walletState.selectedWalletID else { return }
        let balance = mixedBalance.duffs
        desktopPreferences.updateM3 { $0.mixedCoinsDismissedAt[wallet.hex] = balance }
        reset()
    }

    public func reset() {
        flow = .idle
        errorMessage = nil
    }

    private func fail(_ error: ServiceError) {
        if error.isNotImplemented {
            flow = .unavailable
        } else {
            errorMessage = ErrorText.m3(error, amount: amountText.callAsFunction)
            flow = .idle
        }
    }
}
