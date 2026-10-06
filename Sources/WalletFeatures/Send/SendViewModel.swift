// Send (QT-051…063, QT-067, IOS-041…052; DESIGN-opus §1.11).
//
// Flow: editing → [confirmDuplicates] → [authorizing] → preparing → confirm
// (3 s countdown) → broadcasting → done | failed | broadcastUnknown.
// `prepare` signs and reserves inputs but never broadcasts; only `confirm()`
// (or `retryBroadcast()` after `send.no_peers`) broadcasts (iOS rule 4).
//
// Releasing reserved inputs (review M-7):
// - Cancel, or any edit of entries, fee or source while a payment is under
//   review (duplicates question, authorizing, preparing, confirm, failed),
//   abandons the prepared transaction and revokes a grant `prepare` has not
//   redeemed. A prepare that finishes after such an edit is abandoned on
//   arrival.
// - A broadcast the network definitely did not take ends in `.failed`;
//   dismissing it abandons the transaction.
// - A broadcast whose outcome is unknown (any other error) ends in
//   `.broadcastUnknown` and is never abandoned: its inputs may already be in
//   the mempool, and releasing them would let the next send double-spend.
import Foundation
import Observation
import WalletRuntime

public enum SendPage: Sendable, Hashable {
    case regular
    /// dash-qt's CoinJoin send page: fully mixed coins only (QT-051).
    case coinJoin
}

public struct RecipientEntry: Sendable, Hashable, Identifiable {
    public let id: UUID
    public var address: String
    public var amountText: String
    public var subtractFee: Bool
    public var label: String
    /// From a URI `message=`; read-only, stored locally only (QT-054).
    public var message: String?
    /// The engine or validation code for this entry, if any.
    public var error: ServiceErrorCode?
    public var addressError: String?
    public var amountError: String?

    public init(
        id: UUID = UUID(), address: String = "", amountText: String = "", subtractFee: Bool = false,
        label: String = "", message: String? = nil
    ) {
        self.id = id
        self.address = address
        self.amountText = amountText
        self.subtractFee = subtractFee
        self.label = label
        self.message = message
    }

    public var isBlank: Bool { address.isEmpty && amountText.isEmpty && label.isEmpty && message == nil }

    /// The fields the user edits; validation results are left out.
    var userFields: [String] {
        [address, amountText, subtractFee ? "1" : "0", label, message ?? "\u{0}"]
    }
}

public struct SendFailure: Sendable, Equatable {
    public let code: ServiceErrorCode
    public let message: String
}

public enum SendPhase: Sendable, Equatable {
    case editing
    /// The same address appears more than once: a question, not an error (QT-060).
    case confirmDuplicates
    /// A passphrase is needed for the spend grant (QT-061).
    case authorizing
    case preparing
    /// Signed and reserved, waiting for the user (QT-059, IOS-046).
    case confirm(PreparedTxSummary)
    case broadcasting
    case done(txid: String)
    case failed(SendFailure)
    /// The broadcast failed in a way that does not tell whether peers got the
    /// transaction. Its inputs stay reserved; the user checks Transactions.
    case broadcastUnknown(txid: String, failure: SendFailure)
}

/// dash-qt confirmation targets in blocks with their labels (QT-057).
public struct ConfirmationTarget: Sendable, Hashable {
    public let blocks: UInt32
    public let label: String

    public static let all: [ConfirmationTarget] = [
        .init(blocks: 2, label: "5 minutes"), .init(blocks: 4, label: "10 minutes"),
        .init(blocks: 6, label: "15 minutes"), .init(blocks: 12, label: "30 minutes"),
        .init(blocks: 24, label: "60 minutes"), .init(blocks: 48, label: "2 hours"),
        .init(blocks: 144, label: "6 hours"), .init(blocks: 504, label: "21 hours"),
        .init(blocks: 1008, label: "42 hours"),
    ]
    public static let defaultBlocks: UInt32 = 6
}

@MainActor
@Observable
public final class SendViewModel {
    /// Custom fee floor, max(mintxfee, minrelay) (QT-057).
    public static let minimumFeePerKilobyte = Amount(duffs: 1000)
    /// `-maxtxfee` default (QT-058).
    public static let maximumFee = Amount(duffs: 10_000_000)
    public static let confirmDelaySeconds = 3
    /// Recipients listed in the confirm text before "(x of y entries displayed)".
    public static let maxConfirmLines = 10

