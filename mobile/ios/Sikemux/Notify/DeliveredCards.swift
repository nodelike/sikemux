import UserNotifications

/// What the accounts server's push carries besides the alert: the collapse id that names a card, and whether it
/// takes a card away.
enum Push {
    static func collapseId(_ info: [AnyHashable: Any]) -> String? { info["c"] as? String }
    static func clears(_ info: [AnyHashable: Any]) -> Bool { info["t"] as? String == "clear" }
}

/// The cards hosts sent that are showing now, each named by its push's collapse id.
enum DeliveredCards {
    struct Shown {
        let tag: String
        let card: Card
    }

    private static func tag(of notification: UNNotification) -> String {
        Push.collapseId(notification.request.content.userInfo) ?? notification.request.identifier
    }

    static func shown() async -> [Shown] {
        await UNUserNotificationCenter.current().deliveredNotifications().compactMap { notification in
            Card.kept(in: notification.request.content.userInfo).map { Shown(tag: tag(of: notification), card: $0) }
        }
    }

    static func remove(_ tags: Set<String>) async {
        guard !tags.isEmpty else { return }
        let center = UNUserNotificationCenter.current()
        let matching = await center.deliveredNotifications().filter { tags.contains(tag(of: $0)) }
        center.removeDeliveredNotifications(withIdentifiers: matching.map(\.request.identifier))
    }
}
