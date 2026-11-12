import SwiftUI

/// The chat's model and effort, picked as the agent offers them, and how full its context is.
struct ConfigSheet: View {
    @Environment(\.palette) private var palette
    let provider: String
    let model: ConfigPicker?
    let effort: ConfigPicker?
    let usage: ContextUsage?
    let onPick: (ConfigPicker, String) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 10) {
                AgentMark(provider: provider, size: 20)
                Text(Catalog.label(provider)).headingStyle(palette)
            }
            .padding(.horizontal, 4)
            if let model {
                label("Model")
                VStack(spacing: 0) {
                    ForEach(Array(model.choices.enumerated()), id: \.element.value) { index, option in
                        let on = option.value == model.current
                        Button {
                            onPick(model, option.value)
                        } label: {
                            HStack(spacing: 12) {
                                AgentMark(provider: provider, size: 18)
                                Text(option.label)
                                    .font(Theme.row)
                                    .foregroundStyle(on ? palette.ink : palette.secondary)
                                    .lineLimit(1)
                                    .frame(maxWidth: .infinity, alignment: .leading)
                                if on { IconView(icon: Icons.check, size: 17).foregroundStyle(palette.ink) }
                            }
                            .padding(.horizontal, 14)
                            .frame(minHeight: 52)
                            .background(on ? palette.active : .clear)
                            .overlay(alignment: .top) { if index > 0 { palette.border.frame(height: 1) } }
                            .contentShape(Rectangle())
                        }
                        .buttonStyle(PressFade())
                        .accessibilityAddTraits(on ? .isSelected : [])
                    }
                }
                .background(palette.raised)
                .clipShape(RoundedRectangle(cornerRadius: Radius.card, style: .continuous))
                .overlay(
                    RoundedRectangle(cornerRadius: Radius.card, style: .continuous).strokeBorder(
                        palette.border, lineWidth: 1))
            }
            if let effort {
                label("Effort")
                Track(
                    options: effort.choices.map { Track.Option(value: $0.value, label: $0.label) },
                    value: Binding(get: { effort.current }, set: { onPick(effort, $0) })
                )
            }
            if let usage {
                let share = ContextFill.share(usage)
                VStack(alignment: .leading, spacing: 0) {
                    HStack {
                        Text("Context").font(Theme.label).foregroundStyle(palette.tertiary)
                        Spacer()
                        Text(
                            "\(Int((share * 100).rounded()))% · \(Format.tokens(usage.used)) of \(Format.tokens(ContextFill.limit(usage)))"
                        )
                        .font(Theme.mono(12))
                        .foregroundStyle(palette.tertiary)
                    }
                    GeometryReader { proxy in
                        ZStack(alignment: .leading) {
                            Capsule().fill(palette.border)
                            Capsule().fill(ContextFill.color(usage, provider: provider, palette: palette))
                                .frame(width: proxy.size.width * share)
                        }
                    }
                    .frame(height: 4)
                    .padding(.top, 8)
                    if let compactAt = usage.compactAt {
                        Text("Compacts at \(Format.tokens(compactAt)) of \(Format.tokens(usage.size))")
                            .font(Theme.ui(12.5)).foregroundStyle(palette.tertiary).padding(.top, 8)
                    }
                    if let cost = usage.cost {
                        Text("Session cost \(ContextFill.money(cost))").font(Theme.ui(12.5)).foregroundStyle(
                            palette.tertiary
                        ).padding(.top, 8)
                    }
                }
                .padding(.top, 20)
                .padding(.horizontal, 4)
            }
        }
    }

    private func label(_ text: LocalizedStringKey) -> some View {
        Text(text)
            .font(Theme.label)
            .foregroundStyle(palette.tertiary)
            .padding(.top, 18)
            .padding(.bottom, 8)
            .padding(.horizontal, 6)
    }
}

/// What this chat sent before, newest first; a tap puts one back in the composer.
struct RecentSheet: View {
    @Environment(\.palette) private var palette
    let prompts: [String]
    let onPick: (String) -> Void

    private static let shown = 20

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            Text("Sent in this chat")
                .font(Theme.label)
                .foregroundStyle(palette.tertiary)
                .padding(.bottom, 8)
                .padding(.horizontal, 6)
            ForEach(Array(prompts.reversed().prefix(Self.shown).enumerated()), id: \.offset) { _, prompt in
                Button {
                    onPick(prompt)
                } label: {
                    HStack(alignment: .top, spacing: 12) {
                        IconView(icon: Icons.clock, size: 15).foregroundStyle(palette.inkFaint).padding(.top, 2)
                        Text(prompt)
                            .font(Theme.ui(14.5))
                            .lineSpacing(20 - 14.5 * 1.2)
                            .foregroundStyle(palette.secondary)
                            .lineLimit(2)
                            .multilineTextAlignment(.leading)
                            .frame(maxWidth: .infinity, alignment: .leading)
                    }
                    .padding(.vertical, 11)
                    .padding(.horizontal, 8)
                    .contentShape(Rectangle())
                }
                .buttonStyle(ActiveFill(radius: 9))
                .accessibilityHint("Puts it in the composer")
            }
        }
        .padding(.bottom, 4)
    }

    /// What the person sent in this chat, oldest first, with a message repeated back to back kept once.
    static func sent(_ messages: [ChatMessage], queued: [String] = []) -> [String] {
        let texts = messages.filter(isPrompt).map(promptText)
        var prompts: [String] = []
        for text in texts + queued.map({ $0.trimmingCharacters(in: .whitespacesAndNewlines) })
        where !text.isEmpty && prompts.last != text {
            prompts.append(text)
        }
        return prompts
    }

    /// Whether the sheet has anything to show for `rows`, found from the newest row back.
    static func offered(_ rows: ChatRows) -> Bool {
        rows.queued.contains { !$0.text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }
            || rows.messages.reversed().contains { isPrompt($0) && !promptText($0).isEmpty }
    }

    private static func isPrompt(_ message: ChatMessage) -> Bool {
        message.role == .user && (message.letters ?? []).isEmpty
    }

    private static func promptText(_ message: ChatMessage) -> String {
        message.parts.map { part in if case .text(let text) = part { text.text } else { "" } }.joined()
            .trimmingCharacters(in: .whitespacesAndNewlines)
    }
}