    /// Editable by the UI. A change of the user fields while a payment is
    /// under review returns to `.editing` and abandons the prepared
    /// transaction (review M-7).
    public var entries: [RecipientEntry] = [RecipientEntry()] {
        didSet {
            guard internalEdits == 0, oldValue.map(\.userFields) != entries.map(\.userFields) else { return }
            userEdited()
        }
    }
    public private(set) var phase: SendPhase = .editing
    public let page: SendPage
    public private(set) var source: CoinSourceChoice
    public private(set) var fee: FeeChoice = .recommended(targetBlocks: ConfirmationTarget.defaultBlocks)
    public private(set) var customFeeWarning = false
    public private(set) var estimate: TxEstimate?
    /// Seconds before Send is enabled in the confirm step.
    public private(set) var confirmCountdown = 0
    /// Set after a broadcast: Transactions with the new transaction selected (QT-063).
    public var route: AppRoute?

    public var canConfirm: Bool {
        if case .confirm = phase { return confirmCountdown == 0 }
        return false
    }

    /// The broadcast found no peers; `retryBroadcast()` sends the same
    /// signed transaction again.
    public var canRetryBroadcast: Bool {
        if case .failed(let failure) = phase { return failure.code == .sendNoPeers && prepared != nil }
        return false
    }

    /// Whether the form may change: not while broadcasting or after a
    /// broadcast with an unknown outcome (dismiss first).
    public var isEditable: Bool {
        switch phase {
        case .broadcasting, .broadcastUnknown: false
        default: true
        }
    }

    public var unit: DisplayUnit { settings.display.unit }

    public var sendButtonTitle: String {
        let base = page == .coinJoin ? L10n.Send.sendMixedFunds : L10n.Send.send
        if case .confirm = phase, confirmCountdown > 0 { return L10n.Send.sendCountdown(confirmCountdown) }
        return base
    }

    private let walletState: any WalletStateProviding
    private let sender: any TransactionSending
    private let auth: any AuthenticationGating
    private let vault: any VaultProviding
    private let uri: any URIHandling
    private let amounts: any AmountFormatting
    private let settings: any SettingsProviding
    private let sync: (any SyncStatusProviding)?
    private let addressBook: (any AddressBookProviding)?
    private let network: DashNetwork
    private let timing: Timing
    private var draft: (any TransactionDrafting)?
    private var prepared: PreparedTransaction?
    private var duplicatesAcknowledged = false
    private var spendLimit: Amount?
    /// A grant issued for this payment that `prepare` has not redeemed.
    private var pendingGrant: AuthGrant?
    /// Bumped by every cancel and edit; async steps that started under an
    /// older value drop (and abandon) their result.
    private var generation = 0
    /// Non-zero while the view model itself changes `entries`.
    private var internalEdits = 0
    private var countdownTask: Task<Void, Never>?

    public init(
        walletState: any WalletStateProviding, sender: any TransactionSending, auth: any AuthenticationGating,
        vault: any VaultProviding, uri: any URIHandling, amounts: any AmountFormatting,
        settings: any SettingsProviding, sync: (any SyncStatusProviding)?, addressBook: (any AddressBookProviding)?,
        network: DashNetwork, timing: Timing, page: SendPage = .regular
    ) {
        self.walletState = walletState
        self.sender = sender
        self.auth = auth
        self.vault = vault
        self.uri = uri
        self.amounts = amounts
        self.settings = settings
        self.sync = sync
        self.addressBook = addressBook
        self.network = network
        self.timing = timing
        self.page = page
        self.source = page == .coinJoin ? .fullyMixed : .any
    }

    public convenience init(env: AppEnvironment, network: DashNetwork, page: SendPage = .regular) {
        self.init(
            walletState: env.walletState, sender: env.sender, auth: env.auth, vault: env.vault, uri: env.uri,
            amounts: env.amounts, settings: env.settings, sync: env.sync, addressBook: env.addressBook,
            network: network, timing: env.timing, page: page)
    }

    // MARK: Editing

