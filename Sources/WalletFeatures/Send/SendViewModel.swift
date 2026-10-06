// Send (QT-051…063, QT-067, IOS-041…052; DESIGN-opus §1.11).
//
// Flow: editing → [confirmDuplicates] → [authorizing] → preparing → confirm
// (3 s countdown) → broadcasting → done | failed | broadcastUnknown.
// `prepare` signs and reserves inputs but never broadcasts; only `confirm()`
// (or `broadcastAgain()` after an unknown outcome) broadcasts (iOS rule 4).
//
// Releasing reserved inputs (review M-7):
// - Cancel, or any edit of entries, fee or source while a payment is under
//   review (duplicates question, authorizing, preparing, confirm, failed),
//   abandons the prepared transaction and revokes a grant `prepare` has not
//   redeemed. A prepare that finishes after such an edit is abandoned on
//   arrival.
// - A broadcast the network definitely did not take ends in `.failed`. After
//   `send.no_peers`, `send.broadcast_rejected` and `send.prepared_tx_spent`
//   the engine has already released the inputs and spent the prepared
//   transaction, so sending again means a new review: new grant, new
//   prepare (review M1). The other definite failures are errors the engine
//   raises before it hands the transaction to the network (m1-engine.md
//   §2.7.1 "not dispatched", review L5): the transaction stays pending and
//   dismissing abandons it.
// - A broadcast whose outcome is unknown (any other error) ends in
//   `.broadcastUnknown` and is never abandoned: its inputs may already be in
//   the mempool, and releasing them would let the next send double-spend.
//   The prepared transaction is kept so `broadcastAgain()` can send the same
//   signed transaction (same txid) again (review M3). The form is read-only
//   until the user dismisses it (review L6).
// - The engine records the recipients in the address book once a broadcast
//   was accepted or its outcome is unknown (m1-engine.md §2.7.1 "Address
//   book"); this view model writes no address-book entries.
//
// Coin control (review M3): with a `CoinControlViewModel` attached, its
// selection is the only source of truth. The draft reads it when it is
// built (review, Create Unsigned, Use available balance), a selection edit
// counts as a user edit, and Clear All and a broadcast (sent, or outcome
// unknown and dismissed) clear it, as dash-qt's `UnSelectAll` does.
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

/// Why `SendViewModel.setSource` refused a source; the text is for the user.
public enum SendSourceRefusal: Error, Sendable, Equatable {
    /// The CoinJoin page always spends fully mixed coins (QT-051).
    case coinJoinPage
    /// Broadcasting, or a broadcast's outcome is unknown: dismiss it first.
    case notEditable
    /// The Coin Selection dialog owns the selection; change it there.
    case coinControlAttached

