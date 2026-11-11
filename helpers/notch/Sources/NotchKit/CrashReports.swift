import Foundation
import Sentry

/// Sends the helper's crashes, hangs and sessions to Sentry. Only builds the release workflow
/// publishes carry its address in their Info.plist, and they send only while the person shares
/// usage data and `DO_NOT_TRACK` is unset. Paths are sent with the home folder written `~`.
enum CrashReports {
    static func start() {
        let info = Bundle.main.infoDictionary ?? [:]
        guard let dsn = info["SikemuxSentryDSN"] as? String, !dsn.isEmpty, allowed() else { return }
        let version = info["CFBundleShortVersionString"] as? String ?? "0"
        SentrySDK.start { options in
            options.dsn = dsn
            options.releaseName = "sikemux@\(version)"
            options.environment = info["SikemuxSentryEnvironment"] as? String ?? "dev"
            options.sendDefaultPii = false
            options.maxBreadcrumbs = 50
            options.beforeSend = { event in
                guard allowed() else { return nil }
                event.serverName = nil
                let user = event.user ?? User()
                user.ipAddress = "0.0.0.0"
                event.user = user
                event.tags = (event.tags ?? [:]).merging(
                    ["process": "notch", "channel": version.contains("-nightly.") ? "nightly" : "stable"]
                ) { kept, _ in kept }
                return withoutHome(event)
            }
        }
    }

    private static func allowed() -> Bool {
        if let value = ProcessInfo.processInfo.environment["DO_NOT_TRACK"], !value.isEmpty, value != "0" {
            return false
        }
        let preferences = FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent("Library/Application Support/com.nodelike.sikemux/preferences.json")
        guard let data = try? Data(contentsOf: preferences) else { return true }
        let saved = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any]
        return saved?["shareUsageData"] as? Bool ?? (saved != nil)
    }

    private static func withoutHome(_ event: Sentry.Event) -> Sentry.Event {
        let home = NSHomeDirectory()
        guard home.count > 1 else { return event }
        let tilde = { (text: String?) in text?.replacingOccurrences(of: home, with: "~") }
        event.debugMeta?.forEach { image in image.codeFile = tilde(image.codeFile) }
        event.exceptions?.forEach { exception in exception.value = tilde(exception.value) ?? "" }
        event.breadcrumbs?.forEach { breadcrumb in breadcrumb.message = tilde(breadcrumb.message) }
        event.message = event.message.map { SentryMessage(formatted: tilde($0.formatted) ?? "") }
        return event
    }
}