    public func addRecipient() {
        guard isEditable else { return }
        entries.append(RecipientEntry())
    }

    /// Removing the last entry leaves one blank entry (dash-qt).
    public func removeRecipient(_ id: RecipientEntry.ID) {
        guard isEditable else { return }
        entries.removeAll { $0.id == id }
        if entries.isEmpty { entries = [RecipientEntry()] }
    }

    /// Clear All: entries, coin selection, fee warning (QT-052).
    public func clearAll() {
        guard isEditable else { return }
        withInternalEdits { entries = [RecipientEntry()] }
        if page == .regular { source = .any }
        userEdited()
    }

    /// Pastes an address or a `dash:` URI into an entry (QT-054). Without
    /// `id`, the first blank entry is used, or a new one is added.
    public func paste(_ text: String, into id: RecipientEntry.ID? = nil) {
        guard isEditable else { return }
        let index = targetIndex(id)
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        if trimmed.lowercased().hasPrefix("dash:") {
            do {
                let parsed = try uri.parsePaymentURI(trimmed)
                apply(parsed, at: index)
            } catch {
                entries[index].address = trimmed
                entries[index].error = error.code
                entries[index].addressError = L10n.Send.invalidAddress
            }
        } else {
            entries[index].address = AddressInput.clean(trimmed)
            entries[index].error = nil
            entries[index].addressError = nil
        }
    }

    /// Fills an entry from an already parsed payment URI (drag and drop, OS handler).
    public func fill(from paymentURI: PaymentURI) {
        guard isEditable else { return }
        apply(paymentURI, at: targetIndex(nil))
    }

    public func setSource(_ source: CoinSourceChoice) {
        guard page == .regular, source != self.source, isEditable else { return }
        self.source = source
        userEdited()
    }

    /// Custom fees below 1000 duff/kB are raised to it, with dash-qt's warning.
    public func setFee(_ fee: FeeChoice) {
        guard isEditable else { return }
        let previous = self.fee
        switch fee {
        case .perKilobyte(let rate) where rate < Self.minimumFeePerKilobyte:
            self.fee = .perKilobyte(Self.minimumFeePerKilobyte)
            customFeeWarning = true
        case .perKilobyte:
            self.fee = fee
            customFeeWarning = true
        case .recommended:
            self.fee = fee
            customFeeWarning = false
        }
        if self.fee != previous { userEdited() }
    }

    /// "Use available balance" (QT-053, dash-qt `useAvailableBalance`): the
    /// maximum minus the other entries' amounts, with subtract-fee ticked so
    /// a later fee change cannot push the total over the balance (review M-4).
    public func useMax(for id: RecipientEntry.ID) async {
        guard isEditable, entries.contains(where: { $0.id == id }),
            let wallet = walletState.selectedWalletID
        else { return }
        do {
            let maximum = try await sender.maxSpendable(wallet: wallet, source: source, fee: fee)
            guard let index = entries.firstIndex(where: { $0.id == id }) else { return }
            var others: Int64 = 0
            for (i, entry) in entries.enumerated() where i != index {
                if case .success(let amount?) = AmountInput.parse(entry.amountText, unit: unit, formatter: amounts) {
                    others += amount.duffs
                }
            }
            let value = Amount(duffs: max(0, maximum.duffs - others))
            entries[index].amountText = amounts.format(value, unit: unit, style: .plain(plusSign: false, separators: .never))
            entries[index].subtractFee = true
            entries[index].amountError = nil
        } catch {
            phase = .failed(failure(for: error))
        }
    }

    // MARK: Review and confirm

    /// Validates, asks about duplicates, authorizes and prepares.
    public func review() async {
        guard phase == .editing || phase == .confirmDuplicates else { return }
        if let guardFailure = sendGuard() {
            phase = .failed(guardFailure)
            return
        }
        guard let recipients = validateEntries() else {
            phase = .editing
            return
        }
        if !duplicatesAcknowledged, Set(recipients.map(\.address)).count < recipients.count {
            phase = .confirmDuplicates
            return
        }
        await buildDraft(recipients)
    }

    /// Yes on "Confirm duplicate recipients".
    public func acknowledgeDuplicates() async {
        guard phase == .confirmDuplicates else { return }
        duplicatesAcknowledged = true
        await review()
    }

