import SwiftUI

enum PromptTone {
    case neutral, danger
}

/// One of the Mac's icons on a raised tile: large over a prompt's title, small over an empty list's line.
struct PromptGlyph: View {
    @Environment(\.palette) private var palette
    let icon: IconDef
    var tone: PromptTone = .neutral
    var small = false

    var body: some View {
        let shape = RoundedRectangle(cornerRadius: small ? 12 : 18, style: .continuous)
        IconView(icon: icon, size: small ? 20 : 34)
            .foregroundStyle(tone == .danger ? palette.danger : small ? palette.secondary : palette.ink)
            .frame(width: small ? 44 : 76, height: small ? 44 : 76)
            .background(shape.fill(tone == .danger ? palette.danger.opacity(0.08) : palette.raised))
            .overlay(
                shape.strokeBorder(
                    tone == .danger ? palette.danger.opacity(0.55) : small ? palette.border : palette.borderStrong,
                    lineWidth: 1
                )
            )
            .accessibilityHidden(true)
    }
}

/// A glyph, a title and a line under it, centred. `fill` centres it in the space it is given, for a screen of its own.
struct Prompt<Content: View>: View {
    @Environment(\.palette) private var palette
    let icon: IconDef
    var tone: PromptTone = .neutral
    let title: String
    var detail: String?
    var fill = false
    @ViewBuilder var content: Content

    var body: some View {
        VStack(spacing: 0) {
            PromptGlyph(icon: icon, tone: tone)
            Text(LocalizedStringKey(title))
                .font(Theme.ui(22, .semibold))
                .tracking(-0.7)
                .lineSpacing(5)
                .foregroundStyle(palette.ink)
                .multilineTextAlignment(.center)
                .padding(.top, 20)
                .accessibilityAddTraits(.isHeader)
            if let detail {
                Text(LocalizedStringKey(detail))
                    .font(Theme.body)
                    .lineSpacing(7)
                    .foregroundStyle(palette.secondary)
                    .multilineTextAlignment(.center)
                    .padding(.top, 8)
            }
            content
        }
        .padding(.horizontal, 28)
        .padding(.top, fill ? 0 : 32)
        .padding(.bottom, fill ? 120 : 0)
        .frame(maxWidth: .infinity, maxHeight: fill ? .infinity : nil)
    }
}

extension Prompt where Content == EmptyView {
    init(icon: IconDef, tone: PromptTone = .neutral, title: String, detail: String? = nil, fill: Bool = false) {
        self.init(icon: icon, tone: tone, title: title, detail: detail, fill: fill) { EmptyView() }
    }
}

/// A screen that is only a prompt, with its actions along the bottom.
struct PromptScreen<Actions: View>: View {
    let icon: IconDef
    var tone: PromptTone = .neutral
    let title: String
    var detail: String?
    @ViewBuilder var actions: Actions

    var body: some View {
        Screen {
            Prompt(icon: icon, tone: tone, title: title, detail: detail, fill: true)
            VStack(spacing: 8) { actions }
                .padding(.horizontal, 16)
                .padding(.top, 12)
                .padding(.bottom, bottomGap)
        }
    }
}

struct EmptyState: View {
    @Environment(\.palette) private var palette
    let icon: IconDef
    let text: String

    var body: some View {
        VStack(spacing: 12) {
            PromptGlyph(icon: icon, small: true)
            Text(LocalizedStringKey(text)).font(Theme.meta).foregroundStyle(palette.tertiary)
                .multilineTextAlignment(.center)
        }
        .padding(.top, 40)
        .padding(.horizontal, 28)
    }
}

/// A line saying what went wrong, in the danger colour.
struct ProblemLine: View {
    @Environment(\.palette) private var palette
    let text: String
    var centred = false

    var body: some View {
        Text(text)
            .font(Theme.meta)
            .foregroundStyle(palette.danger)
            .multilineTextAlignment(centred ? .center : .leading)
            .frame(maxWidth: .infinity, alignment: centred ? .center : .leading)
            .padding(.top, 4)
    }
}

/// Something the person asked for that failed, with why, for `failureAlert`.
struct Failure: Identifiable {
    let id = UUID()
    let title: String
    let detail: String

    init(_ title: String, _ error: Error) {
        self.title = title
        detail = problem(error)
    }
}

extension View {
    /// An alert saying what failed and why, as the Mac says it when an action it was asked for fails.
    func failureAlert(_ failure: Binding<Failure?>) -> some View {
        alert(
            Text(LocalizedStringKey(failure.wrappedValue?.title ?? "")),
            isPresented: Binding(get: { failure.wrappedValue != nil }, set: { if !$0 { failure.wrappedValue = nil } }),
            presenting: failure.wrappedValue
        ) { _ in
            Button("OK", role: .cancel) {}
        } message: { shown in
            Text(shown.detail)
        }
    }
}
