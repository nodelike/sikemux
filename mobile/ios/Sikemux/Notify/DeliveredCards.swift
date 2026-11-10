import UserNotifications

/// The cards hosts sent that are showing now, each named by its push's collapse id.
enum DeliveredCards {
  static func tag(of notification: UNNotification) -> String {
    notification.request.content.userInfo["c"] as? String ?? notification.request.identifier
  }

  static func shown() async -> [[String: String]] {
    await UNUserNotificationCenter.current().deliveredNotifications().compactMap { notification in
      guard var card = notification.request.content.userInfo["sikemux"] as? [String: String] else { return nil }
      card["tag"] = tag(of: notification)
      card.removeValue(forKey: "hostName")
      card.removeValue(forKey: "url")
      card.removeValue(forKey: "allow")
      card.removeValue(forKey: "reject")
      return card
    }
  }

  static func remove(_ tag: String, then done: @escaping () -> Void = {}) {
    let center = UNUserNotificationCenter.current()
    center.getDeliveredNotifications { delivered in
      let matching = delivered.filter { self.tag(of: $0) == tag }.map(\.request.identifier)
      center.removeDeliveredNotifications(withIdentifiers: matching)
      done()
    }
  }
}