    /// Passphrase for the spend grant, while `phase == .authorizing`.
    public func authorize(passphrase: String) async {
        guard phase == .authorizing, let spendLimit else { return }
        let secret = vault.makeSecret(utf8: passphrase)
        let started = generation
        do {
            let grant = try await auth.authorize(.spend(max: spendLimit), credential: .passphrase(secret))
            guard started == generation else {
                auth.revoke(grant)
                return
            }
            await prepare(grant: grant)
        } catch {
            guard started == generation else { return }
            phase = .failed(failure(for: error))
        }
    }

    /// Broadcasts the prepared transaction. Refused until the countdown ends.
    public func confirm() async {
        guard case .confirm = phase, confirmCountdown == 0 else { return }
        await broadcast()
    }

    /// Sends the same signed transaction again after `send.no_peers`.
    public func retryBroadcast() async {
        guard canRetryBroadcast else { return }
        await broadcast()
    }

    /// Cancel before the broadcast: abandons the prepared transaction,
    /// revokes an unredeemed grant and returns to editing. Ignored while
    /// broadcasting, after a send and after a broadcast whose outcome is
    /// unknown (use `dismiss()`).
    public func cancel() async {
        switch phase {
        case .broadcasting, .broadcastUnknown, .done:
            return
        case .editing, .confirmDuplicates, .authorizing, .preparing, .confirm, .failed:
            await releaseReview()
            phase = .editing
        }
    }

    /// Leaves a final phase. After a failure it abandons like `cancel()`;
    /// after `.done` and `.broadcastUnknown` it clears the form and abandons
    /// nothing.
    public func dismiss() async {
        switch phase {
        case .failed:
            await cancel()
        case .done, .broadcastUnknown:
            generation += 1
            prepared = nil
            draft = nil
            withInternalEdits { entries = [RecipientEntry()] }
            estimate = nil
            duplicatesAcknowledged = false
            phase = .editing
        case .editing, .confirmDuplicates, .authorizing, .preparing, .confirm, .broadcasting:
            break
        }
    }

    /// dash-qt "Confirm send coins" text lines (QT-059).
    public var confirmLines: [String] {
        guard case .confirm(let summary) = phase else { return [] }
        let style = AmountStyle.withUnit(plusSign: false, separators: .always)
        let recipients = summary.outputs.filter { !$0.isChange }
        var lines = [L10n.Send.confirmQuestion, L10n.Send.confirmReview]
        for output in recipients.prefix(Self.maxConfirmLines) {
            lines.append(
                L10n.Send.recipientLine(
                    amount: amounts.format(output.amount, unit: unit, style: style), label: output.label,
                    address: output.address ?? ""))
        }
        if recipients.count > Self.maxConfirmLines {
            lines.append(L10n.Send.entriesDisplayed(Self.maxConfirmLines, of: recipients.count))
        }
        lines.append(page == .coinJoin ? L10n.Send.usingCoinJoinFunds : L10n.Send.usingAnyFunds)
        var feeLine = L10n.Send.transactionFee(amounts.format(summary.fee, unit: unit, style: style))
        if page == .coinJoin { feeLine += " " + L10n.Send.coinJoinFeeNote }
        lines.append(feeLine)
        let kilobytes = String(format: "%.3f", Double(summary.sizeBytes) / 1000)
        lines.append(
            L10n.Send.sizeAndRate(
                kilobytes: kilobytes, rate: amounts.format(summary.feeRatePerKilobyte, unit: unit, style: style)))
        if page == .coinJoin { lines.append(L10n.Send.inputCount(summary.inputCount)) }
        lines.append(L10n.Send.totalAmount(amounts.format(summary.totalDebit, unit: unit, style: style)))
        return lines
    }

    /// The spend cap requested for `recipients`: their amounts plus the
    /// maximum fee (`-maxtxfee`). The final fee is only known after signing,
    /// so a cap of amounts + estimated fee could fail whenever the signed size
    /// differs from the estimate (review H-4); fees above the maximum are
    /// refused as absurd anyway (QT-058).
    public static func spendLimit(for recipients: [PaymentRecipient]) -> Amount {
        Amount(duffs: recipients.reduce(Int64(0)) { $0 + $1.amount.duffs } + maximumFee.duffs)
    }

