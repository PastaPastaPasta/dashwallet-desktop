// Address book (QT-095…098): sending and receiving lists with search,
// new/edit/delete, copy and CSV export; also the picker Send uses.
#if os(macOS)
import DashUIMac
import DesignTokens
import PlatformServicesMac
import SwiftUI
import WalletFeatures
import WalletRuntime

/// The Window ▸ Sending/Receiving Addresses window.
struct AddressBookWindow: View {
    let model: MacAppModel
    @State private var book: AddressBookViewModel?

    var body: some View {
        Group {
            if let book {
                AddressBookView(book: book, onChoose: nil)
            } else {
                Text(L10n.Common.noWallet)
                    .foregroundStyle(Color.role.textSecondary)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            }
        }
        .frame(minWidth: 560, minHeight: 360)
        .task(id: model.main?.selectedWalletID) {
            book = model.makeAddressBook(purpose: model.addressBookPurpose)
        }
        .onChange(of: model.addressBookPurpose) { _, purpose in book?.setPurpose(purpose) }
    }
}

struct AddressBookView: View {
    let book: AddressBookViewModel
    /// Selection mode: called with the chosen entry ("Choose" or double click).
    let onChoose: ((AddressBookEntry) -> Void)?
    @State private var selection: AddressBookEntry.ID?
    @State private var editor: EditorState?
    @State private var search = ""
    @State private var exportMessage: String?
    @State private var toast: ToastMessage?

    private var selected: AddressBookEntry? {
        selection.flatMap { id in book.entries.first { $0.id == id } }
    }

    /// UX-SPEC §4.10: the dash-qt text, Sending | Receiving, search, and the
    /// entries as iOS contact rows in a menu card; copy / edit on hover and
    /// dash-qt's actions in the context menu.
    var body: some View {
        VStack(alignment: .leading, spacing: DashSpacing.m) {
            HStack {
                if !book.selectionMode {
                    DashSegmentedControl(
                        [(AddressPurpose.send, MacStrings.AddressBook.sending),
                         (AddressPurpose.receive, MacStrings.AddressBook.receiving)],
                        selection: Binding(get: { book.purpose }, set: { book.setPurpose($0) }))
                    .accessibilityIdentifier("addressBook.purpose")
                }
                Spacer()
                if book.canCreate {
                    Button(MacStrings.AddressBook.new, systemImage: "plus") { editor = EditorState(entry: nil) }
                        .buttonStyle(.dash(.tintedBlue, .small))
                }
                if onChoose == nil {
                    Button(MacStrings.Common.export) { Task { await export() } }
                        .buttonStyle(.dash(.plainBlue, .small))
                }
            }
            Text(book.header)
                .dashFont(.footnote)
                .foregroundStyle(Color.role.textSecondary)
                .fixedSize(horizontal: false, vertical: true)
            HStack(spacing: DashSpacing.xs) {
                Image(systemName: "magnifyingglass")
                    .foregroundStyle(Color.role.textTertiary)
                    .accessibilityHidden(true)
                TextField(L10n.AddressBook.searchPlaceholder, text: $search)
                    .textFieldStyle(.plain)
                    .onChange(of: search) { _, text in book.setSearch(text) }
                    .accessibilityIdentifier("addressBook.search")
            }
            .dashFont(.subhead)
            .padding(.horizontal, DashSpacing.m)
            .frame(height: 32)
            .background(RoundedRectangle(cornerRadius: DashRadius.searchField, style: .continuous).fill(Color.role.fieldFill))
            ScrollView {
                if book.entries.isEmpty {
                    EmptyState(icon: .token(.addressBook), title: MacStrings.AddressBook.empty)
                } else {
                    VStack(spacing: DashSpacing.xxxs) {
                        ForEach(book.entries) { entry in
                            AddressRow(
                                entry: entry, label: book.labelText(for: entry), isSelected: entry.id == selection,
                                onSelect: { selection = entry.id },
                                onActivate: {
                                    if let onChoose { onChoose(entry) } else { editor = EditorState(entry: entry) }
                                },
                                onCopy: { copy(entry.address) },
                                onEdit: book.selectionMode ? nil : { editor = EditorState(entry: entry) })
                            .contextMenu { menu(for: entry) }
                        }
                    }
                    .padding(DashSpacing.menuCardInner)
                }
            }
            .frame(maxHeight: .infinity)
            .dashCard(padding: nil, elevation: .menuCard)
            .accessibilityIdentifier("addressBook.table")
            if let error = book.errorMessage {
                Text(error).dashFont(.footnote).foregroundStyle(Color.role.danger)
            }
            if let exportMessage {
                Text(exportMessage).dashFont(.footnote).foregroundStyle(Color.role.textSecondary)
            }
            if onChoose != nil {
                HStack {
                    Spacer()
                    Button(MacStrings.AddressBook.choose) { if let selected { onChoose?(selected) } }
                        .buttonStyle(.dash(.filledBlue, .medium))
                        .keyboardShortcut(.defaultAction)
                        .disabled(selected == nil)
                }
            }
        }
        .padding(DashSpacing.xl)
        .dashCanvas()
        .dashToast($toast)
        .task { await book.load() }
        .sheet(item: $editor) { state in
            AddressEditor(book: book, state: state, onClose: { editor = nil })
        }
        .sheet(isPresented: Binding(get: { book.qrEntry != nil }, set: { if !$0 { book.hideQR() } })) {
            AddressQRSheet(book: book)
        }
    }

