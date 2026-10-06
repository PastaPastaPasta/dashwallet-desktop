// dash-qt's Create Proposal dialog (QT-132): name, URL, payment date (the
// next 12 superblocks), payments 1–12, address and amount with the derived
// total, View JSON / View Payload, the non-refundable fee confirmation, then
// the 1 DASH collateral transaction and the hand-off to Resume Proposals. The
// chosen payment date is honoured (dash-qt ignores it, research 02 §21).
import Foundation
import Observation
import WalletRuntime

public enum CreateProposalStep: Sendable, Hashable {
    case loading
    case unavailable
    case editing
    /// "Creating a proposal pays 1 DASH to the network…" (Yes/No).
    case confirming
    case needsPassphrase
    case creating
    /// Created; the UI shows the message and opens Resume Proposals.
    case created(PendingProposal)
}

@MainActor
@Observable
public final class CreateProposalWizardViewModel {
    /// The most the collateral transaction's network fee may take on top of
    /// the 1 DASH collateral (the grant's spend limit; the engine caps it).
    public static let feeAllowance = Amount(duffs: 100_000)

    public var name = ""
    public var url = ""
    public var paymentAddress = ""
    public var amountText = ""
    public var paymentCount = 1
    /// The chosen first superblock (one of `superblocks`).
    public var firstSuperblock: UInt32?

    public private(set) var step: CreateProposalStep = .loading
    public private(set) var superblocks: [SuperblockDate] = []
    /// Failing fields, in field order.
    public private(set) var fieldErrors: [ProposalField] = []
    public private(set) var errorMessage: String?
    /// "View JSON" / "View Payload" output.
    public private(set) var preview: String?

    public let parameters: GovernanceParameters
    public var title: String { L10n.Governance.createTitle }
    public var paymentDateHelp: String { L10n.Governance.paymentDateHonoured }
    public var paymentCountRange: ClosedRange<Int> { 1...parameters.maxPayments }

    public var superblockOptions: [(height: UInt32, title: String)] {
        superblocks.map { ($0.height, L10n.Governance.superblockOption(
            height: $0.height, date: M3Dates.date($0.estimatedDate, timing: timing))) }
    }

    /// Payment amount × payments, in the display unit.
    public var totalText: String {
        guard let amount = parsedAmount else { return "" }
        return format(Amount(duffs: amount.duffs * Int64(paymentCount)))
    }

    public var confirmationTitle: String { L10n.Governance.confirmTitle }
    public var confirmationQuestion: String { L10n.Governance.confirmQuestion }
    public var confirmationDetail: String { L10n.Governance.confirmFee(format(parameters.proposalFee)) }

    public var createdMessage: String? {
        guard case .created = step else { return nil }
        return L10n.Governance.created(fee: format(parameters.proposalFee), name: name)
    }

    public func fieldErrorText(_ field: ProposalField) -> String? {
        fieldErrors.contains(field) ? L10n.Governance.fieldError(field) : nil
    }

    private let proposals: any ProposalCreating
    private let walletState: any WalletStateProviding
    private let grants: GrantRequester
    private let amounts: any AmountFormatting
    private let settings: any SettingsProviding
    private let timing: Timing

    public init(
        proposals: any ProposalCreating, parameters: GovernanceParameters, walletState: any WalletStateProviding,
        auth: any AuthenticationGating, vault: any VaultProviding, amounts: any AmountFormatting,
        settings: any SettingsProviding, timing: Timing
    ) {
        self.proposals = proposals
        self.parameters = parameters
        self.walletState = walletState
        grants = GrantRequester(auth: auth, vault: vault)
        self.amounts = amounts
        self.settings = settings
        self.timing = timing
    }

    public convenience init(env: AppEnvironment, m3: M3Services) {
        self.init(
            proposals: m3.proposals, parameters: m3.governance.parameters(), walletState: env.walletState,
            auth: env.auth, vault: env.vault, amounts: env.amounts, settings: env.settings, timing: env.timing)
    }

    private func format(_ amount: Amount) -> String { AmountText(amounts: amounts, settings: settings)(amount) }

