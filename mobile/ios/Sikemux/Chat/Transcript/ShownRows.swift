import Foundation

/// How a row of a run of messages between agents shows.
struct TeamRow {
    /// The run, by the row that names it.
    var id: String
    /// What its line says, on the first row of it held.
    var head: TeamLine?
    var hidden: Bool
    /// Which parts stay in view, or nil when the row shows whole.
    var shown: [Int]?
}

enum RunOpen { case closed, open, all }

/// The rows a transcript draws and how each one looks, from the core's folds and the ones the person opened.
enum ShownRows {
    /// How each row of a run shows, by the row. A run whose line is not held, as when rows after it were dropped,
    /// shows whole, as nothing could open it.
    static func teamRows(_ rows: [ChatMessage], _ how: (String) -> RunOpen) -> [String: TeamRow] {
        var found: [String: TeamRow] = [:]
        var at = 0
        while at < rows.count {
            guard let id = rows[at].team?.id else {
                at += 1
                continue
            }
            var end = at
            var line: TeamLine?
            while end < rows.count, rows[end].team?.id == id {
                line = rows[end].team?.line ?? line
                end += 1
            }
            let open = line != nil ? how(id) : .all
            var from = -1
            if open == .open, let line {
                from = (at..<end).first { rows[$0].id == line.from } ?? -1
            }
            for index in at..<end {
                let kept = rows[index].team?.shown ?? []
                found[rows[index].id] = TeamRow(
                    id: id, head: index == at ? line : nil,
                    hidden: open == .closed ? kept.isEmpty : index < from,
                    shown: open == .closed ? kept : nil)
            }
            at = end
        }
        return found
    }

    struct Fold {
        var summary: WorkFold?
        var hidden: Bool
        var kept: [Int]?
    }

    static func fold(_ row: ChatMessage, _ open: (String) -> Bool, _ run: TeamRow?) -> Fold {
        if let run { return Fold(summary: nil, hidden: run.hidden, kept: run.shown) }
        guard let fold = row.fold else { return Fold(summary: nil, hidden: false, kept: nil) }
        let summary = row.id == fold.id ? fold : nil
        if open(fold.id) { return Fold(summary: summary, hidden: false, kept: nil) }
        return Fold(summary: summary, hidden: fold.shown.isEmpty, kept: fold.shown)
    }

    /// The person's last message, of the rows held.
    static func lastAsked(_ rows: [ChatMessage]) -> ChatMessage? {
        rows.last { $0.role == .user && ($0.letters ?? []).isEmpty }
    }

    /// What it says, on one line.
    static func askedText(_ message: ChatMessage) -> String {
        message.parts.compactMap { part -> String? in
            if case .text(let text) = part { return text.text }
            return nil
        }
        .joined(separator: " ")
        .components(separatedBy: .whitespacesAndNewlines).filter { !$0.isEmpty }.joined(separator: " ")
    }
}
