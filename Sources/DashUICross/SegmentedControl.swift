// The iOS segmented control (C19): a capsule group with the selected
// segment as a raised pill. Each segment is a button named by its title, so
// AT-SPI and keyboard users reach it like any other button (SwiftCrossUI's
// GTK segmented picker draws the toolkit's look and has no per-segment name).
import DesignTokens
import SwiftCrossUI

public struct SegmentedControl<Value: Hashable & Sendable>: View {
    let options: [PickerOption<Value>]
    let selection: Value
    let disabled: Set<Value>
    let help: [Value: String]
    let onSelect: @MainActor @Sendable (Value) -> Void

    public init(
        options: [PickerOption<Value>], selection: Value, disabled: Set<Value> = [], help: [Value: String] = [:],
        onSelect: @escaping @MainActor @Sendable (Value) -> Void
    ) {
        self.options = options
        self.selection = selection
        self.disabled = disabled
        self.help = help
        self.onSelect = onSelect
    }

    public var body: some View {
        let onSelect = onSelect
        HStack(spacing: points(DashSpacing.xxxs)) {
            ForEach(options, id: \.self) { option in
                Segment(
                    title: option.title, selected: option.value == selection, enabled: !disabled.contains(option.value),
                    help: help[option.value]
                ) {
                    onSelect(option.value)
                }
            }
        }
        .padding(points(DashSpacing.xxxs))
        .background(Capsule().fill(DashColor.segmentControlBackgroundGroup.color))
        .fixedSize()
    }
}

struct Segment: View {
    let title: String
    let selected: Bool
    let enabled: Bool
    let help: String?
    let action: @MainActor @Sendable () -> Void

    @State var hovering = false

    var body: some View {
        let content: DashColor =
            !enabled
            ? CrossRole.textTertiary
            : (selected ? DashColor.segmentControlContSelected : DashColor.segmentControlContNotSelected)
        let button = Button(title, action: action)
            .buttonStyle(.plain)
            .font(.system(size: DashTextStyle.footnote.size, weight: selected ? .semibold : .medium))
            .foregroundColor(content.color)
            .disabled(!enabled)
            .fixedSize()
            .padding(.horizontal, points(DashSpacing.m))
            .padding(.vertical, points(DashSpacing.xs))
            .background {
                if selected {
                    Capsule().fill(DashColor.segmentControlBackground.color)
                } else if hovering && enabled {
                    Capsule().fill(CrossRole.accentTint.color)
                }
            }
            .onHover { hovering = $0 }
        if let help {
            button.help(help)
        } else {
            button
        }
    }
}