    private var parsedAmount: Amount? {
        let text = amountText.trimmingCharacters(in: .whitespaces)
        guard !text.isEmpty, let amount = try? amounts.parse(text, unit: settings.display.unit), amount > .zero else {
            return nil
        }
        return amount
    }

    private var draft: ProposalDraft? {
        guard let firstSuperblock else { return nil }
        return ProposalDraft(
            name: name.trimmingCharacters(in: .whitespaces), url: url.trimmingCharacters(in: .whitespaces),
            paymentAddress: paymentAddress.trimmingCharacters(in: .whitespaces),
            paymentAmount: parsedAmount ?? .zero, paymentCount: paymentCount, firstSuperblockHeight: firstSuperblock)
    }

    /// The next 12 superblocks for the payment date.
    public func load() async {
        step = .loading
        do {
            superblocks = try await proposals.superblockDates(count: parameters.maxPayments)
        } catch {
            if error.isNotImplemented {
                step = .unavailable
                return
            }
            errorMessage = ErrorText.m3(error, amount: format)
            step = .editing
            return
        }
        if firstSuperblock == nil { firstSuperblock = superblocks.first?.height }
        step = .editing
    }

    /// The engine's field check; empty = valid. "All fields are mandatory"
    /// comes first, like dash-qt.
    @discardableResult
    public func validate() async -> Bool {
        errorMessage = nil
        guard !name.trimmingCharacters(in: .whitespaces).isEmpty, !url.trimmingCharacters(in: .whitespaces).isEmpty,
            !paymentAddress.trimmingCharacters(in: .whitespaces).isEmpty, parsedAmount != nil
        else {
            errorMessage = L10n.Governance.allFieldsMandatory
            return false
        }
        guard let draft else {
            fieldErrors = [.firstPayment]
            return false
        }
        do {
            fieldErrors = try await proposals.validate(draft)
        } catch {
            errorMessage = ErrorText.m3(error, amount: format)
            return false
        }
        return fieldErrors.isEmpty
    }

    public func viewJSON() async {
        guard await validate(), let draft else { return }
        do {
            preview = try await proposals.json(draft)
        } catch {
            errorMessage = ErrorText.m3(error, amount: format)
        }
    }

    public func viewPayload() async {
        guard await validate(), let draft else { return }
        do {
            preview = try await proposals.payloadHex(draft)
        } catch {
            errorMessage = ErrorText.m3(error, amount: format)
        }
    }

    public func closePreview() {
        preview = nil
    }

    /// "Create Proposal": validates, then asks the fee question.
    public func requestCreate() async {
        guard step == .editing, await validate() else { return }
        step = .confirming
    }

    /// No on the fee question.
    public func cancelConfirmation() {
        if step == .confirming || step == .needsPassphrase { step = .editing }
    }

    /// Yes: pays the collateral with a `.spend` grant of 1 DASH plus the fee
    /// allowance; an encrypted vault needs `passphrase`.
    public func confirm(passphrase: String? = nil) async {
        guard step == .confirming || step == .needsPassphrase, let draft,
            let wallet = walletState.selectedWalletID
        else { return }
        errorMessage = nil
        let limit = Amount(duffs: parameters.proposalFee.duffs + Self.feeAllowance.duffs)
        let grant: AuthGrant
        do {
            guard let issued = try await grants.authorize(.spend(max: limit), wallet: wallet, passphrase: passphrase)
            else {
                step = .needsPassphrase
                return
            }
            grant = issued
        } catch {
            errorMessage = ErrorText.m3(error, amount: format)
            step = error.code == .vaultWrongPassphrase ? .needsPassphrase : .confirming
            return
        }
        step = .creating
        do {
            step = .created(try await proposals.create(wallet: wallet, draft: draft, grant: grant))
        } catch {
            grants.revoke(grant)
            errorMessage = "\(L10n.Governance.creationFailed): \(ErrorText.m3(error, amount: format))"
            if error.code == .governanceInvalidProposal, let index = error.parameters["field"],
                index >= 0, index < Int64(ProposalField.allCases.count)
            {
                fieldErrors = [ProposalField.allCases[Int(index)]]
            }
            step = .editing
        }
    }
}
