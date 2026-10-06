// Transaction notifications (QT-031…033, IOS-116): the engine's
// `newTransactions` batches read into rows, and dash-qt's rules for
// showing them.
import DashKit
import Foundation
import Observation
import PlatformServices

extension TransactionNotice {
    init(_ kit: DashKit.TxNotice) {
        self.init(
            txid: kit.txid, recordIndex: kit.recordIndex, amount: Amount(kit.amount), date: kit.timestamp,
            type: TxType(kit.type), address: kit.address, label: kit.label, coinJoinInternal: kit.coinJoinInternal)
    }
}

/// `TransactionNotifying` over the engine: each `newTransactions` event of
/// the open network is read with `tx_notices` into one batch. Events of
/// other networks are ignored. A failed read (for example while the engine
/// call is not implemented) yields no batch and is kept in `lastError`;
/// notification rows are never invented.
public final class TransactionNotificationFeed: TransactionNotifying, @unchecked Sendable {
    private let context: EngineContext
    // `lock` guards `continuations`, `pump` and `error`.
    private let lock = NSLock()
    private var continuations: [UUID: AsyncStream<TransactionNoticeBatch>.Continuation] = [:]
    private var pump: Task<Void, Never>?
    private var error: ServiceError?

    init(context: EngineContext) {
        self.context = context
    }

    deinit {
        pump?.cancel()
    }

    /// The last failure to read a batch's rows.
    public var lastError: ServiceError? {
        lock.withLock { error }
    }

    public func batches() -> AsyncStream<TransactionNoticeBatch> {
        let (stream, continuation) = AsyncStream<TransactionNoticeBatch>.makeStream()
        let id = UUID()
        lock.withLock {
            continuations[id] = continuation
            if pump == nil { pump = startPump() }
        }
        continuation.onTermination = { [weak self] _ in
            self?.lock.withLock { _ = self?.continuations.removeValue(forKey: id) }
        }
        return stream
    }

    private func startPump() -> Task<Void, Never> {
        let subscription = context.engine.events.subscribe()
        return Task { [weak self] in
            for await event in subscription {
                guard case .newTransactions(let network, let wallet, let txids, let catchUp) = event else {
                    continue
                }
                await self?.read(network: network, wallet: wallet, txids: txids, catchUp: catchUp)
            }
        }
    }

    private func read(network: DashKit.DashNetwork, wallet: DashKit.WalletID, txids: [String], catchUp: Bool) async {
        guard context.active.network == network else { return }
        let engine = context.engine
        let rows: [DashKit.TxNotice]
        do {
            rows = try await serviceCall { () async throws(DashKitError) in
                try await engine.txNotices(on: network, wallet: wallet, txids: txids)
            }
        } catch {
            lock.withLock { self.error = error }
            return
        }
        let batch = TransactionNoticeBatch(
            network: DashNetwork(network), wallet: WalletID(wallet), notices: rows.map(TransactionNotice.init),
            catchUp: catchUp)
        let targets = lock.withLock {
            self.error = nil
            return Array(continuations.values)
        }
        for continuation in targets {
            continuation.yield(batch)
        }
    }
}

/// The user-visible texts of a notification (localized by the app; the
/// defaults are dash-qt's English).
public struct TransactionNotificationText: Sendable {
    public var incomingTitle = "Incoming transaction"
    public var sentTitle = "Sent transaction"
    public var receivedAndSentMany = "Received and sent multiple transactions"
    public var sentMany = "Sent multiple transactions"
    public var receivedMany = "Received multiple transactions"
    /// Each takes one value, written where `%1` is.
    public var dateLine = "Date: %1"
    public var amountLine = "Amount: %1"
    public var walletLine = "Wallet: %1"
    public var typeLine = "Type: %1"
    public var labelLine = "Label: %1"
    public var addressLine = "Address: %1"
    public var sentAmountLine = "Sent Amount: %1"
    public var receivedAmountLine = "Received Amount: %1"
    /// dash-qt's type column texts.
    public var typeName: @Sendable (TxType) -> String = { "\($0)" }
    public var date: @Sendable (Date) -> String = { date in
        let formatter = DateFormatter()
        formatter.dateStyle = .short
        formatter.timeStyle = .short
        return formatter.string(from: date)
    }

    public init() {}

    func line(_ template: String, _ value: String) -> String {
        template.replacingOccurrences(of: "%1", with: value)
    }
}

/// Shows the notifications of `TransactionNotifying` batches with dash-qt's
/// rules (bitcoingui.cpp `showIncomingTransactions`):
/// - nothing while notifications are off in the shell settings, and
///   nothing for a `catchUp` batch (initial sync);
/// - CoinJoin-internal rows only with "show CoinJoin popups" on (QT-033);
/// - 100 or more rows: one summary with the sent and received totals;
///   otherwise one notification per row with Date, Amount, Wallet (when
///   more than one wallet is loaded), Type, and Label or else Address.
/// Notifications open `dashwallet://tx/<wallet>/<txid>` when clicked.
@MainActor
@Observable
public final class NotificationPresenter {
    public static let summaryThreshold = 100

