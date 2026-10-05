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
                .foregroundStyle(Color.dash.gray500)

            HStack(spacing: DashSpacing.s) {
                TextField(
                    label,
                    text: $text,
                    prompt: Text(placeholder).foregroundStyle(Color.dash.black1000Alpha30)
                )
                .textFieldStyle(.plain)
                .font(DashTextStyle.callout.font)
                .foregroundStyle(Color.dash.primaryText)
                .multilineTextAlignment(.leading)
                .focused($isFocused)
                .disabled(isDisabled)

                Text(unit)
                    .font(DashTextStyle.calloutMedium.font)
                    .foregroundStyle(Color.dash.secondaryText)
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
                    .stroke(isFocused && !isDisabled ? Color.dash.gray300Alpha40 : .clear, lineWidth: 1)
            )

            if let errorText {
                Text(errorText)
                    .dashFont(.footnote)
                    .foregroundStyle(Color.dash.errorText)
            } else if let secondaryText {
                Text(secondaryText)
                    .dashFont(.footnote)
                    .foregroundStyle(Color.dash.secondaryText)
            }
        }
        .accessibilityElement(children: .contain)
        .accessibilityValue(Text(unit))
    }

    private var backgroundColor: Color {
        if isFocused && !isDisabled { return .clear }
        if errorText != nil { return Color.dash.redAlpha5 }
        return Color.dash.gray300Alpha10
    }
}
#endif
