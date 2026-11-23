// Conversions between DashKit values and the WalletRuntime contract values
// view models see. Contract types shadow the DashKit types of the same name
// in this module, so DashKit's are spelled `DashKit.X`.
import DashKit
import Foundation

// MARK: Errors

extension ServiceError {
    init(_ error: DashKitError) {
        self.init(
            code: ServiceErrorCode(rawValue: error.code), detail: error.detail,
            recipientIndex: error.recipientIndex, retryAfterSeconds: error.retryAfterSeconds,
            parameters: error.parameters)
    }
}

/// Runs an engine call and maps its error to `ServiceError`. The call stays
/// on the caller's isolation, so `body` may capture isolated state.
func serviceCall<T>(
    isolation: isolated (any Actor)? = #isolation,
    _ body: () async throws(DashKitError) -> T
) async throws(ServiceError) -> T {
    do {
        return try await body()
    } catch {
        throw ServiceError(error)
    }
}

/// Synchronous variant of `serviceCall`.
func serviceCall<T>(_ body: () throws(DashKitError) -> T) throws(ServiceError) -> T {
    do {
        return try body()
    } catch {
        throw ServiceError(error)
    }
}

// MARK: Common

extension Amount {
    init(_ kit: DashKit.Amount) {
        self.init(duffs: kit.duffs)
    }

    var kit: DashKit.Amount { DashKit.Amount(duffs: duffs) }
}

extension WalletID {
    /// `hex` is already a valid id (it came from DashKit).
    init(_ kit: DashKit.WalletID) {
        hex = kit.hex
    }

    /// The DashKit id. Throws `invalid_argument` for an id that bypassed
    /// validation (e.g. decoded from a corrupt settings file).
    var kit: DashKit.WalletID {
        get throws(ServiceError) {
            guard let id = DashKit.WalletID(hex: hex) else {
                throw ServiceError(code: .invalidArgument, detail: "malformed wallet id")
            }
            return id
        }
    }
}

extension DashNetwork {
    init(_ kit: DashKit.DashNetwork) {
        switch kit {
        case .mainnet: self = .mainnet
        case .testnet: self = .testnet
        case .devnet(let name): self = .devnet(name: name)
        case .regtest: self = .regtest
        }
    }

    var kit: DashKit.DashNetwork {
        switch self {
        case .mainnet: .mainnet
        case .testnet: .testnet
        case .devnet(let name): .devnet(name: name)
        case .regtest: .regtest
        }
    }
}

extension OutPoint {
    init(_ kit: DashKit.OutPoint) {
        self.init(txid: kit.txid, vout: kit.vout)
    }

    var kit: DashKit.OutPoint { DashKit.OutPoint(txid: txid, vout: vout) }
}

extension NetworkOptions {
    var kit: DashKit.SessionOptions {
        DashKit.SessionOptions(dapiAddresses: dapiAddresses, quorumURL: quorumURL, spvPeers: spvPeers)
    }
}

extension DashKit.SecretBytes: SecretBuffer {}

/// The bytes of any `SecretBuffer` as DashKit `SecretBytes`, copying only
/// when the buffer is not already one.
func secretBytes(_ buffer: any SecretBuffer) -> DashKit.SecretBytes {
    if let bytes = buffer as? DashKit.SecretBytes { return bytes }
    return buffer.withUnsafeBytes { DashKit.SecretBytes(copying: $0) }
}

// MARK: Wallets and vault

extension WalletBalances {
    init(_ kit: DashKit.WalletBalances) {
        self.init(
            confirmed: Amount(kit.confirmed), unconfirmed: Amount(kit.unconfirmed), immature: Amount(kit.immature),
            locked: Amount(kit.locked), total: Amount(kit.total), coinjoin: Amount(kit.coinjoin))
    }
}

