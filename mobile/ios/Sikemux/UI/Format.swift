import Foundation

/// The app's words for times and counts, as `shared/lib/format.ts` writes them.
enum Format {
    private static let ages: [(limit: Double, unit: String, per: Double)] = [
        (3600, "m", 60), (86_400, "h", 3600), (2_592_000, "d", 86_400), (31_536_000, "mo", 2_592_000),
    ]

    /// How long ago, in a tight column: `now`, `5m`, `3h`, `4d`, `2mo`, `1y`.
    static func age(_ at: Date?, now: Date = Date()) -> String {
        guard let at else { return "—" }
        let seconds = max(0, floor(now.timeIntervalSince(at)))
        if seconds < 60 { return "now" }
        for age in ages where seconds < age.limit { return "\(Int(seconds / age.per))\(age.unit)" }
        return "\(Int(seconds / 31_536_000))y"
    }

    /// How long ago, in a sentence: `now`, `5m ago`, `3h ago`.
    static func ago(_ at: Date?, now: Date = Date()) -> String {
        let age = age(at, now: now)
        return age == "now" || age == "—" ? age : "\(age) ago"
    }

    /// Epoch milliseconds, as the core and the saved files write times.
    static func date(millis: Double?) -> Date? {
        millis.map { Date(timeIntervalSince1970: $0 / 1000) }
    }

    static func date(millis: Int) -> Date {
        Date(timeIntervalSince1970: Double(millis) / 1000)
    }

    /// An ISO date the core sent.
    static func date(iso: String?) -> Date? {
        guard let iso else { return nil }
        return (try? Date(iso, strategy: isoFractional)) ?? (try? Date(iso, strategy: .iso8601))
    }

    /// A countdown in words: `5 min`, `1 hour`, `3 hours`, `2 days`.
    static func timeLeft(until: Date, now: Date = Date()) -> String {
        let minutes = max(1, Int((until.timeIntervalSince(now) / 60).rounded()))
        if minutes < 60 { return "\(minutes) min" }
        let hours = Int((Double(minutes) / 60).rounded())
        if hours < 48 { return hours == 1 ? "1 hour" : "\(hours) hours" }
        return "\(Int((Double(hours) / 24).rounded())) days"
    }

    /// `word` as it reads beside `count`: "1 agent", "2 agents".
    static func counted(_ word: String, _ count: Int) -> String {
        count == 1 ? word : "\(word)s"
    }

    /// A count of tokens, short: 950, 12.3K, 1.2M.
    static func tokens(_ count: Double) -> String {
        count.formatted(
            .number.notation(.compactName).precision(.fractionLength(0...1)).locale(Locale(identifier: "en")))
    }

    static func tokens(_ count: Int) -> String { tokens(Double(count)) }

    /// A size in 1024s: `512 B`, `2.0 KB`, `15 MB`.
    static func bytes(_ count: Int) -> String {
        if abs(count) < 1024 { return "\(count) B" }
        let units = ["KB", "MB", "GB", "TB"]
        var value = Double(count) / 1024
        var unit = 0
        while abs(value) >= 1024, unit < units.count - 1 {
            value /= 1024
            unit += 1
        }
        let shown = abs(value) < 10 ? String(format: "%.1f", value) : String(Int(value.rounded()))
        return "\(shown) \(units[unit])"
    }

    private static let isoFractional = Date.ISO8601FormatStyle(includingFractionalSeconds: true)
}
