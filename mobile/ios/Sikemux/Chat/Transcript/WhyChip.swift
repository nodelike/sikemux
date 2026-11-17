import SwiftUI

extension EnvironmentValues {
    /// The `why` links the core checked in the answer being drawn.
    @Entry var whyChips: [WhyChip] = []
}

/// What a claim in an answer rests on, as the core checked it, drawn in the line as the Mac's chip: what it is, then
/// when. Only a chip of another chat opens, to that agent's chat; the phone has nowhere to show a commit, a rule or a call.
enum WhyLink {
    static let prefix = "sikemux://why/"
    static let scheme = "sikemux-why"

    /// The chip the core checked for `href`, or nothing for a link it did not check, as the Mac draws it.
    static func chip(_ href: String, in chips: [WhyChip], palette: Palette) -> AttributedString {
        guard let chip = chips.first(where: { $0.href == href }) else { return AttributedString() }
        let (icon, key, title) = words(chip)
        var run = FileGlyph.mark(icon: icon, size: 12, tint: palette.tertiary) + AttributedString(" \(key)")
        run.foregroundColor = palette.ink
        if !title.isEmpty {
            var when = AttributedString(" \(title)")
            when.foregroundColor = palette.secondary
            run += when
        }
        run.backgroundColor = palette.active
        if case .chat(let chat) = chip.open {
            var parts = URLComponents()
            parts.scheme = scheme
            parts.host = "chat"
            parts.queryItems = [URLQueryItem(name: "agent", value: chat.agent)]
            run.link = parts.url
        }
        return run
    }

    /// The chips of an answer in `here`'s chat, where a chip of that same chat has nothing to open.
    static func chips(_ chips: [WhyChip], here: String?) -> [WhyChip] {
        chips.map { chip in
            guard case .chat(let chat) = chip.open, chat.agent == here else { return chip }
            var shown = chip
            shown.open = .unknown
            return shown
        }
    }

    /// The agent whose chat a tapped chip opens.
    static func agent(_ url: URL) -> String? {
        guard url.scheme == scheme else { return nil }
        return URLComponents(url: url, resolvingAgainstBaseURL: false)?.queryItems?.first { $0.name == "agent" }?.value
    }

    private static func words(_ chip: WhyChip) -> (icon: String, key: String, title: String) {
        let day = { (time: Bool) in chip.at.map { said(Date(timeIntervalSince1970: Double($0) / 1000), time) } ?? "" }
        switch chip.kind {
        case .said: return ("IconMessage", String(localized: "your message"), day(true))
        case .change where chip.landed == true:
            return ("IconCommit", String(localized: "the change"), String(localized: "landed \(day(false))"))
        case .change: return ("IconCommit", String(localized: "the change"), day(false))
        case .call: return ("IconCheck", String(localized: "your answer"), day(false))
        case .rule: return ("IconBook", String(localized: "your rule"), String(localized: "said \(day(false))"))
        case .decision: return ("IconMerge", String(localized: "decided"), day(false))
        case .unknown: return ("IconMessage", String(localized: "source"), day(false))
        }
    }

    /// When something happened, as a person says it looking back: the time today, the weekday this week, else the date.
    private static func said(_ at: Date, _ time: Bool) -> String {
        let calendar = Calendar.current
        let days =
            calendar.dateComponents([.day], from: calendar.startOfDay(for: at), to: calendar.startOfDay(for: .now))
            .day ?? 0
        let clock = at.formatted(date: .omitted, time: .shortened)
        let weekday = at.formatted(.dateTime.weekday(.abbreviated))
        if days <= 0 { return time ? clock : String(localized: "today") }
        if days <= 6 { return time ? "\(weekday) \(clock)" : weekday }
        return at.formatted(.dateTime.month(.abbreviated).day())
    }
}
