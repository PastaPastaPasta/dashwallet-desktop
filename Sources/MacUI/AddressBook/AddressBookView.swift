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
                    .foregroundStyle(Color.dash.secondaryText)
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
    @State private var selection: Set<AddressBookEntry.ID> = []
    @State private var sortOrder: DataTableSortOrder? = DataTableSortOrder(columnID: "label")
    @State private var editor: EditorState?
    @State private var search = ""
    @State private var exportMessage: String?

    private var selected: AddressBookEntry? {
        selection.count == 1 ? book.entries.first { selection.contains($0.id) } : nil
    }

    var body: some View {
        VStack(alignment: .leading, spacing: DashSpacing.m) {
            if !book.selectionMode {
                Picker("", selection: Binding(get: { book.purpose }, set: { book.setPurpose($0) })) {
                    Text(MacStrings.AddressBook.sending).tag(AddressPurpose.send)
                    Text(MacStrings.AddressBook.receiving).tag(AddressPurpose.receive)
                }
                .pickerStyle(.segmented)
                .labelsHidden()
                .frame(width: 260)
                .accessibilityIdentifier("addressBook.purpose")
            }
            Text(book.header)
                .dashFont(.footnote)
                .foregroundStyle(Color.dash.secondaryText)
                .fixedSize(horizontal: false, vertical: true)
            TextField(L10n.AddressBook.searchPlaceholder, text: $search)
                .textFieldStyle(.roundedBorder)
                .onChange(of: search) { _, text in book.setSearch(text) }
                .accessibilityIdentifier("addressBook.search")
            DataTable(
                rows: book.entries,
                columns: [
                    DataTableColumn(MacStrings.AddressBook.label, id: "label", width: .flexible(min: 140)) {
                        book.labelText(for: $0)
                    },
                    DataTableColumn(MacStrings.AddressBook.address, id: "address", width: .flexible(min: 260)) { entry in
                        Text(entry.address)
                            .font(.system(.footnote, design: .monospaced))
                            .lineLimit(1)
                            .truncationMode(.middle)
                    },
                ],
                selection: $selection, sortOrder: $sortOrder, emptyText: MacStrings.AddressBook.empty,
                onActivate: { id in
                    guard let entry = book.entries.first(where: { $0.id == id }) else { return }
                    if let onChoose { onChoose(entry) } else { editor = EditorState(entry: entry) }
                })
            .clipShape(RoundedRectangle(cornerRadius: DashRadius.standard))
            .accessibilityIdentifier("addressBook.table")
            if let error = book.errorMessage {
                Text(error).dashFont(.footnote).foregroundStyle(Color.dash.errorText)
            }
            buttons
        }
        .padding(DashSpacing.xl)
        .task { await book.load() }
        .sheet(item: $editor) { state in
            AddressEditor(book: book, state: state, onClose: { editor = nil })
        }
    }

    private var buttons: some View {
        HStack(spacing: DashSpacing.s) {
            if book.canCreate {
                Button(MacStrings.AddressBook.new, systemImage: "plus") { editor = EditorState(entry: nil) }
            }
            Button(MacStrings.Common.copy, systemImage: "doc.on.doc") {
                if let selected { MacPasteboard.copy(selected.address) }
            }
            .disabled(selected == nil)
            Button(MacStrings.Common.edit) { if let selected { editor = EditorState(entry: selected) } }
                .disabled(selected == nil)
            if book.canDelete {
                Button(MacStrings.Common.delete, role: .destructive) {
                    if let selected { Task { await book.delete(address: selected.address) } }
                }
                .disabled(selected == nil)
            }
            Spacer()
            if let exportMessage { Text(exportMessage).dashFont(.footnote).foregroundStyle(Color.dash.secondaryText) }
            if onChoose == nil {
                Button(MacStrings.Common.export) { Task { await export() } }
            } else {
                Button(MacStrings.AddressBook.choose) { if let selected { onChoose?(selected) } }
                    .keyboardShortcut(.defaultAction)
                    .disabled(selected == nil)
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
            Text(title).dashFont(.title3)
            TextField(MacStrings.AddressBook.label, text: $label)
                .textFieldStyle(.roundedBorder)
            TextField(MacStrings.AddressBook.address, text: $address)
                .textFieldStyle(.roundedBorder)
                .font(.system(.body, design: .monospaced))
                .disabled(state.entry != nil)
            if let error = book.errorMessage {
                Text(error).dashFont(.footnote).foregroundStyle(Color.dash.errorText)
            }
            HStack {
                Spacer()
                Button(MacStrings.Common.cancel, action: onClose).keyboardShortcut(.cancelAction)
                Button(MacStrings.Common.save) {
                    Task {
                        if await book.save(address: address, label: label, replace: state.entry != nil) { onClose() }
                    }
                }
                .keyboardShortcut(.defaultAction)
                .disabled(address.isEmpty)
            }
        }
        .padding(DashSpacing.xl)
        .frame(width: 480)
        .onAppear {
            label = state.entry?.label ?? ""
            address = state.entry?.address ?? ""
        }
    }
}
#endif