    private func copy(_ text: String) {
        MacPasteboard.copy(text)
        toast = ToastMessage(.copied, L10n.UX.copied)
    }

    /// dash-qt's address-book context menu.
    @ViewBuilder
    private func menu(for entry: AddressBookEntry) -> some View {
        Button(MacStrings.AddressBook.copyAddress) { copy(entry.address) }
        Button(MacStrings.AddressBook.copyLabel) { copy(entry.label) }
            .disabled(entry.label.isEmpty)
        if !book.selectionMode {
            Button(MacStrings.Common.edit) { editor = EditorState(entry: entry) }
        }
        Button(MacStrings.AddressBook.showQR) { book.showQR(for: entry) }
        if book.canDelete {
            Divider()
            Button(MacStrings.Common.delete, role: .destructive) {
                Task { await book.delete(address: entry.address) }
            }
        }
    }

    private func export() async {
        switch await MacSavePanel.save(
            text: book.exportCSV(), suggestedName: MacStrings.AddressBook.exportName, title: book.header)
        {
        case .saved: exportMessage = MacStrings.Common.exportSaved
        case .cancelled: exportMessage = nil
        case .failed(let reason): exportMessage = reason
        }
    }
}

/// One entry: a blue initial avatar, the label and the middle-truncated
/// address; copy and edit appear under the pointer.
private struct AddressRow: View {
    let entry: AddressBookEntry
    let label: String
    let isSelected: Bool
    let onSelect: () -> Void
    let onActivate: () -> Void
    let onCopy: () -> Void
    let onEdit: (() -> Void)?
    @State private var isHovering = false

    var body: some View {
        HStack(spacing: DashSpacing.sm) {
            Text(String(label.first ?? "#").uppercased())
                .font(.system(size: 13, weight: .semibold))
                .foregroundStyle(Color.role.textOnHero)
                .frame(width: DashLayout.rowIconSize, height: DashLayout.rowIconSize)
                .background(Circle().fill(Color.role.accent))
                .accessibilityHidden(true)
            VStack(alignment: .leading, spacing: 1) {
                Text(label)
                    .dashFont(.subheadMedium)
                    .foregroundStyle(Color.role.textPrimary)
                    .lineLimit(1)
                Text(entry.address)
                    .dashFont(.footnote)
                    .foregroundStyle(Color.role.textSecondary)
                    .lineLimit(1)
                    .truncationMode(.middle)
                    .help(entry.address)
            }
            Spacer(minLength: DashSpacing.m)
            if isHovering || isSelected {
                CopyButton(value: entry.address, label: MacStrings.AddressBook.copyAddress, onCopied: onCopy)
                if let onEdit {
                    Button(action: onEdit) { Image(systemName: "pencil") }
                        .buttonStyle(.dash(.tintedGray, .extraSmall))
                        .help(MacStrings.Common.edit)
                        .accessibilityLabel(MacStrings.Common.edit)
                }
            }
        }
        .padding(.horizontal, DashSpacing.sm)
        .frame(minHeight: DashLayout.rowMinHeight)
        .contentShape(Rectangle())
        .dashRowHighlight(isSelected: isSelected, radius: DashRadius.standard + 2)
        .onHover { isHovering = $0 }
        .gesture(TapGesture(count: 2).onEnded(onActivate))
        .simultaneousGesture(TapGesture().onEnded(onSelect))
        .accessibilityElement(children: .contain)
        .accessibilityAddTraits(isSelected ? [.isSelected, .isButton] : .isButton)
    }
}

