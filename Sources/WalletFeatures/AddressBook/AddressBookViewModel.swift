// Address book (QT-095…098).
import Foundation
import Observation
import WalletRuntime

@MainActor
@Observable
public final class AddressBookViewModel {
    public private(set) var purpose: AddressPurpose
    public private(set) var search = ""
    /// Entries of `purpose` matching `search`, sorted by label (case-insensitive).
    public private(set) var entries: [AddressBookEntry] = []
    /// Picker for Send (sending list) and Sign (receiving list) (QT-097).
    public let selectionMode: Bool
    /// The entry chosen in selection mode.
    public private(set) var chosen: AddressBookEntry?
    public private(set) var errorMessage: String?
    /// The entry whose QR code is shown, its `dash:` URI and the code
    /// (dash-qt's "Show QR code", QT-095/096).
    public private(set) var qrEntry: AddressBookEntry?
    public private(set) var qrURI: String?
    public private(set) var qr: QRMatrix?

    public var header: String {
        purpose == .send ? L10n.AddressBook.sendingHeader : L10n.AddressBook.receivingHeader
    }

    /// Only sending entries can be created and deleted (QT-095/096).
    public var canCreate: Bool { purpose == .send }
    public var canDelete: Bool { purpose == .send }

    private let walletState: any WalletStateProviding
    private let addressBook: any AddressBookProviding
    private let uri: any URIHandling
    private let network: DashNetwork
    /// Every entry of both purposes, for the duplicate checks.
    private var all: [AddressBookEntry] = []

    public init(
        walletState: any WalletStateProviding, addressBook: any AddressBookProviding, uri: any URIHandling,
        network: DashNetwork, purpose: AddressPurpose, selectionMode: Bool = false
    ) {
        self.walletState = walletState
        self.addressBook = addressBook
        self.uri = uri
        self.network = network
        self.purpose = purpose
        self.selectionMode = selectionMode
    }

    public convenience init(env: AppEnvironment, network: DashNetwork, purpose: AddressPurpose, selectionMode: Bool = false) {
        self.init(
            walletState: env.walletState, addressBook: env.addressBook, uri: env.uri, network: network,
            purpose: purpose, selectionMode: selectionMode)
    }

    public func load() async {
        guard let wallet = walletState.selectedWalletID else {
            errorMessage = L10n.Common.noWallet
            return
        }
        do {
            all = try await addressBook.entries(wallet: wallet, purpose: nil, search: nil)
            errorMessage = nil
            applyFilter()
        } catch {
            errorMessage = text(for: error, address: "")
        }
    }

    public func setPurpose(_ purpose: AddressPurpose) {
        self.purpose = purpose
        hideQR()
        applyFilter()
    }

    /// dash-qt wildcard search on address or label: `*` and `?` wildcards,
    /// case-insensitive, matching anywhere in the text.
    public func setSearch(_ text: String) {
        search = text
        applyFilter()
    }

    /// New or edited entry. Returns `true` when saved. `replace` edits the
    /// label of an existing entry (the Edit dialog).
    @discardableResult
    public func save(address rawAddress: String, label: String, replace: Bool) async -> Bool {
        guard let wallet = walletState.selectedWalletID else { return false }
        let address = AddressInput.clean(rawAddress)
        errorMessage = nil
        if let existing = all.first(where: { $0.address == address }) {
            if existing.purpose == .receive && purpose == .send {
                errorMessage = L10n.AddressBook.existsAsReceiving(address, label: existing.label)
                return false
            }
            if !replace {
                errorMessage = L10n.AddressBook.alreadyInBook(address, label: existing.label)
                return false
            }
        } else {
            if purpose == .receive {
                // Receiving entries come from the Receive page; only labels are editable here.
                errorMessage = L10n.AddressBook.entryNotFound
                return false
            }
            if case .failure = AddressInput.checkCore(address, uri: uri, network: network) {
                errorMessage = L10n.AddressBook.invalidAddress(address)
                return false
            }
        }
        do {
            let saved = try await addressBook.save(
                wallet: wallet, address: address, label: label, purpose: purpose, replace: replace)
            all.removeAll { $0.address == saved.address }
            all.append(saved)
            applyFilter()
            return true
        } catch {
            errorMessage = text(for: error, address: address)
            return false
        }
    }