extension WalletInfo {
    init(_ kit: DashKit.WalletInfo) {
        self.init(
            id: WalletID(kit.walletID), name: kit.name, watchOnly: kit.watchOnly, hasMnemonic: kit.hasMnemonic,
            hd: kit.hd, birthHeight: kit.birthHeight, createdAt: kit.createdAt,
            balances: kit.balances.map(WalletBalances.init))
    }
}

extension WalletImportOptions {
    var kit: DashKit.ImportOptions {
        DashKit.ImportOptions(name: name, birthHeight: birthHeight, coreCompatible: coreCompatible, lookahead: lookahead)
    }
}

extension MnemonicCheck {
    init(_ kit: DashKit.MnemonicCheck) {
        self.init(
            wordCount: kit.wordCount, unknownWordIndices: kit.unknownWordIndices,
            language: kit.language.map(MnemonicLanguage.init), checksum: MnemonicChecksum(kit.checksum))
    }
}

extension VaultStatus {
    init(_ kit: DashKit.VaultStatus) {
        self.init(
            state: VaultLockState(kit.state), encrypted: kit.encrypted, quickUnlockEnrolled: kit.quickUnlockEnrolled,
            failedAttempts: kit.failedAttempts, retryAfterSeconds: kit.retryAfterSeconds,
            walletsWithSecrets: kit.walletsWithSecrets.map(WalletID.init))
    }
}

extension GrantPurpose {
    init(_ kit: DashKit.GrantPurpose) {
        switch kit {
        case .spend(let max): self = .spend(max: Amount(max))
        case .revealSecret: self = .revealSecret
        case .signMessage: self = .signMessage
        case .changeCredential: self = .changeCredential
        case .wipe: self = .wipe
        case .platformOperation: self = .platformOperation
        }
    }

    var kit: DashKit.GrantPurpose {
        switch self {
        case .spend(let max): .spend(max: max.kit)
        case .revealSecret: .revealSecret
        case .signMessage: .signMessage
        case .changeCredential: .changeCredential
        case .wipe: .wipe
        case .platformOperation: .platformOperation
        }
    }
}

extension AuthGrant {
    init(_ kit: DashKit.AuthGrant) {
        self.init(id: kit.id, purpose: GrantPurpose(kit.purpose), expiresAt: kit.expiresAt, singleUse: kit.singleUse)
    }
}

extension Credential {
    var kit: DashKit.VaultCredential {
        switch self {
        case .passphrase(let secret): .passphrase(secretBytes(secret))
        case .quickUnlock(let key): .quickUnlock(secretBytes(key))
        case .unencrypted: .unencrypted
        }
    }
}

// MARK: Sync

extension SyncPhaseProgress {
    init(_ kit: DashKit.SyncPhaseProgress) {
        self.init(
            phase: SyncPhase(kit.phase), currentHeight: kit.currentHeight, targetHeight: kit.targetHeight,
            done: kit.done)
    }
}

extension PeerInfo {
    init(_ kit: DashKit.PeerInfo) {
        self.init(
            address: kit.address, userAgent: kit.userAgent, protocolVersion: kit.protocolVersion,
            bestHeight: kit.bestHeight, pingMilliseconds: kit.pingMilliseconds, connectedSince: kit.connectedSince,
            inbound: kit.inbound)
    }
}

extension RescanStart {
    var kit: DashKit.RescanStart {
        switch self {
        case .walletBirth: .walletBirth
        case .genesis: .genesis
        case .height(let h): .height(h)
        }
    }
}

// MARK: History

extension TxStatus {
    init(_ kit: DashKit.TxStatus) {
        self.init(
            kind: TxStatusKind(kit.kind), confirmations: kit.confirmations, instantLocked: kit.instantLocked,
            chainLocked: kit.chainLocked, maturesIn: kit.maturesIn)
    }
}