    public var message: String {
        switch self {
        case .coinJoinPage: L10n.Send.sourceFixedOnCoinJoinPage
        case .notEditable: L10n.CoinControl.selectionLockedWhileSending
        case .coinControlAttached: L10n.Send.sourceOwnedByCoinControl
        }
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
    /// transaction. Its inputs stay reserved; the user checks Transactions
    /// or broadcasts it again (`canBroadcastAgain`). `failure` is the last
    /// attempt's error.
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

    /// Editable by the UI while `isEditable`. A change of the user fields
    /// while a payment is under review returns to `.editing` and abandons the
    /// prepared transaction (review M-7). A change of the user fields while
    /// the form is read-only (broadcasting, outcome unknown) is undone (review
    /// L6).
    public var entries: [RecipientEntry] = [RecipientEntry()] {
        didSet {
            guard internalEdits == 0, oldValue.map(\.userFields) != entries.map(\.userFields) else { return }
            guard isEditable else {
                // `entries` is an observed property: the undo goes through its
                // setter, so it is marked internal to end the recursion.
                withInternalEdits { entries = oldValue }
                return
            }
            userEdited()
        }
    }
    public private(set) var phase: SendPhase = .editing
    public let page: SendPage
    /// What the draft spends: the attached Coin Selection dialog's selection
    /// (read now, never cached), else the source set with `setSource`.
    public var source: CoinSourceChoice { coinControl?.source() ?? chosenSource }
    /// The Coin Selection dialog this page spends from (`attach`).
    public private(set) var coinControl: CoinControlViewModel?
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

    /// After an unknown broadcast outcome, while the engine still holds the
    /// signed transaction: `broadcastAgain()` sends it again (same txid).
    public var canBroadcastAgain: Bool {
        if case .broadcastUnknown = phase { return prepared != nil }
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

    /// The recipients' amounts in the display unit, for the coin-control
    /// summary (QT-072); empty and unparsable amounts are left out.
    public var coinControlAmounts: [Amount] {
        entries.compactMap { entry in
            entry.amountText.isEmpty ? nil : try? amounts.parse(entry.amountText, unit: unit)
        }
    }

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
    private let network: DashNetwork
    private let timing: Timing
    private var draft: (any TransactionDrafting)?
    /// The wallet `draft` spends from; spend grants are bound to it.
    private var draftWallet: WalletID?
    private var prepared: PreparedTransaction?
    private var spendLimit: Amount?
    /// A grant issued for this payment that `prepare` has not redeemed.
    private var pendingGrant: AuthGrant?
    /// Bumped by every cancel and edit; async steps that started under an
    /// older value drop (and abandon) their result.
    private var generation = 0
    /// Non-zero while the view model itself changes `entries`.
    private var internalEdits = 0
    private var countdownTask: Task<Void, Never>?
    /// The source without a Coin Selection dialog: `.any`, `.fullyMixed` on
    /// the CoinJoin page, or what `setSource` chose.
    private var chosenSource: CoinSourceChoice

    public init(
        walletState: any WalletStateProviding, sender: any TransactionSending, auth: any AuthenticationGating,
        vault: any VaultProviding, uri: any URIHandling, amounts: any AmountFormatting,
        settings: any SettingsProviding, sync: (any SyncStatusProviding)?, network: DashNetwork, timing: Timing,
        page: SendPage = .regular
    ) {
        self.walletState = walletState
        self.sender = sender
        self.auth = auth
        self.vault = vault
        self.uri = uri
        self.amounts = amounts
        self.settings = settings
        self.sync = sync
        self.network = network
        self.timing = timing
        self.page = page
        self.chosenSource = page == .coinJoin ? .fullyMixed : .any
    }

    public convenience init(env: AppEnvironment, network: DashNetwork, page: SendPage = .regular) {
        self.init(
            walletState: env.walletState, sender: env.sender, auth: env.auth, vault: env.vault, uri: env.uri,
            amounts: env.amounts, settings: env.settings, sync: env.sync, network: network, timing: env.timing, page: page)
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

    /// Makes `coinControl`'s selection the coins this page spends (review
    /// M3). Its page must match: the CoinJoin dialog for the CoinJoin page.
    public func attach(_ coinControl: CoinControlViewModel) {
        precondition(coinControl.coinJoinPage == (page == .coinJoin), "coin control of another send page")
        self.coinControl?.send = nil
        self.coinControl = coinControl
        coinControl.send = self
    }

    /// The selected wallet changed: the coin selection (coins of the old
    /// wallet) is emptied and a review prepared from the old wallet ends as
    /// an edit would (its grant revoked, its prepared transaction
    /// abandoned). The recipients stay. A payment broadcasting or of unknown
    /// outcome is left to finish.
    public func walletChanged() {
        withInternalEdits { clearCoinSelection() }
        userEdited()
    }

    /// Clear All: entries, coin selection, fee warning (QT-052).
    public func clearAll() {
        guard isEditable else { return }
        withInternalEdits {
            entries = [RecipientEntry()]
            clearCoinSelection()
        }
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

    /// The coins to spend on a page without a Coin Selection dialog. A
    /// refusal says why, so a host never drops a selection silently.
    public func setSource(_ source: CoinSourceChoice) throws(SendSourceRefusal) {
        guard page == .regular else { throw .coinJoinPage }
        guard coinControl == nil else { throw .coinControlAttached }
        guard source != chosenSource else { return }
        guard isEditable else { throw .notEditable }
        chosenSource = source
        userEdited()
    }

    /// The attached dialog's selection changed by the user: like any edit,
    /// it ends a review in progress (review M-7).
    func coinSelectionEdited() {
        guard internalEdits == 0 else { return }
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
        if Set(recipients.map(\.address)).count < recipients.count {
            phase = .confirmDuplicates
            return
        }
        await buildDraft(recipients)
    }

    // MARK: Create Unsigned (QT-076)

    /// dash-qt "Create Unsigned": a draft with the page's recipients, coins
    /// and fee that is never signed, for `PSBTViewModel.createUnsigned`.
    /// `nil` when the form does not validate (the entries show why) or the
    /// engine refuses the draft (`phase` becomes `.failed`).
    public func makeUnsignedDraft() async -> (any TransactionDrafting)? {
        guard phase == .editing, page == .regular, let wallet = walletState.selectedWalletID else { return nil }
        guard let recipients = validateEntries() else { return nil }
        do {
            let draft = try await sender.makeDraft(wallet: wallet)
            try await draft.setRecipients(recipients)
            try await draft.setSource(source)
            try await draft.setFee(fee)
            return draft
        } catch {
            handle(error)
            return nil
        }
    }

    /// Yes on "Confirm duplicate recipients" (QT-060). The engine refuses an
    /// address twice (`send.duplicate_address`), so the entries that pay the
    /// same address are merged into the first of them: amounts summed,
    /// subtract-fee if any of them had it, the first non-empty label, their
    /// distinct messages joined by newlines. The form shows the merged entry,
    /// then the review continues.
    public func acknowledgeDuplicates() async {
        guard phase == .confirmDuplicates else { return }
        guard mergeDuplicateEntries() else {
            phase = .editing
            return
        }
        await review()
    }

    /// Passphrase for the spend grant, while `phase == .authorizing`. The
    /// phase moves to `.preparing` before the vault is asked, so a second call
    /// while the passphrase is being checked does nothing.
    public func authorize(passphrase: String) async {
        guard phase == .authorizing, let spendLimit, let draftWallet else { return }
        phase = .preparing
        let secret = vault.makeSecret(utf8: passphrase)
        let started = generation
        do {
            let grant = try await auth.authorize(
                .spend(max: spendLimit), wallet: draftWallet, credential: .passphrase(secret))
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

    /// "Broadcast again" after an unknown outcome: sends the same signed
    /// transaction (same txid), which the engine allows only in that state.
    /// Whatever this attempt's error, the first attempt's outcome stays
    /// unknown, so the phase stays `.broadcastUnknown` unless it succeeds.
    public func broadcastAgain() async {
        guard canBroadcastAgain else { return }
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

    /// Leaves a final phase. After a failure it abandons like `cancel()` and
    /// keeps the form, so Review starts a new grant and prepare. After
    /// `.done` and `.broadcastUnknown` it clears the form and abandons
    /// nothing: the engine refuses to release a transaction whose broadcast
    /// outcome is unknown (`send.prepared_tx_spent`), so its inputs stay
    /// reserved and the handle is dropped.
    public func dismiss() async {
        switch phase {
        case .failed:
            await cancel()
        case .done, .broadcastUnknown:
            generation += 1
            prepared = nil
            draft = nil
            withInternalEdits {
                entries = [RecipientEntry()]
                clearCoinSelection()
            }
            estimate = nil
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

    /// The spend cap requested for `recipients` paying `fee`: the sum of
    /// their amounts plus the estimated fee. The engine caps what leaves the
    /// wallet, fee included (`total_debit`, m1-engine.md §2.7.1), the same
    /// outflow `sign_psbt` caps, so a quick-unlock spending limit means the
    /// same in Send and PSBT (IOS-016). With subtract-fee or recipients the
    /// wallet owns the outflow is lower. This view model never sets a change
    /// address, so change always returns to the wallet and adds nothing; a
    /// host that sets a change address the wallet does not own must add the
    /// change amount.
    public static func spendLimit(for recipients: [PaymentRecipient], fee: Amount) -> Amount {
        Amount(duffs: recipients.reduce(fee.duffs) { $0 + $1.amount.duffs })
    }

    /// Errors of a first broadcast after which peers certainly do not have
    /// the transaction: never sent (`send.no_peers`, `send.broadcast_rejected`,
    /// `send.prepared_tx_spent`), not held by the draft, or raised by the
    /// engine before dispatch, which leaves the transaction pending
    /// (m1-engine.md §2.7.1 "not dispatched": `network_not_open`,
    /// `wallet_not_found`, `invalid_argument`, `wallet`, `storage`, `spv`,
    /// `io`; review L5).
    public static let definiteBroadcastFailures: Set<ServiceErrorCode> = [
        .sendBroadcastRejected, .sendPreparedTxSpent, .sendNoPeers, .sendPreparedTxUnknown, .networkNotOpen,
        .walletNotFound, .invalidArgument, .notImplemented, .wallet, .storage, .spv, .io,
    ]

    /// Broadcast errors after which the prepared transaction no longer
    /// exists: the engine released its inputs (`send.no_peers`,
    /// `send.broadcast_rejected`, `send.prepared_tx_spent`) or the draft does
    /// not hold it (`send.prepared_tx_unknown`). Nothing is left to abandon or
    /// broadcast; sending needs a new prepare and grant.
    static let preparedGoneAfter: Set<ServiceErrorCode> = [
        .sendNoPeers, .sendBroadcastRejected, .sendPreparedTxSpent, .sendPreparedTxUnknown,
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
            draftWallet = wallet
            try await draft.setRecipients(recipients)
            try await draft.setSource(source)
            try await draft.setFee(fee)
            let estimate = try await draft.estimate()
            guard started == generation else { return }
            self.estimate = estimate
            let limit = Self.spendLimit(for: recipients, fee: estimate.fee)
            spendLimit = limit
            switch auth.requirement(for: .spend(max: limit)) {
            case .none:
                let grant = try await auth.authorize(.spend(max: limit), wallet: wallet, credential: .unencrypted)
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

    /// Broadcasts `prepared` from `.confirm` or, again, from `.broadcastUnknown`.
    private func broadcast() async {
        guard let draft, let prepared else { return }
        let isRetry: Bool
        if case .broadcastUnknown = phase { isRetry = true } else { isRetry = false }
        stopCountdown()
        phase = .broadcasting
        do {
            let result = try await draft.broadcast(prepared)
            self.prepared = nil
            self.draft = nil
            // dash-qt `UnSelectAll` after a send: the spent coins are gone.
            withInternalEdits {
                entries = [RecipientEntry()]
                clearCoinSelection()
            }
            estimate = nil
            phase = .done(txid: result.txid)
            route = .transaction(txid: result.txid)
        } catch {
            let failure = failure(for: error)
            if Self.preparedGoneAfter.contains(error.code) {
                self.prepared = nil
            }
            let txid = prepared.summary.txid
            if isRetry {
                // The first attempt may still have reached a peer.
                phase = .broadcastUnknown(txid: txid, failure: failure)
            } else if Self.definiteBroadcastFailures.contains(error.code) {
                phase = .failed(failure)
            } else {
                // Keep the draft and the prepared transaction: the engine
                // refuses to abandon it but may broadcast it again.
                phase = .broadcastUnknown(txid: txid, failure: failure)
                route = .transaction(txid: txid)
            }
        }
    }

    /// A user change of entries, fee or source.
    private func userEdited() {
        estimate = nil
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
        if let draft, let prepared {
            try? await draft.abandon(prepared)
        }
    }

    /// Empties the attached dialog's selection, or resets `setSource`.
    private func clearCoinSelection() {
        coinControl?.clearSelection()
        if page == .regular { chosenSource = .any }
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
        .sendAmountTooSmallAfterFee,
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
            // The engine reports the fee with the error (review M6); no
            // estimate exists when `estimate()` itself failed this way.
            let fee = error.parameters["fee"].map { Amount(duffs: $0) } ?? estimate?.fee
            message = L10n.Send.amountWithFeeExceedsBalance(fee.map { amounts.format($0, unit: unit, style: withUnit) })
        case .sendAmountTooSmallAfterFee: message = L10n.Send.amountTooSmallAfterFee
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

    /// Merges entries that pay the same address (see `acknowledgeDuplicates`).
    /// The first entry of each address keeps its place and id; a merged
    /// amount is re-formatted in the display unit. Returns `false`, leaving
    /// the entries as they are, when an amount does not parse.
    private func mergeDuplicateEntries() -> Bool {
        var merged: [RecipientEntry] = []
        var totals: [Int64] = []
        var counts: [Int] = []
        var indexByAddress: [String: Int] = [:]
        for entry in entries {
            guard case .success(let amount) = AmountInput.parsePayment(entry.amountText, unit: unit, formatter: amounts)
            else { return false }
            let address = AddressInput.clean(entry.address)
            guard let index = indexByAddress[address] else {
                indexByAddress[address] = merged.count
                var first = entry
                first.address = address
                merged.append(first)
                totals.append(amount.duffs)
                counts.append(1)
                continue
            }
            // Saturates instead of trapping; validation then reports the
            // amount as larger than 21 million DASH.
            let (sum, overflow) = totals[index].addingReportingOverflow(amount.duffs)
            totals[index] = overflow ? Int64.max : sum
            counts[index] += 1
            merged[index].subtractFee = merged[index].subtractFee || entry.subtractFee
            if merged[index].label.isEmpty { merged[index].label = entry.label }
            if let message = entry.message, !message.isEmpty {
                let current = merged[index].message ?? ""
                if current.isEmpty {
                    merged[index].message = message
                } else if !current.split(separator: "\n", omittingEmptySubsequences: false).contains(Substring(message)) {
                    merged[index].message = current + "\n" + message
                }
            }
        }
        for index in merged.indices where counts[index] > 1 {
            merged[index].amountText = amounts.format(
                Amount(duffs: totals[index]), unit: unit, style: .plain(plusSign: false, separators: .never))
            merged[index].error = nil
            merged[index].addressError = nil
            merged[index].amountError = nil
        }
        withInternalEdits { entries = merged }
        return true
    }
}