    /// Deletes a sending entry; receiving entries cannot be deleted (QT-096).
    public func delete(address: String) async {
        guard let wallet = walletState.selectedWalletID else { return }
        guard all.first(where: { $0.address == address })?.purpose != .receive else {
            errorMessage = L10n.AddressBook.receivingNotDeletable
            return
        }
        do {
            try await addressBook.delete(wallet: wallet, address: address)
            all.removeAll { $0.address == address }
            applyFilter()
        } catch {
            errorMessage = text(for: error, address: address)
        }
    }

    /// Selection mode: accepts an entry ("Choose" or double-click).
    public func choose(_ entry: AddressBookEntry) {
        guard selectionMode else { return }
        chosen = entry
    }

    /// Shows the QR code of `entry`: its `dash:` URI with the label, as
    /// dash-qt's address book builds it. A URI too long for a QR code
    /// (QT-084) shows the error instead.
    public func showQR(for entry: AddressBookEntry) {
        do {
            let text = try uri.buildPaymentURI(
                address: entry.address, amount: nil, label: entry.label.isEmpty ? nil : entry.label, message: nil)
            qr = try uri.qrMatrix(for: text)
            qrURI = text
            qrEntry = entry
            errorMessage = nil
        } catch {
            hideQR()
            errorMessage = error.code == EngineCode.uriTooLongForQR ? L10n.Receive.uriTooLong : ErrorText.common(error.code)
        }
    }

    public func hideQR() {
        qrEntry = nil
        qrURI = nil
        qr = nil
    }

    /// CSV with untranslated headers `Label`, `Address` of the visible list.
    public func exportCSV() -> String {
        let rows = [["Label", "Address"]] + entries.map { [$0.label, $0.address] }
        return rows.map { fields in
            fields.map { "\"" + $0.replacingOccurrences(of: "\"", with: "\"\"") + "\"" }.joined(separator: ",")
        }.joined(separator: "\n") + "\n"
    }

    /// Label column text with dash-qt's placeholder.
    public func labelText(for entry: AddressBookEntry) -> String {
        entry.label.isEmpty ? L10n.AddressBook.noLabel : entry.label
    }

    // MARK: Private

    private func applyFilter() {
        let matcher = WildcardMatcher(pattern: search)
        entries = all
            .filter { $0.purpose == purpose && (matcher.matches($0.address) || matcher.matches($0.label)) }
            .sorted { lhs, rhs in
                let order = lhs.label.localizedCaseInsensitiveCompare(rhs.label)
                return order == .orderedSame ? lhs.address < rhs.address : order == .orderedAscending
            }
    }

    private func text(for error: ServiceError, address: String) -> String {
        let label = all.first(where: { $0.address == address })?.label ?? ""
        switch error.code {
        case EngineCode.labelsInvalidAddress: return L10n.AddressBook.invalidAddress(address)
        case .labelsDuplicateAddress: return L10n.AddressBook.alreadyInBook(address, label: label)
        case EngineCode.labelsOwnAddress: return L10n.AddressBook.existsAsReceiving(address, label: label)
        case EngineCode.labelsReceiveEntryNotDeletable: return L10n.AddressBook.receivingNotDeletable
        case EngineCode.labelsEntryNotFound: return L10n.AddressBook.entryNotFound
        default: return ErrorText.common(error.code)
        }
    }
}

/// Qt wildcard matching as dash-qt's address-book filter uses it: `*` any
/// run, `?` one character, case-insensitive, unanchored. Empty matches all.
struct WildcardMatcher {
    private let pattern: [Character]

    init(pattern: String) {
        self.pattern = Array(pattern.lowercased())
    }

    func matches(_ text: String) -> Bool {
        guard !pattern.isEmpty else { return true }
        // Unanchored: same as matching "*pattern*".
        return Self.match(Array("*") + pattern + Array("*"), Array(text.lowercased()))
    }

    private static func match(_ pattern: [Character], _ text: [Character]) -> Bool {
        var p = 0
        var t = 0
        var star: Int?
        var mark = 0
        while t < text.count {
            if p < pattern.count, pattern[p] == "*" {
                star = p
                mark = t
                p += 1
            } else if p < pattern.count, pattern[p] == "?" || pattern[p] == text[t] {
                p += 1
                t += 1
            } else if let star {
                p = star + 1
                mark += 1
                t = mark
            } else {
                return false
            }
        }
        while p < pattern.count, pattern[p] == "*" { p += 1 }
        return p == pattern.count
    }
}