extension TxRecord {
    init(_ kit: DashKit.TxRecord) {
        self.init(
            id: ID(txid: kit.txid, recordIndex: kit.recordIndex), type: TxType(kit.type),
            category: TxCategory(kit.category), status: TxStatus(kit.status), date: kit.date,
            blockHeight: kit.blockHeight, amount: Amount(kit.amount), fee: kit.fee.map(Amount.init),
            address: kit.address, label: kit.label, countsTowardBalance: kit.countsTowardBalance,
            involvesWatchOnly: kit.involvesWatchOnly)
    }
}

extension HistoryFilter {
    var kit: DashKit.HistoryFilter {
        DashKit.HistoryFilter(
            types: Set(types.map(\.kit)), categories: Set(categories.map(\.kit)), statuses: Set(statuses.map(\.kit)),
            from: from, until: until, text: text, minimumAmount: minimumAmount?.kit, watchOnly: watchOnly.kit)
    }
}

extension HistoryQuery {
    var kit: DashKit.HistoryQuery {
        DashKit.HistoryQuery(filter: filter.kit, sort: sort.kit, cursor: cursor, limit: limit)
    }
}

extension HistoryPage {
    init(_ kit: DashKit.HistoryPage) {
        self.init(records: kit.records.map(TxRecord.init), nextCursor: kit.nextCursor, totalMatching: kit.totalMatching)
    }
}

extension TransactionDetail {
    init(_ kit: DashKit.TxDetail) {
        self.init(
            txid: kit.txid, records: kit.records.map(TxRecord.init), status: TxStatus(kit.status), date: kit.date,
            blockHeight: kit.blockHeight, blockHash: kit.blockHash, fee: kit.fee.map(Amount.init),
            sizeBytes: kit.sizeBytes,
            inputs: kit.inputs.map {
                TxInput(previousOutput: OutPoint($0.previousOutput), address: $0.address, amount: $0.amount.map(Amount.init), isMine: $0.isMine)
            },
            outputs: kit.outputs.map {
                TxOutput(vout: $0.vout, address: $0.address, amount: Amount($0.amount), isMine: $0.isMine, isChange: $0.isChange, dataHex: $0.dataHex)
            },
            message: kit.message, label: kit.label, rawHex: kit.rawHex)
    }
}

// MARK: Receive

extension AddressInfo {
    init(_ kit: DashKit.AddressInfo) {
        self.init(
            address: kit.address, chain: AddressChain(kit.chain), index: kit.index, derivationPath: kit.derivationPath,
            used: kit.used, label: kit.label, balance: kit.balance.map(Amount.init), txCount: kit.txCount)
    }
}

extension AddressFilter {
    var kit: DashKit.AddressFilter { DashKit.AddressFilter(chain: chain?.kit, used: used) }
}

extension ReceiveRequest {
    init(_ kit: DashKit.ReceiveRequest) {
        self.init(
            id: kit.id, createdAt: kit.createdAt, address: kit.address, amount: kit.amount.map(Amount.init),
            label: kit.label, message: kit.message, uri: kit.uri)
    }
}

// MARK: Send

extension PaymentRecipient {
    var kit: DashKit.Recipient {
        DashKit.Recipient(
            address: address, amount: amount.kit, subtractFeeFromAmount: subtractFeeFromAmount, label: label,
            message: message)
    }
}

extension CoinSourceChoice {
    var kit: DashKit.CoinSource {
        switch self {
        case .any: .any
        case .fullyMixed: .fullyMixedOnly
        case .outpoints(let outpoints): .outpoints(outpoints.map(\.kit))
        }
    }
}

extension FeeChoice {
    var kit: DashKit.FeeMode {
        switch self {
        case .recommended(let blocks): .recommended(targetBlocks: blocks)
        case .perKilobyte(let rate): .perKilobyte(rate.kit)
        }
    }
}

extension ChangeChoice {
    var kit: DashKit.ChangePolicy {
        switch self {
        case .automatic: .automatic
        case .address(let address): .address(address)
        }
    }
}

