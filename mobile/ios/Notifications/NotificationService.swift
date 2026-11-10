import UserNotifications

/// Opens the card a host sealed and shows it in place of the push's own text, which only says that
/// something needs the person. A push it cannot open keeps that text and opens the app.
final class NotificationService: UNNotificationServiceExtension {
  override func didReceive(
    _ request: UNNotificationRequest,
    withContentHandler contentHandler: @escaping (UNNotificationContent) -> Void
  ) {
    contentHandler(Self.content(for: request.content) ?? request.content)
  }

  static func content(for pushed: UNNotificationContent) -> UNNotificationContent? {
    guard let blob = pushed.userInfo["b"] as? String else { return nil }
    let keys = Keys()
    guard let phone = keys.phone else { return nil }
    switch Envelope.open(blob, phone: phone, keys: keys.all()) {
    case let .read(host, plaintext):
      guard let card = Card.parse(plaintext, host: host), card.kind != .clear else { return nil }
      return show(card, over: pushed)
    case let .unreadable(host):
      keys.unreadable(host: host)
      return nil
    case .noKey, .unknown:
      return nil
    }
  }

  private static func show(_ card: Card, over pushed: UNNotificationContent) -> UNNotificationContent? {
    guard let content = pushed.mutableCopy() as? UNMutableNotificationContent else { return nil }
    content.title = card.title
    content.body = card.detail.map { "\($0)\n\(card.body)" } ?? card.body
    content.threadIdentifier = card.thread
    content.categoryIdentifier = card.categoryIdentifier
    content.userInfo["sikemux"] = card.userInfo
    return content
  }
}
