import Foundation
import SikemuxCore
import UIKit
import UserNotifications

/// The buttons on a card, by the identifiers iOS answers with.
enum CardAction: String {
    case allow, reject
}

/// The cards hosts sent that show now: tidied as hosts settle what they asked, and answered or opened when tapped.
@MainActor
enum Cards {
    /// The permission requests each host's last view held, so a view that settles none needs no look at the cards.
    private static var pendingSeen: [String: Set<String>] = [:]
    private static var arrivals: [UUID: (host: String, kind: NotificationKind, call: @MainActor () -> Void)] = [:]

    private static func dismissShown(_ settled: @escaping (Card) -> Bool) {
        Task {
            await DeliveredCards.remove(Set(await DeliveredCards.shown().filter { settled($0.card) }.map(\.tag)))
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
        dismissShown { card in
            card.hostKey == host && card.kind == .permission && card.requestId.map { !pending.contains($0) } == true
        }
    }

    /// Removes the cards about one agent, once the person is looking at it.
    static func dismiss(host: String, agent: String) {
        dismissShown { $0.hostKey == host && $0.agentId == agent }
    }

    /// Removes every card from a host, as when it is forgotten.
    static func dismiss(host: String) {
        dismissShown { $0.hostKey == host }
    }

    /// Calls `call` while the returned watch is held, whenever a card of `kind` from `host` arrives with the app open.
    static func onArrive(host: String, kind: NotificationKind, _ call: @escaping @MainActor () -> Void) -> HostWatch {
        let id = UUID()
        arrivals[id] = (host, kind, call)
        return HostWatch { arrivals[id] = nil }
    }

    fileprivate static func arrived(_ card: Card) {
        for arrival in arrivals.values where arrival.host == card.hostKey && arrival.kind == card.kind {
            arrival.call()
        }
    }

    /// Opens the chat a tapped card is about, and sends the answer when it was Allow or Reject, an answer to a call,
    /// or the person's own words to one.
    fileprivate static func tapped(_ card: Card, action identifier: String, typed: String?, tag: String) {
        if let url = URL(string: card.url) { Router.shared.open(url) }
        if let call = card.callId, let answer = answer(call, identifier, typed) {
            Task { await answerCall(tag: tag, card: card, answer) }
            return
        }
        let action = CardAction(rawValue: identifier)
        let option =
            switch action {
            case .allow: card.allowOptionId
            case .reject: card.rejectOptionId
            case nil: String?.none
            }
        guard let option, let request = card.requestId else { return }
        Task { await answer(tag: tag, card: card, request: request, option: option) }
    }

    private static func answer(_ call: String, _ identifier: String, _ typed: String?) -> AnswerCallParams? {
        if identifier.hasPrefix(CallPush.option) {
            return AnswerCallParams(id: call, option: String(identifier.dropFirst(CallPush.option.count)))
        }
        let words = typed?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        return identifier == CallPush.words && !words.isEmpty ? AnswerCallParams(id: call, words: words) : nil
    }

    /// Answers a call as `answer` does a permission. The button opened the app on the asker's chat; if the answer
    /// does not go, the call's card there says so and why, and the call waits to be answered there.
    private static func answerCall(tag: String, card: Card, _ params: AnswerCallParams) async {
        do {
            if let held = Hub.shared.openConnections.first(where: { $0.core == card.hostKey })?.connection {
                _ = try await held.call(PhoneCall.AnswerCall.self, params)
            } else {
                try await BackgroundTime.run("Answering from a notification") {
                    try await Identity.shared.whileJoining { device in
                        let connection = try await device.connect(core: card.hostKey, listener: QuietListener())
                        defer { connection.close() }
                        _ = try await connection.call(PhoneCall.AnswerCall.self, params)
                    }
                }
            }
        } catch {
            HostCalls.of(card.hostKey).answerFailed(params.id, problem(error))
            return
        }
        await DeliveredCards.remove([tag])
    }

    /// Answers over the connection the app already holds to the host, or else over one of its own that keeps the
    /// phone online until the answer is in. iOS has taken the card away by the time the app opens; a failed answer
    /// leaves the request in its chat.
    private static func answer(tag: String, card: Card, request: String, option: String) async {
        do {
            if let held = Hub.shared.openConnections.first(where: { $0.core == card.hostKey })?.connection {
                try await held.answerPermission(agentId: card.agentId, requestId: request, optionId: option)
            } else {
                try await BackgroundTime.run("Answering from a notification") {
                    try await Identity.shared.whileJoining { device in
                        let connection = try await device.connect(core: card.hostKey, listener: QuietListener())
                        defer { connection.close() }
                        try await connection.answerPermission(
                            agentId: card.agentId, requestId: request, optionId: option)
                    }
                }
            }
        } catch let refused as MobileError where refused.refusal == .notPending {
        } catch {
            print("sikemux: could not answer from the notification: \(error)")
            return
        }
        await DeliveredCards.remove([tag])
    }
}

/// A connection made only to answer a card listens to nothing.
private final class QuietListener: CoreListener {
    func events(events: [CoreEvent]) throws {}
    func closed() throws {}
}

/// Hears the system about notifications: the token, a clear that takes a card away, a card arriving while the app
/// is open, and a card tapped.
@MainActor
final class AppDelegate: NSObject, UIApplicationDelegate {
    func application(_: UIApplication, didFinishLaunchingWithOptions _: [UIApplication.LaunchOptionsKey: Any]? = nil)
        -> Bool
    {
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
    func application(_: UIApplication, didReceiveRemoteNotification info: [AnyHashable: Any]) async
        -> UIBackgroundFetchResult
    {
        guard Push.clears(info), let tag = Push.collapseId(info) else { return .noData }
        await DeliveredCards.remove([tag])
        return .newData
    }
}

/// iOS asks these on the main thread, which the conformance checks as they run.
extension AppDelegate: @preconcurrency UNUserNotificationCenterDelegate {
    /// A card that arrives while the app is open shows over it.
    func userNotificationCenter(
        _: UNUserNotificationCenter, willPresent notification: UNNotification,
        withCompletionHandler show: @escaping (UNNotificationPresentationOptions) -> Void
    ) {
        if let card = Card.kept(in: notification.request.content.userInfo) { Cards.arrived(card) }
        show([.banner, .list, .sound])
    }

    /// A card tapped, or one of its buttons.
    func userNotificationCenter(
        _: UNUserNotificationCenter, didReceive response: UNNotificationResponse,
        withCompletionHandler done: @escaping () -> Void
    ) {
        let request = response.notification.request
        if let card = Card.kept(in: request.content.userInfo) {
            let tag = Push.collapseId(request.content.userInfo) ?? request.identifier
            Cards.tapped(
                card, action: response.actionIdentifier,
                typed: (response as? UNTextInputNotificationResponse)?.userText, tag: tag)
        }
        done()
    }
}
