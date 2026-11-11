import Foundation

typealias Card = NotificationCard

extension NotificationCard {
  static let version = 1
  /// The category whose cards carry Reject and Allow.
  static let answerCategory = NotificationCategory.permission.rawValue

  var answerable: Bool {
    kind == .permission && requestId != nil && allowOptionId != nil && rejectOptionId != nil
  }

  var categoryIdentifier: String {
    answerable && category == .permission ? Card.answerCategory : ""
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