    /// Broadcast errors after which peers certainly do not have the
    /// transaction, so its inputs may be released.
    public static let definiteBroadcastFailures: Set<ServiceErrorCode> = [
        .sendBroadcastRejected, .sendPreparedTxSpent, .sendNoPeers, .sendPreparedTxUnknown, .networkNotOpen,
        .walletNotFound, .invalidArgument, .notImplemented,
    ]

    // MARK: Private

    private func targetIndex(_ id: RecipientEntry.ID?) -> Int {
        if let id, let index = entries.firstIndex(where: { $0.id == id }) { return index }
        if let blank = entries.firstIndex(where: \.isBlank) { return blank }
        entries.append(RecipientEntry())
        return entries.count - 1
    }

    private func apply(_ parsed: PaymentURI, at index: Int) {
        entries[index].address = parsed.address
        if let amount = parsed.amount {
            entries[index].amountText = amounts.format(amount, unit: unit, style: .plain(plusSign: false, separators: .never))
        }
        if let label = parsed.label { entries[index].label = label }
        entries[index].message = parsed.message
        entries[index].error = nil
        entries[index].addressError = nil
        entries[index].amountError = nil
    }

    /// iOS send guards (IOS-051): no sends before sync is done or while offline.
    private func sendGuard() -> SendFailure? {
        guard walletState.selectedWalletID != nil else {
            return SendFailure(code: .walletNotFound, message: L10n.Common.noWallet)
        }
        guard let sync else { return nil }
        guard let status = sync.status, status.isDone else {
            return SendFailure(code: .syncSpvNotRunning, message: L10n.Send.syncing)
        }
        if status.connectedPeers == 0 {
            return SendFailure(code: .sendNoPeers, message: L10n.Send.offline)
        }
        return nil
    }

    /// Per-entry checks (QT-055); highlights the bad fields and returns `nil`
    /// when any entry fails.
    private func validateEntries() -> [PaymentRecipient]? {
        var recipients: [PaymentRecipient] = []
        var valid = true
        var checked = entries
        for index in checked.indices {
            var entry = checked[index]
            entry.error = nil
            entry.addressError = nil
            entry.amountError = nil
            entry.address = AddressInput.clean(entry.address)
            switch AddressInput.checkCore(entry.address, uri: uri, network: network) {
            case .success:
                break
            case .failure(let problem):
                valid = false
                (entry.error, entry.addressError) = Self.addressProblem(problem, network: network)
            }
            var amount = Amount.zero
            switch AmountInput.parsePayment(entry.amountText, unit: unit, formatter: amounts) {
            case .success(let value):
                amount = value
            case .failure(let problem):
                valid = false
                entry.error = entry.error ?? (problem == .dust ? .sendDustAmount : .sendInvalidAmount)
                entry.amountError = Self.amountProblem(problem)
            }
            checked[index] = entry
            recipients.append(
                PaymentRecipient(
                    address: entry.address, amount: amount, subtractFeeFromAmount: entry.subtractFee,
                    label: entry.label.isEmpty ? nil : entry.label, message: entry.message))
        }
        withInternalEdits { entries = checked }
        if recipients.isEmpty { valid = false }
        return valid ? recipients : nil
    }

    private func buildDraft(_ recipients: [PaymentRecipient]) async {
        guard let wallet = walletState.selectedWalletID else { return }
        phase = .preparing
        let started = generation
        do {
            let draft = try await sender.makeDraft(wallet: wallet)
            guard started == generation else { return }
            self.draft = draft
            try await draft.setRecipients(recipients)
            try await draft.setSource(source)
            try await draft.setFee(fee)
            let estimate = try await draft.estimate()
            guard started == generation else { return }
            self.estimate = estimate
            let limit = Self.spendLimit(for: recipients)
            spendLimit = limit
            switch auth.requirement(for: .spend(max: limit)) {
            case .none:
                let grant = try await auth.authorize(.spend(max: limit), credential: .unencrypted)
                guard started == generation else {
                    auth.revoke(grant)
                    return
                }
                await prepare(grant: grant)
            case .passphrase, .quickUnlockOrPassphrase:
                phase = .authorizing
            }
        } catch {
            guard started == generation else { return }
            handle(error)
        }
    }

