// Page-level components (UX-SPEC C13, C19, C21–C30): the page scaffold (canvas, centred column,
// title), segmented control, sheet scaffold with footer bar, empty/loading states, progress bar,
// toast host, wizard header and the recovery-phrase grid.
#if os(macOS)
import DesignTokens
import SwiftUI

// MARK: Page

/// A scrollable page on `canvas` with a centred column of at most `maxWidth`, a `title2` title, an
/// optional description and trailing actions on the title row.
public struct DashPage<Actions: View, Content: View>: View {
    public let title: String?
    public let subtitle: String?
    public let maxWidth: Double
    public let scrolls: Bool
    @ViewBuilder public let actions: () -> Actions
    @ViewBuilder public let content: () -> Content

    public init(
        title: String?, subtitle: String? = nil, maxWidth: Double = DashLayout.contentMaxWidth, scrolls: Bool = true,
        @ViewBuilder actions: @escaping () -> Actions, @ViewBuilder content: @escaping () -> Content
    ) {
        self.title = title
        self.subtitle = subtitle
        self.maxWidth = maxWidth
        self.scrolls = scrolls
        self.actions = actions
        self.content = content
    }

    public var body: some View {
        Group {
            if scrolls {
                ScrollView { column }
            } else {
                column.frame(maxHeight: .infinity, alignment: .top)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .dashCanvas()
    }

    private var column: some View {
        VStack(alignment: .leading, spacing: DashLayout.sectionGap) {
            if let title {
                HStack(alignment: .top, spacing: DashSpacing.m) {
                    PageTitle(title: title, subtitle: subtitle)
                    Spacer(minLength: DashSpacing.m)
                    actions()
                }
            }
            content()
        }
        .frame(maxWidth: maxWidth, alignment: .leading)
        .padding(.horizontal, DashLayout.pagePaddingH)
        .padding(.top, DashLayout.pagePaddingTop)
        .padding(.bottom, DashLayout.sectionGap)
        .frame(maxWidth: .infinity)
    }
}

public extension DashPage where Actions == EmptyView {
    init(
        title: String?, subtitle: String? = nil, maxWidth: Double = DashLayout.contentMaxWidth, scrolls: Bool = true,
        @ViewBuilder content: @escaping () -> Content
    ) {
        self.init(title: title, subtitle: subtitle, maxWidth: maxWidth, scrolls: scrolls, actions: { EmptyView() },
                  content: content)
    }
}

/// DashUIKit `TopIntroView`'s title block: `title2` and a `subhead` secondary description.
public struct PageTitle: View {
    public let title: String
    public let subtitle: String?

    public init(title: String, subtitle: String? = nil) {
        self.title = title
        self.subtitle = subtitle
    }

    public var body: some View {
        VStack(alignment: .leading, spacing: DashSpacing.xxs) {
            Text(title)
                .dashFont(.title2)
                .foregroundStyle(Color.role.textPrimary)
                .accessibilityAddTraits(.isHeader)
            if let subtitle {
                Text(subtitle)
                    .dashFont(.subhead)
                    .foregroundStyle(Color.role.textSecondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }
}

// MARK: Segmented control (C19)

/// The iOS segmented control: a capsule group with a white pill under the selected segment.
public struct DashSegmentedControl<Value: Hashable>: View {
    public let options: [(value: Value, title: String)]
    @Binding public var selection: Value
    /// Accessibility identifier of each segment (UI tests).
    public let segmentIdentifier: ((Value) -> String)?

    public init(
        _ options: [(value: Value, title: String)], selection: Binding<Value>,
        segmentIdentifier: ((Value) -> String)? = nil
    ) {
        self.options = options
        self._selection = selection
        self.segmentIdentifier = segmentIdentifier
    }

    public var body: some View {
        HStack(spacing: 0) {
            ForEach(Array(options.enumerated()), id: \.offset) { _, option in
                let selected = option.value == selection
                Button {
                    selection = option.value
                } label: {
                    Text(option.title)
                        .dashFont(selected ? .footnoteMedium : .footnote)
                        .foregroundStyle(selected ? Color.dash.segmentControlContSelected : Color.dash.segmentControlContNotSelected)
                        .lineLimit(1)
                        .padding(.horizontal, DashSpacing.m)
                        .frame(maxWidth: .infinity, minHeight: 28)
                        .background(
                            Capsule().fill(selected ? Color.dash.segmentControlBackground : .clear)
                                .shadow(color: selected ? Color.role.shadow : .clear, radius: 2, y: 1))
                        .contentShape(Capsule())
                }
                .buttonStyle(.plain)
                .accessibilityAddTraits(selected ? .isSelected : [])
                .accessibilityIdentifier(segmentIdentifier?(option.value) ?? "")
            }
        }
        .padding(2)
        .background(Capsule().fill(Color.dash.segmentControlBackgroundGroup))
        .fixedSize(horizontal: true, vertical: false)
    }
}

// MARK: Sheet scaffold (C23)

/// Sheet chrome on `canvas`: DashUIKit's header (centred title, close ✕) then the content, then an
/// optional footer bar of buttons. Escape closes unless dismissal is locked.
public struct SheetScaffold<Content: View, Footer: View>: View {
    public let title: String
    public let width: Double?
    public let isDismissalEnabled: Bool
    public let onClose: (() -> Void)?
    @ViewBuilder public let content: () -> Content
    @ViewBuilder public let footer: () -> Footer

    public init(
        title: String, width: Double? = DashLayout.sheetWidthSmall, isDismissalEnabled: Bool = true,
        onClose: (() -> Void)? = nil, @ViewBuilder content: @escaping () -> Content,
        @ViewBuilder footer: @escaping () -> Footer
    ) {
        self.title = title
        self.width = width
        self.isDismissalEnabled = isDismissalEnabled
        self.onClose = onClose
        self.content = content
        self.footer = footer
    }

    public var body: some View {
        BottomSheet.selfSizing(
            title: title, showBackButton: .constant(false), isDismissalEnabled: .constant(isDismissalEnabled),
            onClose: onClose, background: Color.role.canvas
        ) {
            VStack(spacing: 0) {
                content()
                    .padding(.horizontal, DashLayout.pagePaddingH)
                    .padding(.bottom, DashSpacing.l)
                let bar = footer()
                if !(bar is EmptyView) {
                    HStack(spacing: DashSpacing.s) { bar }
                        .padding(.horizontal, DashLayout.pagePaddingH)
                        .padding(.vertical, DashSpacing.l)
                        .frame(maxWidth: .infinity)
                        .background(Color.role.card)
                        .overlay(alignment: .top) { Rectangle().fill(Color.role.separator).frame(height: 0.5) }
                }
            }
        }
        .frame(width: width.map { CGFloat($0) })
    }
}

public extension SheetScaffold where Footer == EmptyView {
    init(
        title: String, width: Double? = DashLayout.sheetWidthSmall, isDismissalEnabled: Bool = true,
        onClose: (() -> Void)? = nil, @ViewBuilder content: @escaping () -> Content
    ) {
        self.init(title: title, width: width, isDismissalEnabled: isDismissalEnabled, onClose: onClose,
                  content: content, footer: { EmptyView() })
    }
}

// MARK: States (C25, C26, C27)

/// Centred icon, title, message and an optional action.
public struct EmptyState<Action: View>: View {
    public let icon: DashIconSource
    public let title: String
    public let message: String?
    @ViewBuilder public let action: () -> Action

    public init(icon: DashIconSource, title: String, message: String? = nil, @ViewBuilder action: @escaping () -> Action) {
        self.icon = icon
        self.title = title
        self.message = message
        self.action = action
    }

    public var body: some View {
        VStack(spacing: DashSpacing.s) {
            DashIconImage(icon)
                .scaledToFit()
                .frame(width: 48, height: 48)
                .foregroundStyle(Color.role.textTertiary)
                .accessibilityHidden(true)
            Text(title)
                .dashFont(.headline)
                .foregroundStyle(Color.role.textPrimary)
                .multilineTextAlignment(.center)
            if let message {
                Text(message)
                    .dashFont(.subhead)
                    .foregroundStyle(Color.role.textSecondary)
                    .multilineTextAlignment(.center)
                    .fixedSize(horizontal: false, vertical: true)
            }
            action()
                .padding(.top, DashSpacing.xs)
        }
        .padding(DashSpacing.xxl)
        .frame(maxWidth: .infinity)
    }
}

public extension EmptyState where Action == EmptyView {
    init(icon: DashIconSource, title: String, message: String? = nil) {
        self.init(icon: icon, title: title, message: message) { EmptyView() }
    }
}

/// A small spinner and a `footnote` secondary line.
public struct LoadingState: View {
    public let text: String

    public init(_ text: String) {
        self.text = text
    }

    public var body: some View {
        HStack(spacing: DashSpacing.s) {
            ProgressView().controlSize(.small)
            Text(text)
                .dashFont(.footnote)
                .foregroundStyle(Color.role.textSecondary)
        }
        .padding(DashSpacing.l)
        .frame(maxWidth: .infinity)
    }
}

/// A 4 pt accent bar on the progress track; `value` nil draws an indeterminate bar.
public struct DashProgressBar: View {
    public let value: Double?
    public let height: Double

    public init(value: Double?, height: Double = 4) {
        self.value = value
        self.height = height
    }

    public var body: some View {
        if let value {
            GeometryReader { proxy in
                ZStack(alignment: .leading) {
                    Capsule().fill(Color.role.progressTrack)
                    Capsule().fill(Color.role.accent)
                        .frame(width: proxy.size.width * min(max(value, 0), 1))
                }
            }
            .frame(height: height)
            .accessibilityElement()
            .accessibilityValue(Text("\(Int((min(max(value, 0), 1) * 100).rounded())) %"))
        } else {
            ProgressView().progressViewStyle(.linear).tint(Color.role.accent)
        }
    }
}

// MARK: Toast host (C21)

/// A toast shown at the bottom centre of a window.
public struct ToastMessage: Equatable, Identifiable, Sendable {
    public let id = UUID()
    public let style: ToastStyle
    public let text: String

    public init(_ style: ToastStyle, _ text: String) {
        self.style = style
        self.text = text
    }
}

/// Shows `message` at the bottom centre, 24 pt above the edge, and clears it after 3 s (1.5 s for
/// "Copied").
public struct ToastHost: ViewModifier {
    @Binding var message: ToastMessage?

    public func body(content: Content) -> some View {
        content.overlay(alignment: .bottom) {
            if let message {
                Toast(style: message.style, message: message.text, onDismiss: { self.message = nil })
                    .fixedSize()
                    .padding(.bottom, DashSpacing.xxl)
                    .transition(.move(edge: .bottom).combined(with: .opacity))
                    .task(id: message.id) {
                        let seconds = message.style == .copied ? DashMotion.toastCopiedDuration : DashMotion.toastDuration
                        try? await Task.sleep(for: .seconds(seconds))
                        if self.message?.id == message.id { self.message = nil }
                    }
                    .accessibilityAddTraits(.isStaticText)
            }
        }
        .animation(.easeInOut(duration: DashMotion.overlay), value: message)
    }
}

public extension View {
    func dashToast(_ message: Binding<ToastMessage?>) -> some View {
        modifier(ToastHost(message: message))
    }
}

// MARK: Wizard header (C29)

/// "Step %1 of %2 · %3", a `title3` title and a segmented progress line.
public struct WizardHeader: View {
    public let stepText: String
    public let title: String
    public let step: Int
    public let count: Int

    public init(stepText: String, title: String, step: Int, count: Int) {
        self.stepText = stepText
        self.title = title
        self.step = step
        self.count = count
    }

    public var body: some View {
        VStack(alignment: .leading, spacing: DashSpacing.xs) {
            Text(stepText)
                .dashFont(.footnote)
                .foregroundStyle(Color.role.textSecondary)
            Text(title)
                .dashFont(.title3)
                .foregroundStyle(Color.role.textPrimary)
            HStack(spacing: DashSpacing.xxs) {
                ForEach(0..<max(count, 1), id: \.self) { index in
                    Capsule()
                        .fill(index < step ? Color.role.accent : Color.role.progressTrack)
                        .frame(height: 4)
                }
            }
        }
    }
}

// MARK: Phrase grid (C30)

/// Recovery words on one white card in three (12 words) or four (24) columns: the index in
/// `caption1` tertiary and the word in `calloutMedium` blue.
public struct PhraseGrid: View {
    public let words: [String]

    public init(words: [String]) {
        self.words = words
    }

    public var body: some View {
        let columns = Array(
            repeating: GridItem(.flexible(), spacing: DashSpacing.l, alignment: .leading), count: words.count > 12 ? 4 : 3)
        LazyVGrid(columns: columns, alignment: .leading, spacing: DashSpacing.m) {
            ForEach(Array(words.enumerated()), id: \.offset) { index, word in
                HStack(alignment: .firstTextBaseline, spacing: DashSpacing.s) {
                    Text("\(index + 1)")
                        .dashFont(.caption1)
                        .monospacedDigit()
                        .foregroundStyle(Color.role.textTertiary)
                        .frame(minWidth: 18, alignment: .trailing)
                    Text(word)
                        .dashFont(.calloutMedium)
                        .foregroundStyle(Color.role.textLink)
                }
                .accessibilityElement(children: .combine)
            }
        }
        .padding(DashSpacing.xl)
        .dashCard(padding: nil)
    }
}
#endif