    /// Notifications posted (tests, diagnostics).
    public private(set) var postedCount = 0
    /// The last failure to post.
    public private(set) var lastError: ServiceError?

    @ObservationIgnored private let notifier: any SystemNotifying
    @ObservationIgnored private let shell: any ShellSettingsProviding
    @ObservationIgnored private let formatter: any AmountFormatting
    @ObservationIgnored private let unit: @MainActor () -> DisplayUnit
    @ObservationIgnored private let walletNames: @MainActor () -> [WalletID: String]
    @ObservationIgnored private let text: TransactionNotificationText
    @ObservationIgnored private let tasks = TaskBag()

    /// - Parameters:
    ///   - unit: the display unit for amounts.
    ///   - walletNames: names of the loaded wallets (the Wallet line shows
    ///     with two or more).
    public init(
        notifier: any SystemNotifying, shell: any ShellSettingsProviding, formatter: any AmountFormatting,
        unit: @escaping @MainActor () -> DisplayUnit, walletNames: @escaping @MainActor () -> [WalletID: String],
        text: TransactionNotificationText = TransactionNotificationText()
    ) {
        self.notifier = notifier
        self.shell = shell
        self.formatter = formatter
        self.unit = unit
        self.walletNames = walletNames
        self.text = text
    }

    /// Starts presenting the batches of `feed`.
    public func start(_ feed: any TransactionNotifying) {
        let stream = feed.batches()
        tasks.set("batches", Task { [weak self] in
            for await batch in stream {
                guard let self else { return }
                await self.present(batch)
            }
        })
    }

    public func stop() {
        tasks.cancelAll()
    }

    /// The notifications one batch produces under the current settings.
    public func notifications(for batch: TransactionNoticeBatch) -> [SystemNotification] {
        let settings = shell.shell
        guard settings.notificationsEnabled, !batch.catchUp else { return [] }
        let rows = batch.notices.filter { settings.showCoinJoinNotifications || !$0.coinJoinInternal }
        guard !rows.isEmpty else { return [] }
        let unit = unit()
        let amount = { (a: Amount) in self.formatter.format(a, unit: unit, style: .withUnit(plusSign: true, separators: .standard)) }
        if rows.count >= Self.summaryThreshold {
            let sent = rows.filter { $0.amount.duffs < 0 }
            let received = rows.filter { $0.amount.duffs >= 0 }
            let title: String
            switch (sent.isEmpty, received.isEmpty) {
            case (false, false): title = text.receivedAndSentMany
            case (false, true): title = text.sentMany
            default: title = text.receivedMany
            }
            var lines: [String] = []
            if !sent.isEmpty {
                lines.append(text.line(text.sentAmountLine, amount(Amount(duffs: sent.reduce(0) { $0 + $1.amount.duffs }))))
            }
            if !received.isEmpty {
                lines.append(
                    text.line(text.receivedAmountLine, amount(Amount(duffs: received.reduce(0) { $0 + $1.amount.duffs }))))
            }
            return [
                SystemNotification(
                    id: "tx-summary-\(batch.wallet.hex)-\(rows[0].txid)", title: title,
                    body: lines.joined(separator: "\n"), deepLink: "dashwallet://transactions/\(batch.wallet.hex)")
            ]
        }
        let names = walletNames()
        return rows.map { row in
            var lines: [String] = []
            if let date = row.date { lines.append(text.line(text.dateLine, text.date(date))) }
            lines.append(text.line(text.amountLine, amount(row.amount)))
            if names.count > 1, let name = names[batch.wallet], !name.isEmpty {
                lines.append(text.line(text.walletLine, name))
            }
            lines.append(text.line(text.typeLine, text.typeName(row.type)))
            if let label = row.label, !label.isEmpty {
                lines.append(text.line(text.labelLine, label))
            } else if let address = row.address, !address.isEmpty {
                lines.append(text.line(text.addressLine, address))
            }
            return SystemNotification(
                id: "tx-\(row.txid)-\(row.recordIndex)",
                title: row.amount.duffs < 0 ? text.sentTitle : text.incomingTitle,
                body: lines.joined(separator: "\n"),
                deepLink: "dashwallet://tx/\(batch.wallet.hex)/\(row.txid)")
        }
    }

    /// Posts the notifications of `batch`.
    public func present(_ batch: TransactionNoticeBatch) async {
        for notification in notifications(for: batch) {
            do {
                try await notifier.post(notification)
                postedCount += 1
                lastError = nil
            } catch {
                lastError = ServiceError(error)
            }
        }
    }
}