    private func prepare(grant: AuthGrant) async {
        guard let draft else { return }
        phase = .preparing
        pendingGrant = grant
        let started = generation
        do {
            let prepared = try await draft.prepare(grant: grant)
            guard started == generation else {
                // Edited or cancelled while signing: release at once.
                try? await draft.abandon(prepared)
                return
            }
            pendingGrant = nil
            self.prepared = prepared
            phase = .confirm(prepared.summary)
            startCountdown()
        } catch {
            guard started == generation else { return }
            revokePendingGrant()
            handle(error)
        }
    }

    private func broadcast() async {
        guard let draft, let prepared else { return }
        stopCountdown()
        phase = .broadcasting
        do {
            let result = try await draft.broadcast(prepared)
            self.prepared = nil
            self.draft = nil
            let sent = entries
            await rememberRecipients(sent)
            withInternalEdits { entries = [RecipientEntry()] }
            estimate = nil
            duplicatesAcknowledged = false
            phase = .done(txid: result.txid)
            route = .transaction(txid: result.txid)
        } catch {
            let failure = failure(for: error)
            if Self.definiteBroadcastFailures.contains(error.code) {
                // Not announced: keep it so dismiss abandons it or a retry sends it.
                phase = .failed(failure)
            } else {
                let txid = prepared.summary.txid
                self.prepared = nil
                self.draft = nil
                phase = .broadcastUnknown(txid: txid, failure: failure)
                route = .transaction(txid: txid)
            }
        }
    }

    /// A user change of entries, fee or source.
    private func userEdited() {
        estimate = nil
        duplicatesAcknowledged = false
        switch phase {
        case .confirmDuplicates, .authorizing, .preparing, .confirm, .failed:
            let draft = draft
            let prepared = prepared
            generation += 1
            stopCountdown()
            revokePendingGrant()
            self.draft = nil
            self.prepared = nil
            phase = .editing
            if let draft, let prepared {
                Task { try? await draft.abandon(prepared) }
            }
        case .done:
            phase = .editing
        case .editing, .broadcasting, .broadcastUnknown:
            break
        }
    }

    /// Abandons the prepared transaction and revokes an unredeemed grant.
    private func releaseReview() async {
        generation += 1
        stopCountdown()
        revokePendingGrant()
        let draft = draft
        let prepared = prepared
        self.prepared = nil
        self.draft = nil
        duplicatesAcknowledged = false
        if let draft, let prepared {
            try? await draft.abandon(prepared)
        }
    }

    private func revokePendingGrant() {
        if let pendingGrant { auth.revoke(pendingGrant) }
        pendingGrant = nil
    }

    private func withInternalEdits(_ body: () -> Void) {
        internalEdits += 1
        defer { internalEdits -= 1 }
        body()
    }

    private func startCountdown() {
        countdownTask?.cancel()
        confirmCountdown = Self.confirmDelaySeconds
        let sleep = timing.sleep
        countdownTask = Task { [weak self] in
            while let self, self.confirmCountdown > 0 {
                do { try await sleep(.seconds(1)) } catch { return }
                guard !Task.isCancelled else { return }
                self.confirmCountdown -= 1
            }
        }
    }

    private func stopCountdown() {
        countdownTask?.cancel()
        countdownTask = nil
        confirmCountdown = 0
    }

    /// Engine errors that name a recipient go back to that entry; the rest
    /// end the flow with dash-qt / iOS copy.
    private func handle(_ error: ServiceError) {
        if let index = error.recipientIndex, entries.indices.contains(index) {
            let text = failure(for: error).message
            withInternalEdits {
                entries[index].error = error.code
                if Self.amountCodes.contains(error.code) {
                    entries[index].amountError = text
                } else {
                    entries[index].addressError = text
                }
            }
            phase = .editing
        } else {
            phase = .failed(failure(for: error))
        }
    }

    private static let amountCodes: Set<ServiceErrorCode> = [
        .sendInvalidAmount, .sendDustAmount, .sendAmountExceedsBalance, .sendAmountWithFeeExceedsBalance,
    ]

