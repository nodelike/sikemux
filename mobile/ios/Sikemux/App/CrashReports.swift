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
    nonisolated private static let allowedKey = "sikemux.crashReports"
    private static let dsn = (Bundle.main.object(forInfoDictionaryKey: "SikemuxSentryDSN") as? String) ?? ""

    var on: Bool {
        didSet {
            UserDefaults.standard.set(on, forKey: Self.allowedKey)
            if on { start() } else { stop() }
        }
    }

    @ObservationIgnored private var account: String?
    @ObservationIgnored private var starting = false

    private init() {
        on = Self.allowed()
    }

    /// Starts the SDK off the main thread, since starting it took about 90 ms of launch there; the SDK queues the
    /// part that must run on the main thread, its crash handler, for after the first frame.
    func start() {
        guard on else { return Self.forget() }
        guard !Self.dsn.isEmpty, !SentrySDK.isEnabled, !starting else { return }
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
            options.urlSession = OnlyWhileShared.session()
            options.releaseName = release
            options.environment = "production"
            options.sendDefaultPii = false
            options.enableNetworkBreadcrumbs = false
            options.enableCaptureFailedRequests = false
            options.enableAutoPerformanceTracing = false
            options.enableMemoryIntrospection = false
            options.beforeSend = { event in CrashReports.withoutRegisters(event) }
            options.initialScope = { scope in
                scope.setUser(CrashReports.anonymous())
                scope.setTag(value: release.contains("-nightly.") ? "nightly" : "stable", key: "channel")
                return scope
            }
        }
    }

    /// Runs after the SDK's own main-thread setup, which it queued first.
    private func started() {
        starting = false
        if on { name() } else { stop() }
    }

    private func stop() {
        SentrySDK.close()
        Self.forget()
    }

    nonisolated fileprivate static func allowed() -> Bool {
        UserDefaults.standard.object(forKey: allowedKey) as? Bool ?? true
    }

    /// Drops what the SDK keeps on the phone until it can send it.
    nonisolated private static func forget() {
        let caches = FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask)[0]
        for name in ["io.sentry", "SentryCrash"] {
            try? FileManager.default.removeItem(at: caches.appendingPathComponent(name))
        }
    }

    /// Names the person by the same one-way hash of their account the Mac's usage ping sends.
    func follow(account: String?) {
        self.account = account
        name()
    }

    private func name() {
        guard SentrySDK.isEnabled else { return }
        let user = CrashReports.anonymous()
        user.userId = account.map { SHA256.hash(data: Data("sikemux-usage|\($0)".utf8)).hex }
        SentrySDK.setUser(user)
    }

    /// What the registers held may be the person's own text.
    nonisolated private static func withoutRegisters(_ event: Sentry.Event) -> Sentry.Event {
        for exception in event.exceptions ?? [] { exception.stacktrace?.registers = [:] }
        for thread in event.threads ?? [] { thread.stacktrace?.registers = [:] }
        return event
    }

    /// Sentry works out a city from the sender's address unless the event names one, so every event names none.
    nonisolated private static func anonymous() -> User {
        let user = User()
        user.ipAddress = "0.0.0.0"
        return user
    }
}

/// Refuses each request the SDK makes once sharing is off, since the SDK sends what it queued earlier as it closes.
private final class OnlyWhileShared: URLProtocol {
    static func session() -> URLSession {
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [OnlyWhileShared.self] + (configuration.protocolClasses ?? [])
        return URLSession(configuration: configuration)
    }

    override class func canInit(with request: URLRequest) -> Bool { !CrashReports.allowed() }

    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }

    override func startLoading() {
        client?.urlProtocol(self, didFailWithError: URLError(.cancelled))
    }

    override func stopLoading() {}
}
