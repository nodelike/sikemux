import Foundation
import UserNotifications

typealias Card = NotificationCard

extension NotificationCard {
    static let version = 1
    /// The category whose cards carry Reject and Allow.
    static let answerCategory = NotificationCategory.permission.rawValue

    var answerable: Bool {
        kind == .permission && requestId != nil && allowOptionId != nil && rejectOptionId != nil
    }

    var categoryIdentifier: String {
        if let call = callCategory { return call.identifier }
        return answerable && category == .permission ? Card.answerCategory : ""
    }

    /// The buttons of a card for a call of taste: each answer it offers, then a field for the person's own words. A
    /// production call's card offers none, so it opens the app. Each call's card has its category, named after when
    /// it came so the oldest can go.
    var callCategory: UNNotificationCategory? {
        guard let callId, let options, !options.isEmpty else { return nil }
        let picks = options.map {
            UNNotificationAction(identifier: CallPush.option + $0.id, title: $0.label, options: [.foreground])
        }
        let words = UNTextInputNotificationAction(
            identifier: CallPush.words, title: CallPush.somethingElse, options: [.foreground],
            textInputButtonTitle: CallPush.reply, textInputPlaceholder: "")
        return UNNotificationCategory(
            identifier: "\(CallPush.category)\(at):\(callId)", actions: picks + [words], intentIdentifiers: [])
    }

    /// Nil when the host sealed a version this app does not know, or something that is not a card from `host`.
    static func parse(_ plaintext: String, host: String) -> Card? {
        do {
            let card = try JSONDecoder().decode(Card.self, from: Data(plaintext.utf8))
            return card.v == version && card.hostKey == host ? card : nil
        } catch {
            print("sikemux: a host sent a card this app cannot read: \(error)")
            return nil
        }
    }

    private static let infoKey = "sikemux"

    /// The card kept on the notification that shows it, for the app to answer it, open its chat, or take it away.
    func keep(in info: inout [AnyHashable: Any]) throws {
        info[Self.infoKey] = String(decoding: try JSONEncoder().encode(self), as: UTF8.self)
    }

    /// The card a shown notification keeps; nil on one the extension could not open, which only opens the app.
    static func kept(in info: [AnyHashable: Any]) -> Card? {
        guard let json = info[infoKey] as? String else { return nil }
        do {
            return try JSONDecoder().decode(Card.self, from: Data(json.utf8))
        } catch {
            print("sikemux: a shown notification keeps a card this app cannot read: \(error)")
            return nil
        }
    }
}

/// What a call's card says on its buttons, and the names iOS answers them by. The rest of the calls' words are in the
/// app's `CallWords`; these sit here because the notification extension draws them.
enum CallPush {
    static let somethingElse = String(localized: "Something else…")
    static let reply = String(localized: "Reply")
    static let category = "call:"
    static let option = "option:"
    static let words = "words"
    /// How many calls' cards keep their buttons; an older card still opens the app.
    static let kept = 24

    /// The categories to register once `adding` arrives: every one that is not a call's, the newest calls', and it.
    static func categories(_ held: Set<UNNotificationCategory>, adding: UNNotificationCategory?) -> Set<
        UNNotificationCategory
    > {
        let calls = held.filter { $0.identifier.hasPrefix(category) && $0.identifier != adding?.identifier }
        let others = held.subtracting(calls).filter { $0.identifier != adding?.identifier }
        let newest = calls.sorted { at($0) > at($1) }.prefix(adding == nil ? kept : kept - 1)
        return others.union(newest).union(adding.map { [$0] } ?? [])
    }

    private static func at(_ held: UNNotificationCategory) -> Int {
        Int(held.identifier.dropFirst(category.count).prefix { $0 != ":" }) ?? 0
    }
}
