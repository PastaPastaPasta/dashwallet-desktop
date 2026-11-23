// Desktop amount entry: a labelled text field with the unit, an optional "Max" button and a
// secondary line (fiat equivalent). Parsing and validation belong to the view model.
#if os(macOS)
import DesignTokens
import SwiftUI

/// A text field for an amount. The text is passed through unchanged; the host parses it and
/// supplies `errorText` and `secondaryText`.
public struct AmountField: View {
    public let label: String
    @Binding public var text: String
    public let unit: String
    public let placeholder: String
    /// Line under the field, e.g. the fiat equivalent.
    public let secondaryText: String?
    public let errorText: String?
    public let isDisabled: Bool
    /// Shows a "Max" button when set.
    public let onMax: (() -> Void)?

    @FocusState private var isFocused: Bool

    public init(
        label: String,
        text: Binding<String>,
        unit: String,
        placeholder: String = "0",
        secondaryText: String? = nil,
        errorText: String? = nil,
        isDisabled: Bool = false,
        onMax: (() -> Void)? = nil
    ) {
        self.label = label
        self._text = text
        self.unit = unit
        self.placeholder = placeholder
        self.secondaryText = secondaryText
        self.errorText = errorText
        self.isDisabled = isDisabled
        self.onMax = onMax
    }

    public var body: some View {
        VStack(alignment: .leading, spacing: DashSpacing.sm) {
            Text(label)
                .dashFont(.footnote)
                .foregroundStyle(Color.role.textSecondary)

            HStack(spacing: DashSpacing.s) {
                TextField(
                    label,
                    text: $text,
                    prompt: Text(placeholder).foregroundStyle(Color.role.textTertiary)
                )
                .textFieldStyle(.plain)
                .font(DashTextStyle.callout.font)
                .monospacedDigit()
                .foregroundStyle(Color.role.textPrimary)
                .multilineTextAlignment(.leading)
                .focused($isFocused)
                .disabled(isDisabled)

                Text(unit)
                    .font(DashTextStyle.calloutMedium.font)
                    .foregroundStyle(Color.role.textSecondary)
                    .accessibilityHidden(true)

                if let onMax, !isDisabled {
                    DashButton(
                        text: NSLocalizedString("Max", bundle: .module, comment: "AmountField"),
                        size: .small,
                        style: .tintedBlue,
                        action: onMax
                    )
                }
            }
            .padding(.horizontal, DashSpacing.l)
            .frame(minHeight: 48)
            .background(
                RoundedRectangle(cornerRadius: DashRadius.textField, style: .continuous)
                    .fill(backgroundColor)
            )
            .overlay(
                RoundedRectangle(cornerRadius: DashRadius.textField, style: .continuous)
                    .strokeBorder(isFocused && !isDisabled ? Color.role.accent : .clear, lineWidth: 1)
            )

            if let errorText {
                Text(errorText)
                    .dashFont(.footnote)
                    .foregroundStyle(Color.role.danger)
            } else if let secondaryText {
                Text(secondaryText)
                    .dashFont(.footnote)
                    .foregroundStyle(Color.role.textSecondary)
            }
        }
        .accessibilityElement(children: .contain)
        .accessibilityValue(Text(unit))
    }

    private var backgroundColor: Color {
        if errorText != nil { return Color.role.dangerTint }
        return Color.role.fieldFill
    }
}
#endif
