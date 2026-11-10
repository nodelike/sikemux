import Foundation
import SikemuxCore
import UIKit
import UserNotifications

/// The cards hosts sent that show now: tidied as hosts settle what they asked, and answered or opened when tapped.
@MainActor
enum Cards {
    /// The permission requests each host's last view held, so a view that settles none needs no look at the cards.
    private static var pendingSeen: [String: Set<String>] = [:]
    private static var arrivals: [UUID: (host: String, kind: NotificationKind, call: @MainActor () -> Void)] = [:]

    private static func dismissShown(_ pick: @escaping ([[String: String]]) -> [String]) {
        Task {
            for tag in pick(await DeliveredCards.shown()) { DeliveredCards.remove(tag) }
        }
    }

    /// Removes the cards a host's current view shows are settled. Listing the shown cards is a trip into the
    /// system, so it happens only when a request went away, or on a connection's first view, which may settle cards
    /// that arrived while the phone was away.
    static func reconcile(host: String, view: DeviceView, fresh: Bool) {
        let pending = Set(view.attentions.map(\.id))
        let before = pendingSeen[host]
        pendingSeen[host] = pending
        if !fresh, let before, before.isSubset(of: pending) { return }
        dismissShown { shown in
            shown.filter { card in
                card["host"] == host && card["kind"] == NotificationKind.permission.rawValue
                    && card["request"].map { !pending.contains($0) } == true
            }.compactMap { $0["tag"] }
        }
    }

    /// Removes the cards about one agent, once the person is looking at it.
    static func dismiss(host: String, agent: String) {
        dismissShown { $0.filter { $0["host"] == host && $0["agent"] == agent }.compactMap { $0["tag"] } }
    }

    /// Removes every card from a host, as when it is forgotten.
    static func dismiss(host: String) {
        dismissShown { $0.filter { $0["host"] == host }.compactMap { $0["tag"] } }
    }

    /// Calls `call` while the returned watch is held, whenever a card of `kind` from `host` arrives with the app open.
    static func onArrive(host: String, kind: NotificationKind, _ call: @escaping @MainActor () -> Void) -> HostWatch {
        let id = UUID()
        arrivals[id] = (host, kind, call)
        return HostWatch { arrivals[id] = nil }
    }

    fileprivate static func arrived(_ info: [AnyHashable: Any]) {
        guard let card = info["sikemux"] as? [String: String] else { return }
        for arrival in arrivals.values where arrival.host == card["host"] && arrival.kind.rawValue == card["kind"] {
            arrival.call()
        }
    }

    /// Opens the chat a tapped card is about, and sends the answer when it was Allow or Reject.
    fileprivate static func tapped(_ info: [AnyHashable: Any], action: String, identifier: String) {
        guard let card = info["sikemux"] as? [String: String], let host = card["host"], let agent = card["agent"],
              let url = card["url"].flatMap(URL.init(string:))
        else { return }
        Router.shared.open(url)
        let option = action == "allow" ? card["allow"] : action == "reject" ? card["reject"] : nil
        guard let option, let request = card["request"] else { return }
        let tag = info["c"] as? String ?? identifier
        Task { await answer(tag: tag, host: host, agent: agent, request: request, option: option) }
    }

    /// Answers over the connection the app already holds to the host, or else over one of its own that keeps the
    /// phone online until the answer is in. A card whose answer failed stays.
    private static func answer(tag: String, host: String, agent: String, request: String, option: String) async {
        let quiet = QuietListener()
        do {
            if let held = Hub.shared.openConnections.first(where: { $0.core == host })?.connection {
                try await held.answerPermission(agentId: agent, requestId: request, optionId: option)
            } else {
                try await BackgroundTime.run("Answering from a notification") {
                    try await Identity.shared.whileJoining { device in
                        let connection = try await device.connect(core: host, listener: quiet)
                        defer { connection.close() }
                        try await connection.answerPermission(agentId: agent, requestId: request, optionId: option)
                    }
                }
            }
        } catch MobileError.Refused(_, let kind) where kind == "notPending" {
        } catch {
            print("sikemux: could not answer from the notification: \(error)")
            return
        }
        DeliveredCards.remove(tag)
    }
}

private final class QuietListener: CoreListener, @unchecked Sendable {
    func events(events: [CoreEvent]) throws {}
    func closed() throws {}
}

/// Hears the system about notifications: the token, a clear that takes a card away, a card arriving while the app
/// is open, and a card tapped.
final class AppDelegate: NSObject, UIApplicationDelegate, UNUserNotificationCenterDelegate {
    func application(_ application: UIApplication, didFinishLaunchingWithOptions _: [UIApplication.LaunchOptionsKey: Any]? = nil) -> Bool {
        UNUserNotificationCenter.current().delegate = self
        _ = Notify.shared
        return true
    }

    func application(_: UIApplication, didRegisterForRemoteNotificationsWithDeviceToken token: Data) {
        Notify.shared.registered(.success(token))
    }

    func application(_: UIApplication, didFailToRegisterForRemoteNotificationsWithError error: Error) {
        Notify.shared.registered(.failure(error))
    }

    /// A host's clear arrives as a background push that wakes the app.
    func application(
        _: UIApplication, didReceiveRemoteNotification info: [AnyHashable: Any],
        fetchCompletionHandler done: @escaping (UIBackgroundFetchResult) -> Void
    ) {
        guard info["t"] as? String == "clear", let tag = info["c"] as? String else { return done(.noData) }
        DeliveredCards.remove(tag) { done(.newData) }
    }

    /// A card that arrives while the app is open shows over it.
    func userNotificationCenter(
        _: UNUserNotificationCenter, willPresent notification: UNNotification
    ) async -> UNNotificationPresentationOptions {
        let info = notification.request.content.userInfo
        await MainActor.run { Cards.arrived(info) }
        return [.banner, .list, .sound]
    }

    func userNotificationCenter(_: UNUserNotificationCenter, didReceive response: UNNotificationResponse) async {
        let request = response.notification.request
        let info = request.content.userInfo
        let action = response.actionIdentifier
        await MainActor.run { Cards.tapped(info, action: action, identifier: request.identifier) }
    }
}
