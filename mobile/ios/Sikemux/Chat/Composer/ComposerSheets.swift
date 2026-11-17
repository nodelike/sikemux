import SwiftUI

/// The chat's agent, model, effort and speed, picked as the agent offers them, and how full its context is.
/// Laid out as the Mac's model menu: the agents across the top, the agent's models under a search, then the effort.
struct ConfigSheet: View {
    @Environment(\.palette) private var palette
    @Environment(\.displayScale) private var scale
    @Environment(\.hostCore) private var core
    let provider: String
    /// The agents the chat can go to, in the strip across the top; it shows with more than one.
    var agents: [String] = []
    var onAgent: (String) -> Void = { _ in }
    let model: ConfigPicker?
    let effort: ConfigPicker?
    let speed: ConfigPicker?
    var usage: ContextUsage?
    /// Why the agent could not say its models, while the chat has not started.
    var unread: ChatFailure?
    var hostName = ""
    var expiredCache: ExpiredCache?
    var onSummarise: (Bool) -> Void = { _ in }
    let onPick: (ConfigPicker, String) -> Void
    /// The connected providers the agent can run on, which a chat can move to before it starts.
    var routes: [ModelRoute] = []
    /// The provider and model picked from `routes`.
    var routed: RoutePick?
    var onRoute: (RoutePick) -> Void = { _ in }
    @State private var query = ""
    /// The older releases of the agent's models show, which start folded away.
    @State private var older = false

