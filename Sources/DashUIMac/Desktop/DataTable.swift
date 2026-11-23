// A sortable, selectable table for transaction, address and coin lists. Columns are values, so
// the same column set can be built at runtime (column visibility settings).
#if os(macOS)
import AppKit
import DesignTokens
import SwiftUI

/// Which column sorts the rows, and in which direction.
public struct DataTableSortOrder: Hashable, Sendable {
    public var columnID: String
    public var ascending: Bool

    public init(columnID: String, ascending: Bool = true) {
        self.columnID = columnID
        self.ascending = ascending
    }
}

/// Horizontal alignment of a column's header and cells.
public enum DataTableAlignment: Sendable, Hashable {
    case leading, center, trailing

    var frameAlignment: Alignment {
        switch self {
        case .leading: .leading
        case .center: .center
        case .trailing: .trailing
        }
    }
}

/// Width of a column: fixed points, or a share of the remaining width with a minimum.
public enum DataTableColumnWidth: Sendable, Hashable {
    case fixed(Double)
    case flexible(min: Double)
}

/// One column: a title, a cell builder and an optional ordering.
public struct DataTableColumn<Row>: Identifiable {
    public let id: String
    public let title: String
    public let width: DataTableColumnWidth
    public let alignment: DataTableAlignment
    let cell: (Row) -> AnyView
    /// Strict "comes before" ordering for ascending sort; nil = the column cannot sort.
    let areInIncreasingOrder: ((Row, Row) -> Bool)?

    /// A column with a custom cell view.
    public init<Cell: View>(
        _ title: String,
        id: String? = nil,
        width: DataTableColumnWidth = .flexible(min: 80),
        alignment: DataTableAlignment = .leading,
        sortBy areInIncreasingOrder: ((Row, Row) -> Bool)? = nil,
        @ViewBuilder cell: @escaping (Row) -> Cell
    ) {
        self.id = id ?? title
        self.title = title
        self.width = width
        self.alignment = alignment
        self.areInIncreasingOrder = areInIncreasingOrder
        self.cell = { AnyView(cell($0)) }
    }

    /// A text column; sorts by `localizedStandardCompare` of the text when `sortable`.
    public init(
        _ title: String,
        id: String? = nil,
        width: DataTableColumnWidth = .flexible(min: 80),
        alignment: DataTableAlignment = .leading,
        sortable: Bool = true,
        value: @escaping (Row) -> String
    ) {
        self.init(
            title,
            id: id,
            width: width,
            alignment: alignment,
            sortBy: sortable ? { value($0).localizedStandardCompare(value($1)) == .orderedAscending } : nil,
            cell: { row in
                Text(value(row))
                    .font(DashTextStyle.footnote.font)
                    .monospacedDigit()
                    .foregroundStyle(Color.role.textPrimary)
                    .lineLimit(1)
                    .truncationMode(.middle)
            }
        )
    }

    public var isSortable: Bool { areInIncreasingOrder != nil }
}

/// An entry of a row's context menu.
public struct DataTableMenuAction: Identifiable {
    public let id: String
    public let title: String
    public let isDestructive: Bool
    public let isEnabled: Bool
    public let action: () -> Void

    public init(id: String? = nil, title: String, isDestructive: Bool = false, isEnabled: Bool = true,
                action: @escaping () -> Void) {
        self.id = id ?? title
        self.title = title
        self.isDestructive = isDestructive
        self.isEnabled = isEnabled
        self.action = action
    }
}

/// Rows under a header of sortable columns.
///
/// - Click selects a row, Command-click toggles it, Shift-click extends from the last click.
/// - Up/Down move the selection while the table has keyboard focus; Return or a double click
///   calls `onActivate`.
/// - Right-click shows `contextMenu` for the selection, or for the clicked row when it is not
///   selected.
/// - Clicking a sortable header sorts by it; clicking it again reverses the direction. Rows
///   are sorted for display only; `rows` is not modified.
/// - With `isScrollable` false the table takes the height of all its rows (for a table inside a
///   view that already scrolls, or for rendering to an image).
public struct DataTable<Row: Identifiable>: View {
    public let rows: [Row]
    public let columns: [DataTableColumn<Row>]
    @Binding public var selection: Set<Row.ID>
    @Binding public var sortOrder: DataTableSortOrder?
    public let emptyText: String?
    public let onActivate: ((Row.ID) -> Void)?
    public let contextMenu: ((Set<Row.ID>) -> [DataTableMenuAction])?
    public let isScrollable: Bool

