// Labelled text inputs (DashUIKit `AddressFieldView`-like caption + field).
//
// SwiftCrossUI cannot tie the caption to the field for assistive technology
// (ADR 0002, gap A3), so each field takes the caption as its accessible name
// through `accessibleName` (Accessibility.swift), and the placeholder repeats
// the caption's meaning (GTK exposes it as `placeholder-text`).
import DesignTokens
import SwiftCrossUI

/// Caption, text field and an optional error line.
public struct DashTextField: View {
    let caption: String
    let placeholder: String
    let text: Binding<String>
    let error: String?
    let width: Int?

    public init(_ caption: String, placeholder: String? = nil, text: Binding<String>, error: String? = nil, width: Int? = nil) {
        self.caption = caption
        self.placeholder = placeholder ?? caption
        self.text = text
        self.error = error
        self.width = width
    }

    public var body: some View {
        VStack(alignment: .leading, spacing: points(DashSpacing.xxs)) {
            FieldCaption(caption)
            if let width {
                TextField(placeholder, text: text).accessibleName(accessibleCaption(caption)).frame(width: Double(width))
            } else {
                TextField(placeholder, text: text).accessibleName(accessibleCaption(caption))
                    .frame(minWidth: 200, maxWidth: .infinity)
            }
            FieldError(error)
        }
    }
}

/// Caption and a secure field; the bound text is the caller's to clear.
public struct DashSecureField: View {
    let caption: String
    let placeholder: String
    let text: Binding<String>
    let error: String?

    public init(_ caption: String, placeholder: String? = nil, text: Binding<String>, error: String? = nil) {
        self.caption = caption
        self.placeholder = placeholder ?? caption
        self.text = text
        self.error = error
    }

    public var body: some View {
        VStack(alignment: .leading, spacing: points(DashSpacing.xxs)) {
            FieldCaption(caption)
            SecureField(placeholder, text: text).accessibleName(accessibleCaption(caption))
                .frame(minWidth: 200, maxWidth: 420)
            FieldError(error)
        }
    }
}

/// A switch with its label as the switch's accessible name (ADR 0002, gap A2:
/// SwiftCrossUI's `Toggle` shows the label as a separate text).
public struct DashToggle: View {
    let title: String
    let isOn: Binding<Bool>

    public init(_ title: String, isOn: Binding<Bool>) {
        self.title = title
        self.isOn = isOn
    }

    public var body: some View {
        Toggle(title, isOn: isOn).toggleStyle(.switch).accessibleName(title)
    }
}

/// A caption without the trailing colon dash-qt puts on form labels.
func accessibleCaption(_ caption: String) -> String {
    caption.hasSuffix(":") ? String(caption.dropLast()) : caption
}

struct FieldCaption: View {
    let text: String

    init(_ text: String) { self.text = text }

    var body: some View {
        Text(text)
            .dashFont(.footnoteMedium)
            .dashForeground(.secondaryText)
    }
}

struct FieldError: View {
    let text: String?

    init(_ text: String?) { self.text = text }

    var body: some View {
        if let text {
            Text(text)
                .dashFont(.caption1)
                .dashForeground(.errorText)
        }
    }
}

/// A choice from labelled options for `Picker` (SwiftCrossUI shows each
/// option's `description`).
public struct PickerOption<Value: Hashable & Sendable>: Hashable, Sendable, CustomStringConvertible {
    public let value: Value
    public let title: String

    public init(_ value: Value, _ title: String) {
        self.value = value
        self.title = title
    }

    public var description: String { title }
}

/// Caption plus a picker over `PickerOption`s, bound to the plain value.
public struct DashPicker<Value: Hashable & Sendable>: View {
    let caption: String?
    let options: [PickerOption<Value>]
    let selection: Binding<Value>

    public init(_ caption: String?, options: [PickerOption<Value>], selection: Binding<Value>) {
        self.caption = caption
        self.options = options
        self.selection = selection
    }

    public var body: some View {
        let options = options
        let selection = selection
        let bridged = Binding<PickerOption<Value>?>(
            get: { options.first { $0.value == selection.wrappedValue } },
            set: { if let option = $0 { selection.wrappedValue = option.value } })
        VStack(alignment: .leading, spacing: points(DashSpacing.xxs)) {
            if let caption {
                FieldCaption(caption)
                Picker(of: options, selection: bridged).accessibleName(accessibleCaption(caption))
            } else {
                Picker(of: options, selection: bridged)
            }
        }
    }
}