extension TxEstimate {
    init(_ kit: DashKit.TxEstimate) {
        self.init(
            fee: Amount(kit.fee), sizeBytes: kit.sizeBytes, inputCount: kit.inputCount,
            change: kit.change.map(Amount.init), totalSent: Amount(kit.totalSent))
    }
}

extension PreparedTxSummary {
    init(_ kit: DashKit.PreparedTxSummary) {
        self.init(
            txid: kit.txid, fee: Amount(kit.fee), feeRatePerKilobyte: Amount(kit.feeRatePerKilobyte),
            sizeBytes: kit.sizeBytes, inputCount: kit.inputs.count,
            outputs: kit.outputs.map {
                PreparedOutput(
                    address: $0.address, amount: Amount($0.amount), isChange: $0.isChange, label: $0.label,
                    isMine: $0.isMine)
            },
            totalSent: Amount(kit.totalSent), totalDebit: Amount(kit.totalDebit),
            externalSent: kit.externalSent.map(Amount.init))
    }
}

// MARK: Coins and labels

extension Utxo {
    init(_ kit: DashKit.Utxo) {
        self.init(
            outpoint: OutPoint(kit.outpoint), address: kit.address, amount: Amount(kit.amount),
            confirmations: kit.confirmations, date: kit.date, instantLocked: kit.instantLocked,
            chainLocked: kit.chainLocked, userLocked: kit.userLocked, reserved: kit.reserved, label: kit.label,
            isChange: kit.isChange, coinJoinDenominated: kit.coinJoinDenominated, coinJoinRounds: kit.coinJoinRounds,
            spendable: kit.spendable)
    }
}

extension UtxoFilter {
    var kit: DashKit.UtxoFilter {
        DashKit.UtxoFilter(
            includeLocked: includeLocked, fullyMixedOnly: fullyMixedOnly, minimumConfirmations: minimumConfirmations)
    }
}

extension AddressBookEntry {
    init(_ kit: DashKit.AddressBookEntry) {
        self.init(address: kit.address, label: kit.label, purpose: AddressPurpose(kit.purpose), createdAt: kit.createdAt)
    }
}

// MARK: URI, QR, units

extension PaymentURI {
    init(_ kit: DashKit.PaymentURI) {
        self.init(address: kit.address, amount: kit.amount.map(Amount.init), label: kit.label, message: kit.message)
    }
}

extension AddressClass {
    init(_ kit: DashKit.AddressClass) {
        switch kit {
        case .core(let p2sh): self = .core(scriptHash: p2sh)
        case .platform: self = .platform
        case .shielded: self = .shielded
        case .invalid(let problem):
            switch problem {
            case .invalidBase58Length: self = .invalid(.invalidBase58Length)
            case .invalidBase58Prefix: self = .invalid(.invalidBase58Prefix)
            case .notBech32mOrBase58: self = .invalid(.notBech32mOrBase58)
            case .invalidBase58ChecksumOrLength: self = .invalid(.invalidBase58ChecksumOrLength)
            case .platformAddress: self = .invalid(.platformAddress)
            case .bech32: self = .invalid(.bech32)
            }
        }
    }
}

extension AmountSeparators {
    var kit: DashKit.Separators {
        switch self {
        case .never: .never
        case .standard: .standard
        case .always: .always
        }
    }
}

extension AmountStyle {
    /// `floored` digits are clamped to dash-qt's 0...8.
    var kit: DashKit.AmountStyle {
        switch self {
        case .plain(let plus, let separators): .plain(plusSign: plus, separators: separators.kit)
        case .withUnit(let plus, let separators): .withUnit(plusSign: plus, separators: separators.kit)
        case .floored(let plus, let separators, let digits):
            .floored(plusSign: plus, separators: separators.kit, digits: UInt8(min(max(digits, 0), 8)))
        case .privacy(let separators, let hidden): .privacy(separators: separators.kit, hidden: hidden)
        case .gui(let signed, let truncate):
            .gui(signed: signed, truncate: truncate.map { UInt8(min(max($0, 0), 8)) })
        }
    }
}