    @State private var anchor: Row.ID?
    @FocusState private var isFocused: Bool

    public init(
        rows: [Row],
        columns: [DataTableColumn<Row>],
        selection: Binding<Set<Row.ID>>,
        sortOrder: Binding<DataTableSortOrder?>,
        emptyText: String? = nil,
        onActivate: ((Row.ID) -> Void)? = nil,
        contextMenu: ((Set<Row.ID>) -> [DataTableMenuAction])? = nil,
        isScrollable: Bool = true
    ) {
        self.rows = rows
        self.columns = columns
        self._selection = selection
        self._sortOrder = sortOrder
        self.emptyText = emptyText
        self.onActivate = onActivate
        self.contextMenu = contextMenu
        self.isScrollable = isScrollable
    }

    /// `rows` in display order.
    public var displayedRows: [Row] {
        guard let sortOrder,
              let column = columns.first(where: { $0.id == sortOrder.columnID }),
              let less = column.areInIncreasingOrder else { return rows }
        return sortOrder.ascending ? rows.sorted(by: less) : rows.sorted { less($1, $0) }
    }

    public var body: some View {
        let displayed = displayedRows
        Group {
            if isScrollable {
                ScrollView(.vertical) {
                    LazyVStack(spacing: 0, pinnedViews: [.sectionHeaders]) {
                        Section(header: header) { rowsContent(displayed) }
                    }
                }
            } else {
                VStack(spacing: 0) {
                    header
                    rowsContent(displayed)
                }
            }
        }
        .background(Color.role.card)
        .focusable()
        .focused($isFocused)
        .focusEffectDisabled()
        .onMoveCommand { direction in move(direction, in: displayed) }
        .onKeyPress(.return) {
            guard let onActivate, selection.count == 1, let id = selection.first else { return .ignored }
            onActivate(id)
            return .handled
        }
    }

    @ViewBuilder
    private func rowsContent(_ displayed: [Row]) -> some View {
        if displayed.isEmpty, let emptyText {
            Text(emptyText)
                .dashFont(.footnote)
                .foregroundStyle(Color.role.textSecondary)
                .frame(maxWidth: .infinity)
                .padding(DashSpacing.xxl)
        }
        ForEach(Array(displayed.enumerated()), id: \.element.id) { index, row in
            rowView(row, index: index, displayed: displayed)
        }
    }

    // MARK: Header

    private var header: some View {
        HStack(spacing: 0) {
            ForEach(columns) { column in
                headerCell(column)
                    .modifier(ColumnFrame(width: column.width, alignment: column.alignment))
            }
        }
        .padding(.horizontal, DashSpacing.m)
        .frame(height: 28)
        .background(Color.role.cardRaised)
        .overlay(alignment: .bottom) {
            Rectangle().fill(Color.role.separator).frame(height: 0.5)
        }
    }

    @ViewBuilder
    private func headerCell(_ column: DataTableColumn<Row>) -> some View {
        let active = sortOrder?.columnID == column.id ? sortOrder : nil
        let label = HStack(spacing: DashSpacing.xxs) {
            Text(column.title)
                .font(DashTextStyle.footnoteMedium.font)
                .foregroundStyle(active == nil ? Color.role.textSecondary : Color.role.textPrimary)
                .lineLimit(1)
            if let active {
                Image(systemName: active.ascending ? "chevron.up" : "chevron.down")
                    .font(.system(size: 9, weight: .semibold))
                    .foregroundStyle(Color.role.textSecondary)
            }
        }
        if column.isSortable {
            Button(action: { toggleSort(column) }) { label }
                .buttonStyle(.plain)
                .accessibilityLabel(Text(column.title))
                .accessibilityHint(Text(NSLocalizedString("Sorts the table by this column", bundle: .module,
                                                          comment: "DataTable")))
        } else {
            label
        }
    }