    var body: some View {
        let name = Catalog.label(provider, core: core)
        let typed = query.trimmingCharacters(in: .whitespaces).lowercased()
        VStack(alignment: .leading, spacing: 0) {
            if agents.count > 1 {
                ScrollView(.horizontal, showsIndicators: false) {
                    HStack(spacing: 6) {
                        ForEach(agents, id: \.self) { agent in
                            let on = agent == provider
                            Button {
                                if !on { onAgent(agent) }
                            } label: {
                                AgentMark(provider: agent, size: 20)
                                    .frame(width: 38, height: 38)
                                    .background(
                                        RoundedRectangle(cornerRadius: 9, style: .continuous).fill(
                                            on ? palette.active : .clear)
                                    )
                                    .contentShape(Rectangle())
                            }
                            .buttonStyle(PressFade())
                            .accessibilityLabel(Catalog.label(agent, core: core))
                            .accessibilityAddTraits(on ? .isSelected : [])
                        }
                    }
                }
                .padding(.bottom, 10)
            } else {
                HStack(spacing: 10) {
                    AgentMark(provider: provider, size: 20)
                    Text(name).headingStyle(palette)
                }
                .padding(.horizontal, 4)
                .padding(.bottom, 12)
            }
            if let unread {
                let signedOut = String(
                    localized: "\(name) isn't signed in on \(hostName). Sign in there, then its models show here.")
                let failed = unread.details.map { String(localized: "Couldn't ask \(name) for its models: \($0)") }
                Text(
                    unread.kind == .signIn
                        ? signedOut : failed ?? String(localized: "Couldn't ask \(name) for its models.")
                )
                .font(Theme.ui(13)).lineSpacing(3).foregroundStyle(palette.secondary)
                .padding(.horizontal, 4).padding(.bottom, 12)
            }
            if let model {
                Field(
                    placeholder: String(localized: "Search \(name)'s models…"), text: $query, icon: Icons.search,
                    font: Theme.ui(15))
                let heading = Catalog.entry(provider, core: core)?.maker ?? name
                let routed = routes.map { ($0, Self.matching($0, typed: typed)) }.filter { !$0.1.isEmpty }
                models(model, typed: typed, heading: heading, others: !routed.isEmpty)
                ForEach(routed, id: \.0.id) { route, shown in
                    section(route, shown: shown)
                }
            }
            if typed.isEmpty {
                if let effort, !effort.choices.isEmpty {
                    HStack(spacing: 6) {
                        IconView(icon: Icons.gauge, size: 13)
                        Text("Effort")
                        Spacer(minLength: 8)
                        if let model = Composer.label(model) {
                            Text(model).font(Theme.ui(12)).lineLimit(1)
                        }
                    }
                    .font(Theme.label)
                    .foregroundStyle(palette.tertiary)
                    .padding(.top, 14)
                    .padding(.bottom, 8)
                    .padding(.horizontal, 6)
                    EffortSteps(effort: effort) { onPick(effort, $0) }
                }
                if let speed, speed.choices.count > 1 {
                    HStack {
                        Text("Speed").font(Theme.label).foregroundStyle(palette.tertiary)
                        Spacer(minLength: 12)
                        Track(
                            options: speed.choices.map { Track.Option(value: $0.value, label: $0.label) },
                            value: Binding(get: { speed.current }, set: { onPick(speed, $0) })
                        )
                        .frame(maxWidth: 200)
                    }
                    .padding(.top, 14)
                    .padding(.leading, 6)
                }
                if let usage {
                    let share = ContextFill.share(usage)
                    VStack(alignment: .leading, spacing: 0) {
                        HStack {
                            (usage.compactAt == nil ? Text("Context") : Text("Until compaction")).font(Theme.label)
                                .foregroundStyle(palette.tertiary)
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
                        if usage.compactAt != nil {
                            Text("Model window \(Format.tokens(usage.size))")
                                .font(Theme.ui(12.5)).foregroundStyle(palette.inkDim).padding(.top, 8)
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
                if let expiredCache {
                    VStack(alignment: .leading, spacing: 0) {
                        let ago = Format.ago(Format.date(millis: expiredCache.at))
                        Text(
                            "Cache expired \(ago) · your next message re-reads all \(Format.tokens(expiredCache.tokens)) tokens"
                        )
                        .font(Theme.ui(12.5)).foregroundStyle(palette.secondary)
                        Toggle(
                            isOn: Binding(get: { expiredCache.summarise }, set: { onSummarise($0) })
                        ) {
                            Text("Summarise the chat before your next message")
                                .font(Theme.ui(15, .medium)).foregroundStyle(palette.ink)
                        }
                        .tint(palette.accent)
                        .frame(minHeight: 48)
                        .padding(.top, 8)
                    }
                    .padding(.top, 16)
                    .overlay(alignment: .top) { palette.border.frame(height: 1 / scale) }
                    .padding(.top, 18)
                    .padding(.horizontal, 4)
                }
            }
        }
    }

    /// The agent's models under its maker, matched to the search, its older releases folded away until asked.
    private func models(_ model: ConfigPicker, typed: String, heading: String, others: Bool) -> some View {
        let unfolded = older || !typed.isEmpty
        let folds = model.choices.contains { $0.superseded == true && $0.value != model.current }
        let shown = model.choices.filter { choice in
            (unfolded || choice.superseded != true || choice.value == model.current)
                && (typed.isEmpty
                    || "\(choice.label) \(choice.description ?? "") \(heading)".lowercased().contains(typed))
        }
        return VStack(alignment: .leading, spacing: 2) {
            if !shown.isEmpty || !others { SectionLabel(heading) }
            ForEach(shown, id: \.value) { choice in
                let on = routed == nil && choice.value == model.current
                Button {
                    onPick(model, choice.value)
                } label: {
                    HStack(spacing: 11) {
                        AgentMark(provider: provider, size: 17)
                        Text(choice.label)
                            .font(Theme.ui(15))
                            .foregroundStyle(on ? palette.ink : palette.secondary)
                            .lineLimit(1)
                        if choice.isDefault == true {
                            Text("Default").font(Theme.ui(12)).foregroundStyle(palette.tertiary).lineLimit(1)
                        }
                        Spacer(minLength: 0)
                        if on { IconView(icon: Icons.check, size: 15).foregroundStyle(palette.accent) }
                    }
                    .padding(.horizontal, 10)
                    .frame(height: 42)
                    .background(
                        RoundedRectangle(cornerRadius: 9, style: .continuous).fill(on ? palette.active : .clear)
                    )
                    .contentShape(Rectangle())
                }
                .buttonStyle(PressFade())
                .accessibilityAddTraits(on ? .isSelected : [])
            }
            if folds && typed.isEmpty {
                Button {
                    older.toggle()
                } label: {
                    HStack(spacing: 11) {
                        IconView(icon: Icons.chevron, size: 14)
                            .rotationEffect(.degrees(older ? -90 : 0))
                            .frame(width: 17)
                        Text(older ? "Show less" : "Show more").font(Theme.ui(15))
                        Spacer(minLength: 0)
                    }
                    .foregroundStyle(palette.tertiary)
                    .padding(.horizontal, 10)
                    .frame(height: 42)
                    .contentShape(Rectangle())
                }
                .buttonStyle(PressFade())
            }
            if shown.isEmpty && !others {
                Text("No model matches").font(Theme.ui(14)).foregroundStyle(palette.tertiary)
                    .padding(.horizontal, 10).padding(.vertical, 12)
            }
        }
    }

    /// The route's models matched to the search, by their name, what is known of them and the provider's.
    private static func matching(_ route: ModelRoute, typed: String) -> [String] {
        guard !typed.isEmpty else { return route.models }
        return route.models.filter { "\($0) \(facts(route, $0)) \(route.label)".lowercased().contains(typed) }
    }

    /// Context and price per million tokens, as the Mac's menu words them.
    private static func facts(_ route: ModelRoute, _ model: String) -> String {
        guard let facts = route.facts[model] else { return "" }
        var words = [String(localized: "\(Format.tokens(facts.context)) context")]
        if let input = facts.input, let output = facts.output {
            let price = { (usd: Double) in usd.formatted(.number.precision(.fractionLength(0...2))) }
            words.append(String(localized: "$\(price(input))/$\(price(output)) per 1M"))
        }
        return words.joined(separator: " · ")
    }

    private func section(_ route: ModelRoute, shown: [String]) -> some View {
        let heading =
            route.local && !route.label.hasPrefix("Local")
            ? String(localized: "\(route.label) · local") : route.label
        return VStack(alignment: .leading, spacing: 2) {
            SectionLabel(heading)
            ForEach(shown, id: \.self) { model in
                let pick = RoutePick(provider: route.id, model: model)
                let on = routed == pick
                let facts = Self.facts(route, model)
                Button {
                    onRoute(pick)
                } label: {
                    HStack(spacing: 11) {
                        AgentMark(provider: provider, size: 17)
                        VStack(alignment: .leading, spacing: 1) {
                            Text(model)
                                .font(Theme.ui(15))
                                .foregroundStyle(on ? palette.ink : palette.secondary)
                                .lineLimit(1)
                            if !facts.isEmpty {
                                Text(facts).font(Theme.ui(12)).foregroundStyle(palette.tertiary).lineLimit(1)
                            }
                        }
                        Spacer(minLength: 0)
                        if on { IconView(icon: Icons.check, size: 15).foregroundStyle(palette.accent) }
                    }
                    .padding(.horizontal, 10)
                    .frame(minHeight: 42)
                    .background(
                        RoundedRectangle(cornerRadius: 9, style: .continuous).fill(on ? palette.active : .clear)
                    )
                    .contentShape(Rectangle())
                }
                .buttonStyle(PressFade())
                .accessibilityAddTraits(on ? .isSelected : [])
            }
        }
        .padding(.top, 10)
    }
}

/// A connected provider's model a chat moves to before it starts.
struct RoutePick: Equatable {
    let provider: String
    let model: String
}

/// The efforts a model thinks at, side by side, each led by the Mac's strength bars.
private struct EffortSteps: View {
    @Environment(\.palette) private var palette
    let effort: ConfigPicker
    let onPick: (String) -> Void

    var body: some View {
        let steps = effort.choices.count
        HStack(spacing: 3) {
            ForEach(Array(effort.choices.enumerated()), id: \.element.value) { at, choice in
                let on = choice.value == effort.current
                Button {
                    Haptics.select()
                    onPick(choice.value)
                } label: {
                    VStack(spacing: 6) {
                        Strength(lit: ((at + 1) * 5 + steps - 1) / steps)
                        Text(choice.label)
                            .font(Theme.ui(12))
                            .foregroundStyle(on ? palette.ink : palette.secondary)
                            .lineLimit(1)
                            .minimumScaleFactor(0.8)
                    }
                    .padding(.vertical, 9)
                    .frame(maxWidth: .infinity)
                    .background(
                        RoundedRectangle(cornerRadius: 9, style: .continuous).fill(on ? palette.active : .clear)
                    )
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityAddTraits(on ? .isSelected : [])
            }
        }
        .padding(3)
        .background(RoundedRectangle(cornerRadius: Radius.card, style: .continuous).fill(palette.raised))
        .overlay(
            RoundedRectangle(cornerRadius: Radius.card, style: .continuous).strokeBorder(palette.border, lineWidth: 1))
    }
}

/// Five bars rising left to right, `lit` of them in the accent.
private struct Strength: View {
    @Environment(\.palette) private var palette
    let lit: Int

    var body: some View {
        HStack(alignment: .bottom, spacing: 1.5) {
            ForEach(0..<5, id: \.self) { bar in
                RoundedRectangle(cornerRadius: 1)
                    .fill(bar < lit ? palette.accent : palette.borderStrong)
                    .frame(width: 2.5, height: 3 + 2 * CGFloat(bar))
            }
        }
        .frame(height: 11, alignment: .bottom)
        .accessibilityHidden(true)
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
