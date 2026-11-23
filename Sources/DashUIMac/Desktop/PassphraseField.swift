// Passphrase entry with a show/hide button and a strength meter. The strength is computed by the
// view model; this file only draws it.
#if os(macOS)
import DesignTokens
import SwiftUI

/// Strength of a passphrase as judged by the caller's estimator.
public enum PassphraseStrength: Int, Sendable, Hashable, CaseIterable, Comparable {
    case veryWeak, weak, fair, good, strong

    public static func < (lhs: Self, rhs: Self) -> Bool { lhs.rawValue < rhs.rawValue }

    /// Number of filled meter segments (1...4).
    var filledSegments: Int { max(rawValue, 1) }

    var color: Color {
        switch self {
        case .veryWeak, .weak: .role.danger
        case .fair: .role.warning
        case .good: .role.caution
        case .strong: .role.success
        }
    }

    /// Default English label; callers may pass their own text.
    public var defaultLabel: String {
        switch self {
        case .veryWeak: NSLocalizedString("Very weak", bundle: .module, comment: "Passphrase strength")
        case .weak: NSLocalizedString("Weak", bundle: .module, comment: "Passphrase strength")
        case .fair: NSLocalizedString("Fair", bundle: .module, comment: "Passphrase strength")
        case .good: NSLocalizedString("Good", bundle: .module, comment: "Passphrase strength")
        case .strong: NSLocalizedString("Strong", bundle: .module, comment: "Passphrase strength")
        }
    }
}

/// Four segments filled according to a strength, with its label.
public struct PassphraseStrengthMeter: View {
    public static let segmentCount = 4

    public let strength: PassphraseStrength
    public let text: String?

    public init(strength: PassphraseStrength, text: String? = nil) {
        self.strength = strength
        self.text = text
    }

    public var body: some View {
        HStack(spacing: DashSpacing.s) {
            HStack(spacing: DashSpacing.xxs) {
                ForEach(0..<Self.segmentCount, id: \.self) { index in
                    Capsule()
                        .fill(index < strength.filledSegments ? strength.color : Color.role.neutralTint)
                        .frame(height: 4)
                }
            }
            Text(text ?? strength.defaultLabel)
                .font(DashTextStyle.caption1Medium.font)
                .foregroundStyle(Color.role.textSecondary)
                .fixedSize()
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(Text(NSLocalizedString("Passphrase strength", bundle: .module, comment: "PassphraseField")))
        .accessibilityValue(Text(text ?? strength.defaultLabel))
    }
}

/// A secure text field with a reveal button, optional strength meter and error line.
///
/// SwiftUI text fields only bind `String`; the view model converts the text to bytes for the
/// vault and clears the binding when it is done.
public struct PassphraseField: View {
    public let label: String
    @Binding public var text: String
    public let placeholder: String
    /// nil hides the meter (e.g. while the field is empty, or for a confirmation field).
    public let strength: PassphraseStrength?
    public let strengthText: String?
    public let errorText: String?
    public let isRevealable: Bool

    @State private var isRevealed = false
    @FocusState private var isFocused: Bool

    public init(
        label: String,
        text: Binding<String>,
        placeholder: String = "",
        strength: PassphraseStrength? = nil,
        strengthText: String? = nil,
        errorText: String? = nil,
        isRevealable: Bool = true
    ) {
        self.label = label
        self._text = text
        self.placeholder = placeholder
        self.strength = strength
        self.strengthText = strengthText
        self.errorText = errorText
        self.isRevealable = isRevealable
    }

    public var body: some View {
        VStack(alignment: .leading, spacing: DashSpacing.sm) {
            Text(label)
                .dashFont(.footnote)
                .foregroundStyle(Color.role.textSecondary)

            HStack(spacing: DashSpacing.s) {
                Group {
                    if isRevealed {
                        TextField(label, text: $text, prompt: prompt)
                    } else {
                        SecureField(label, text: $text, prompt: prompt)
                    }
                }
                .textFieldStyle(.plain)
                .font(DashTextStyle.callout.font)
                .foregroundStyle(Color.role.textPrimary)
                .autocorrectionDisabled(true)
                .focused($isFocused)

                if isRevealable {
                    Button(action: { isRevealed.toggle() }) {
                        DashIconImage(.token(isRevealed ? .eyeClosed : .eyeOpen))
                            .scaledToFit()
                            .frame(width: 18, height: 18)
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel(Text(isRevealed
                        ? NSLocalizedString("Hide passphrase", bundle: .module, comment: "PassphraseField")
                        : NSLocalizedString("Show passphrase", bundle: .module, comment: "PassphraseField")))
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
                    .strokeBorder(isFocused ? Color.role.accent : .clear, lineWidth: 1)
            )

            if let strength {
                PassphraseStrengthMeter(strength: strength, text: strengthText)
            }

            if let errorText {
                Text(errorText)
                    .dashFont(.footnote)
                    .foregroundStyle(Color.role.danger)
            }
        }
    }

    private var prompt: Text {
        Text(placeholder).foregroundStyle(Color.role.textTertiary)
    }

    private var backgroundColor: Color {
        if errorText != nil { return Color.role.dangerTint }
        return Color.role.fieldFill
    }
}
#endif