    func failure(for error: ServiceError) -> SendFailure {
        let withUnit = AmountStyle.withUnit(plusSign: false, separators: .always)
        let message: String
        switch error.code {
        case .sendInvalidAddress: message = L10n.Send.invalidAddress
        case .sendPlatformAddress: message = L10n.Send.platformAddress
        case .sendInvalidAmount: message = L10n.Send.invalidAmount
        case .sendDustAmount: message = L10n.Send.dustAmount
        case .sendAmountExceedsBalance: message = L10n.Send.amountExceedsBalance
        case .sendAmountWithFeeExceedsBalance:
            message = L10n.Send.amountWithFeeExceedsBalance(estimate.map { amounts.format($0.fee, unit: unit, style: withUnit) })
        case .sendAbsurdFee:
            message = L10n.Send.absurdFee(amounts.format(Self.maximumFee, unit: unit, style: withUnit))
        case .sendDuplicateAddress: message = L10n.Send.duplicateText
        case .sendNoRecipients: message = L10n.Send.noRecipients
        case .sendGrantExceeded: message = L10n.Send.grantExceeded
        case .sendBroadcastRejected: message = L10n.Send.broadcastRejected
        case .sendPreparedTxSpent: message = L10n.Send.preparedTxSpent
        case .sendNoPeers: message = L10n.Send.noPeers
        case EngineCode.sendInsufficientMixedFunds: message = L10n.Send.insufficientMixedFunds
        case EngineCode.sendOutpointUnavailable: message = L10n.Send.preselectedCoinsInsufficient
        case EngineCode.sendTxTooLarge: message = L10n.Send.txTooLarge
        case EngineCode.sendInvalidChangeAddress: message = L10n.Send.invalidChangeAddress
        case EngineCode.sendWatchOnly: message = L10n.Send.watchOnly
        case EngineCode.sendGrantInvalid: message = L10n.Common.unlockCancelled
        case .uriUnparsable, .uriInvalidAddress: message = L10n.Send.invalidAddress
        default:
            message = error.code.rawValue.hasPrefix("send.") ? L10n.Send.creationFailed : ErrorText.common(error.code)
        }
        return SendFailure(code: error.code, message: message)
    }

    static func addressProblem(_ problem: AddressInputProblem, network: DashNetwork) -> (ServiceErrorCode, String) {
        switch problem {
        case .empty, .invalid: (.sendInvalidAddress, L10n.Send.invalidAddress)
        case .wrongNetwork: (.sendInvalidAddress, L10n.Send.networkMismatch(L10n.Settings.networkName(network)))
        case .platformAddress: (.sendPlatformAddress, L10n.Send.platformAddress)
        case .shieldedAddress: (.sendInvalidAddress, L10n.Send.shieldedAddress)
        }
    }

    static func amountProblem(_ problem: AmountInputProblem) -> String {
        switch problem {
        case .unparsable: L10n.Send.unparsableAmount
        case .notPositive: L10n.Send.invalidAmount
        case .outOfRange: L10n.Send.amountTooLarge
        case .dust: L10n.Send.dustAmount
        }
    }

    /// Adds sent-to addresses to the sending address book (QT-063). A label
    /// from the send form only fills an empty label and never overwrites an
    /// existing one or a receiving entry (dash-qt quirk #6 fixed). Best
    /// effort: a failure here does not undo the sent transaction.
    private func rememberRecipients(_ sent: [RecipientEntry]) async {
        guard let addressBook, let wallet = walletState.selectedWalletID else { return }
        let existing = (try? await addressBook.entries(wallet: wallet, purpose: nil, search: nil)) ?? []
        let byAddress = Dictionary(existing.map { ($0.address, $0) }, uniquingKeysWith: { first, _ in first })
        var seen = Set<String>()
        for entry in sent where seen.insert(entry.address).inserted {
            if let current = byAddress[entry.address] {
                guard current.purpose == .send, current.label.isEmpty, !entry.label.isEmpty else { continue }
                _ = try? await addressBook.save(
                    wallet: wallet, address: entry.address, label: entry.label, purpose: .send, replace: true)
            } else {
                _ = try? await addressBook.save(
                    wallet: wallet, address: entry.address, label: entry.label, purpose: .send, replace: false)
            }
        }
    }
}
