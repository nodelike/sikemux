import Foundation
import Sentry

/// Sends the helper's crashes, hangs and sessions to Sentry. Only builds the release workflow
/// publishes carry its address in their Info.plist, and they send only while the person shares
/// usage data, has been shown that choice, and `DO_NOT_TRACK` is unset. Paths are sent with the
/// home folder written `~`.
/// The SDK starts its own sessions when an app becomes active, which this accessory never does,
/// so each run starts and ends its session here.
enum CrashReports {
    static func start() {
        let info = Bundle.main.infoDictionary ?? [:]
        guard let dsn = info["SikemuxSentryDSN"] as? String, !dsn.isEmpty else { return }
        guard allowed() else { return forget() }
        let version = info["CFBundleShortVersionString"] as? String ?? "0"
        SentrySDK.start { options in
            options.dsn = dsn
            options.cacheDirectoryPath = waiting.path
            options.releaseName = "sikemux@\(version)"
            options.environment = info["SikemuxSentryEnvironment"] as? String ?? "dev"
            options.sendDefaultPii = false
            options.maxBreadcrumbs = 50
            options.enableAutoSessionTracking = false
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
        SentrySDK.startSession()
    }

    /// Ends this run's session as one that exited well, and sends what is waiting.
    static func stop() {
        guard SentrySDK.isEnabled else { return }
        guard allowed() else { return forget() }
        SentrySDK.endSession()
        SentrySDK.flush(timeout: 2)
    }

    /// Where reports wait to be sent, the helper's own so it can drop them.
    private static let waiting = FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask)[0]
        .appendingPathComponent("com.nodelike.sikemux/notch-crash-reports")

    private static func forget() {
        try? FileManager.default.removeItem(at: waiting)
    }

    private static func allowed() -> Bool {
        let environment = ProcessInfo.processInfo.environment
        let optedOut = ["DO_NOT_TRACK", "CI"].contains { name in
            let value = (environment[name] ?? "").trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
            return !["", "0", "false", "no", "off"].contains(value)
        }
        let preferences = FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent("Library/Application Support/com.nodelike.sikemux/preferences.json")
        guard !optedOut,
              let data = try? Data(contentsOf: preferences),
              let saved = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any]
        else { return false }
        return (saved["shareUsageData"] as? Bool ?? true) && (saved["onboardingComplete"] as? Bool ?? false)
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