/// dash-qt's "Show QR code" dialog: the entry's `dash:` URI with its label.
private struct AddressQRSheet: View {
    let book: AddressBookViewModel

    var body: some View {
        VStack(spacing: DashSpacing.m) {
            if let entry = book.qrEntry {
                Text(book.labelText(for: entry)).dashFont(.headline).foregroundStyle(Color.role.textPrimary)
            }
            if let qr = book.qr {
                QRView(size: qr.size, modules: qr.modules, accessibilityLabel: MacStrings.AddressBook.qrTitle)
                    .frame(width: 240, height: 240)
            }
            if let uri = book.qrURI {
                Text(uri)
                    .dashFont(.footnote)
                    .foregroundStyle(Color.role.textSecondary)
                    .textSelection(.enabled)
                    .multilineTextAlignment(.center)
            }
            HStack {
                Button(MacStrings.Common.copy, systemImage: "doc.on.doc") {
                    if let uri = book.qrURI { MacPasteboard.copy(uri) }
                }
                .buttonStyle(.dash(.tintedBlue, .medium))
                Spacer()
                Button(MacStrings.Common.close) { book.hideQR() }
                    .buttonStyle(.dash(.tintedGray, .medium))
                    .keyboardShortcut(.cancelAction)
            }
        }
        .padding(DashSpacing.xl)
        .frame(width: 360)
        .dashCanvas()
        .accessibilityIdentifier("addressBook.qr")
    }
}

struct EditorState: Identifiable {
    let id = UUID()
    /// `nil` creates a new sending entry.
    let entry: AddressBookEntry?
}

/// dash-qt `EditAddressDialog`.
private struct AddressEditor: View {
    let book: AddressBookViewModel
    let state: EditorState
    let onClose: () -> Void
    @State private var label = ""
    @State private var address = ""

    private var title: String {
        guard let entry = state.entry else { return L10n.AddressBook.newSendingAddress }
        return entry.purpose == .send ? L10n.AddressBook.editSendingAddress : L10n.AddressBook.editReceivingAddress
    }

    var body: some View {
        VStack(alignment: .leading, spacing: DashSpacing.m) {
            Text(title).dashFont(.title3).foregroundStyle(Color.role.textPrimary)
            FieldCaption(MacStrings.AddressBook.label)
            TextField(MacStrings.AddressBook.label, text: $label)
                .textFieldStyle(.dash)
            FieldCaption(MacStrings.AddressBook.address)
            TextField(MacStrings.AddressBook.address, text: $address)
                .textFieldStyle(.dash)
                .disabled(state.entry != nil)
            if let error = book.errorMessage {
                Text(error).dashFont(.footnote).foregroundStyle(Color.role.danger)
            }
            HStack {
                Spacer()
                Button(MacStrings.Common.cancel, action: onClose)
                    .buttonStyle(.dash(.tintedGray, .medium))
                    .keyboardShortcut(.cancelAction)
                Button(MacStrings.Common.save) {
                    Task {
                        if await book.save(address: address, label: label, replace: state.entry != nil) { onClose() }
                    }
                }
                .buttonStyle(.dash(.filledBlue, .medium))
                .keyboardShortcut(.defaultAction)
                .disabled(address.isEmpty)
            }
        }
        .padding(DashSpacing.xl)
        .frame(width: DashLayout.sheetWidthSmall)
        .dashCanvas()
        .onAppear {
            label = state.entry?.label ?? ""
            address = state.entry?.address ?? ""
        }
    }
}
#endif
