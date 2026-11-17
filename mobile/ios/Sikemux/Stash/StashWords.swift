import Foundation

/// A stashed link or text as a chat's draft holds it, in `extra.context`: a chip in every app, sent with the message.
typealias ContextChip = ChatContext

/// How the stash reads in lists, and how its items go into a message.
enum StashWords {
    static func site(of url: String) -> String {
        let bare = url.replacingOccurrences(
            of: #"^[a-z]+://"#, with: "", options: [.regularExpression, .caseInsensitive])
        return bare.split(whereSeparator: { "/?#".contains($0) }).first.map(String.init) ?? url
    }

    /// What an item is called in a list: a page's title, a text's or prompt's first line, or a file's name.
    static func title(_ item: StashItem) -> String {
        switch item.kind {
        case .link:
            if let title = item.preview?.title, !title.isEmpty { return title }
            return (item.url ?? "").replacingOccurrences(of: #"^https?://"#, with: "", options: .regularExpression)
        case .text, .prompt:
            return (item.text ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
                .split(separator: "\n", omittingEmptySubsequences: false).first.map(String.init) ?? ""
        default:
            return item.name ?? String(localized: "file")
        }
    }

    /// The line under it: where a link goes or what the item is, then how long ago it was stashed.
    static func place(_ item: StashItem, now: Date = Date()) -> String {
        let what: String
        switch item.kind {
        case .link:
            if let site = item.preview?.site, !site.isEmpty { what = site } else { what = site(of: item.url ?? "") }
        case .text: what = String(localized: "Text")
        case .prompt:
            let files = item.attachments?.count ?? 0
            switch files {
            case 0: what = String(localized: "Prompt")
            case 1: what = String(localized: "Prompt · 1 file")
            default: what = String(localized: "Prompt · \(files) files")
            }
        default: what = Format.bytes(item.size ?? 0)
        }
        let age = Format.age(Format.date(iso: item.createdAt), now: now)
        return "\(what) · \(age == Format.age(now, now: now) ? String(localized: "just now") : age)"
    }

    private static let mention = #"(^|\s)&(\S*)$"#

    /// What follows an `&` that starts the last word of `draft`, or nil when there is none.
    static func query(in draft: String) -> String? {
        guard let found = draft.range(of: mention, options: .regularExpression) else { return nil }
        let word = draft[found]
        return word.firstIndex(of: "&").map { String(word[word.index(after: $0)...]) }
    }

    /// `draft` without the `&` word it ends with.
    static func withoutQuery(_ draft: String) -> String {
        guard let found = draft.range(of: mention, options: .regularExpression) else { return draft }
        let lead = draft[found].first.map { $0.isWhitespace ? String($0) : "" } ?? ""
        var kept = draft.replacingCharacters(in: found, with: lead)
        while kept.last?.isWhitespace == true { kept.removeLast() }
        return kept
    }

    /// The items whose title, note, link or text holds every word of `query`, ignoring case.
    static func matches(_ items: [StashItem], _ query: String) -> [StashItem] {
        let words = query.lowercased().split(whereSeparator: { $0.isWhitespace || $0 == "&" })
        return items.filter { item in
            let said = [title(item), item.note, item.url, item.text, item.preview?.site].compactMap { $0 }
                .filter { !$0.isEmpty }.joined(separator: " ").lowercased()
            return words.allSatisfy { said.contains($0) }
        }
    }

    /// How the host puts a link or text in a draft, for a draft that is still only on this phone.
    static func contextChip(_ item: StashItem) -> ContextChip {
        if item.kind == .link {
            let url = item.url ?? ""
            let title = item.preview?.title.isEmpty == false ? item.preview!.title : url
            let text = [title, url, item.preview?.description].compactMap { $0 }.filter { !$0.isEmpty }
            return ContextChip(uri: url, title: title, text: text.joined(separator: "\n\n"))
        }
        return ContextChip(
            uri: "\(WireURIs.stashURI)\(item.id)", title: String(title(item).prefix(80)), text: item.text ?? "")
    }

    /// The message as this phone sends it: what was written, then each chip's words.
    static func withContext(_ text: String, _ chips: [ContextChip]) -> String {
        ([text] + chips.map(\.text)).filter { !$0.isEmpty }.joined(separator: "\n\n")
    }

    struct IntoMessage {
        let draft: String
        /// Links and text, which ride beside the words as chips until the message is sent.
        let chips: [StashItem]
        /// The files the items carry, once each is on this phone; nil when none does.
        let files: Task<[(item: StashItem, url: URL)], Error>?
        /// Stashed prompts, which came back whole and leave the stash at once.
        let restored: [String]
    }

    /// What picking stash items does to a message: each item's note joins its words, a link or text rides beside
    /// them as a chip, and each file is fetched to go with it. A stashed prompt comes back whole, its words into the
    /// message and its files beside them; those files are no longer stash items of their own.
    static func intoMessage(_ items: [StashItem], draft: String) -> IntoMessage {
        let prompts = items.filter { $0.kind == .prompt }
        let rest = items.filter { $0.kind != .prompt }
        let words =
            (prompts.map { ($0.text ?? "").trimmingCharacters(in: .whitespacesAndNewlines) }
            + rest.compactMap { $0.note?.trimmingCharacters(in: .whitespacesAndNewlines) })
            .filter { !$0.isEmpty }
        let fetching = rest.filter(\.carriesFile) + prompts.flatMap { $0.attachments ?? [] }
        let trimmed = String(draft.reversed().drop(while: \.isWhitespace).reversed())
        return IntoMessage(
            draft: words.isEmpty ? draft : ([trimmed] + words).filter { !$0.isEmpty }.joined(separator: "\n\n"),
            chips: rest.filter { !$0.carriesFile },
            files: fetching.isEmpty
                ? nil
                : Task {
                    try await withThrowingTaskGroup(of: (Int, StashItem, URL).self) { group in
                        for (index, item) in fetching.enumerated() {
                            group.addTask { (index, item, try await StashAPI.download(item)) }
                        }
                        var done: [(Int, StashItem, URL)] = []
                        for try await one in group { done.append(one) }
                        return done.sorted { $0.0 < $1.0 }.map { ($0.1, $0.2) }
                    }
                },
            restored: prompts.map(\.id)
        )
    }
}
