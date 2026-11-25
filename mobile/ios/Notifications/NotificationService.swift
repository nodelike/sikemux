import UserNotifications

/// Opens the card a host sealed and shows it in place of the push's own text, which only says that
/// something needs the person. A push it cannot open keeps that text and opens the app.
final class NotificationService: UNNotificationServiceExtension {
    override func didReceive(
        _ request: UNNotificationRequest,
        withContentHandler contentHandler: @escaping (UNNotificationContent) -> Void
    ) {
        switch Self.resolve(request.content) {
        case .clear(let collapseId):
            nonisolated(unsafe) let show = contentHandler
            Task {
                await Self.cancel(collapseId: collapseId)
                show(UNMutableNotificationContent())
            }
        case .show(let shown):
            guard let category = Card.kept(in: shown.userInfo)?.callCategory else { return contentHandler(shown) }
            // UserNotifications' types are not Sendable; these are handed once to the task that delivers the card.
            nonisolated(unsafe) let delivery = (content: shown, category: category, show: contentHandler)
            Task {
                await Self.register(delivery.category)
                delivery.show(delivery.content)
            }
        case .fallback:
            contentHandler(request.content)
        }
    }

    private enum Resolution {
        case show(UNNotificationContent)
        case clear(String)
        case fallback
    }

    private static func cancel(collapseId: String) async {
        let center = UNUserNotificationCenter.current()
        let matching = await center.deliveredNotifications().filter { notification in
            let tag = (notification.request.content.userInfo["c"] as? String) ?? notification.request.identifier
            return tag == collapseId
        }
        if !matching.isEmpty {
            center.removeDeliveredNotifications(withIdentifiers: matching.map(\.request.identifier))
        }
    }

    /// Adds a call's buttons to the categories already set, and waits, a second at most, until iOS has them, so the
    /// card shows with its buttons.
    private static func register(_ category: UNNotificationCategory) async {
        let center = UNUserNotificationCenter.current()
        center.setNotificationCategories(CallPush.categories(await center.notificationCategories(), adding: category))
        for _ in 0..<20 {
            if await center.notificationCategories().contains(where: { $0.identifier == category.identifier }) {
                return
            }
            try? await Task.sleep(for: .milliseconds(50))
        }
    }

    static func content(for pushed: UNNotificationContent) -> UNNotificationContent? {
        switch resolve(pushed) {
        case .show(let content): return content
        case .clear: return UNMutableNotificationContent()
        case .fallback: return nil
        }
    }

    private static func resolve(_ pushed: UNNotificationContent) -> Resolution {
        if pushed.userInfo["t"] as? String == "clear", let collapseId = pushed.userInfo["c"] as? String {
            return .clear(collapseId)
        }
        guard let blob = pushed.userInfo["b"] as? String else { return .fallback }
        let keys = Keys()
        guard let phone = keys.phone else { return .fallback }
        switch Envelope.open(blob, phone: phone, keys: keys.all()) {
        case .read(let host, let plaintext):
            guard let card = Card.parse(plaintext, host: host) else { return .fallback }
            GlanceStore.heard(card)
            if card.kind == .clear { return .clear(card.collapseId) }
            return show(card, over: pushed).map(Resolution.show) ?? .fallback
        case .unreadable(let host):
            keys.unreadable(host: host)
            return .fallback
        case .noKey, .unknown:
            return .fallback
        }
    }

    private static func show(_ card: Card, over pushed: UNNotificationContent) -> UNNotificationContent? {
        guard let content = pushed.mutableCopy() as? UNMutableNotificationContent else { return nil }
        content.title = card.title
        content.body = card.detail.map { "\($0)\n\(card.body)" } ?? card.body
        content.threadIdentifier = card.thread
        content.categoryIdentifier = card.categoryIdentifier
        do {
            try card.keep(in: &content.userInfo)
        } catch {
            print("sikemux: could not keep the card on its notification: \(error)")
        }
        return content
    }
}