    private func toggleSort(_ column: DataTableColumn<Row>) {
        if let current = sortOrder, current.columnID == column.id {
            sortOrder = DataTableSortOrder(columnID: column.id, ascending: !current.ascending)
        } else {
            sortOrder = DataTableSortOrder(columnID: column.id, ascending: true)
        }
    }

    // MARK: Rows

    private func rowView(_ row: Row, index: Int, displayed: [Row]) -> some View {
        let isSelected = selection.contains(row.id)
        return HStack(spacing: 0) {
            ForEach(columns) { column in
                column.cell(row)
                    .modifier(ColumnFrame(width: column.width, alignment: column.alignment))
            }
        }
        .padding(.horizontal, DashSpacing.m)
        .frame(minHeight: 30)
        .background(rowBackground(isSelected: isSelected, index: index))
        .overlay(alignment: .bottom) {
            Rectangle().fill(Color.role.separator.opacity(0.5)).frame(height: 0.5)
        }
        .contentShape(Rectangle())
        .gesture(TapGesture(count: 2).onEnded {
            selection = [row.id]
            anchor = row.id
            onActivate?(row.id)
        })
        .simultaneousGesture(TapGesture().onEnded {
            click(row.id, in: displayed)
            isFocused = true
        })
        .contextMenu {
            if let contextMenu {
                ForEach(contextMenu(menuTarget(for: row.id))) { item in
                    Button(role: item.isDestructive ? .destructive : nil, action: item.action) {
                        Text(item.title)
                    }
                    .disabled(!item.isEnabled)
                }
            }
        }
        .accessibilityElement(children: .combine)
        .accessibilityAddTraits(isSelected ? [.isSelected, .isButton] : .isButton)
    }

    private func rowBackground(isSelected: Bool, index: Int) -> Color {
        // No zebra stripes (UX-SPEC C28); selection is the strong accent tint.
        if isSelected { return Color.role.accentTintStrong }
        return .clear
    }

    /// Rows a context-menu action applies to.
    private func menuTarget(for id: Row.ID) -> Set<Row.ID> {
        selection.contains(id) ? selection : [id]
    }

    private func click(_ id: Row.ID, in displayed: [Row]) {
        let flags = NSEvent.modifierFlags
        if flags.contains(.command) {
            if selection.contains(id) { selection.remove(id) } else { selection.insert(id) }
            anchor = id
        } else if flags.contains(.shift), let anchor,
                  let from = displayed.firstIndex(where: { $0.id == anchor }),
                  let to = displayed.firstIndex(where: { $0.id == id }) {
            selection = Set(displayed[min(from, to)...max(from, to)].map(\.id))
        } else {
            selection = [id]
            anchor = id
        }
    }

    private func move(_ direction: MoveCommandDirection, in displayed: [Row]) {
        guard !displayed.isEmpty else { return }
        let current = anchor.flatMap { id in displayed.firstIndex(where: { $0.id == id }) }
        let next: Int
        switch direction {
        case .up: next = max((current ?? displayed.count) - 1, 0)
        case .down: next = min((current ?? -1) + 1, displayed.count - 1)
        default: return
        }
        let id = displayed[next].id
        selection = [id]
        anchor = id
    }
}

/// Applies a column's width and alignment to a header or body cell.
private struct ColumnFrame: ViewModifier {
    let width: DataTableColumnWidth
    let alignment: DataTableAlignment

    func body(content: Content) -> some View {
        switch width {
        case .fixed(let points):
            content
                .frame(width: points, alignment: alignment.frameAlignment)
                .padding(.trailing, DashSpacing.s)
        case .flexible(let minimum):
            content
                .frame(minWidth: minimum, maxWidth: .infinity, alignment: alignment.frameAlignment)
                .padding(.trailing, DashSpacing.s)
        }
    }
}
#endif
