import CryptoKit
import Foundation
import Observation
import Sentry

/// Sends crashes and errors to Sentry while the person allows it. Only builds the release workflow made carry the
/// address to send to, so a dev build never sends.
@MainActor
@Observable
final class CrashReports {
    static let shared = CrashReports()
    private static let allowedKey = "sikemux.crashReports"
    private static let dsn = (Bundle.main.object(forInfoDictionaryKey: "SikemuxSentryDSN") as? String) ?? ""

    var on: Bool {
        didSet {
            UserDefaults.standard.set(on, forKey: Self.allowedKey)
            if on { start() } else { SentrySDK.close() }
        }
    }

    @ObservationIgnored private var account: String?
    @ObservationIgnored private var starting = false

    private init() {
        on = UserDefaults.standard.object(forKey: Self.allowedKey) as? Bool ?? true
    }

    /// Starts the SDK off the main thread, since starting it took about 90 ms of launch there; the SDK queues the
    /// part that must run on the main thread, its crash handler, for after the first frame.
    func start() {
        guard on, !Self.dsn.isEmpty, !SentrySDK.isEnabled, !starting else { return }
        starting = true
        let release = "sikemux-mobile@\(Versions.fromBuild(Config.build) ?? Config.version)+\(Config.build)"
        let dsn = Self.dsn
        Task.detached(priority: .utility) {
            Self.startSDK(dsn: dsn, release: release)
            await self.started()
        }
    }

    nonisolated private static func startSDK(dsn: String, release: String) {
        SentrySDK.start { options in
            options.dsn = dsn
            options.releaseName = release
            options.environment = "production"
            options.sendDefaultPii = false
            options.enableNetworkBreadcrumbs = false
            options.enableCaptureFailedRequests = false
            options.enableAutoPerformanceTracing = false
            options.initialScope = { scope in
                scope.setTag(value: release.contains("-nightly.") ? "nightly" : "stable", key: "channel")
                return scope
            }
        }
    }

    /// Runs after the SDK's own main-thread setup, which it queued first.
    private func started() {
        starting = false
        if on { name() } else { SentrySDK.close() }
    }

    /// Names the person by the same one-way hash of their account the Mac's usage ping sends.
    func follow(account: String?) {
        self.account = account
        name()
    }

    private func name() {
        guard SentrySDK.isEnabled else { return }
        SentrySDK.setUser(
            account.map { id in
                let user = User()
                user.userId = SHA256.hash(data: Data("sikemux-usage|\(id)".utf8)).hex
                return user
            })
    }
}
