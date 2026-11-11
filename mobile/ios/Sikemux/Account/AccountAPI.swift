import Foundation
import UIKit

/// Why a request to the accounts server failed, in words for the person.
struct ApiProblem: LocalizedError {
    /// The server's status, or nil when it could not be reached at all.
    let status: Int?
    let code: ErrorCode?
    let message: String
    /// Only with reverify_required: how recently the person must have proved who they are.
    var reverifyWithinMinutes: Int?

    var errorDescription: String? { message }

    /// Nothing changed on the server: it was out of reach, or failed on its side.
    var unreachable: Bool { status == nil || status! >= 500 }
}

extension WireDevice: Identifiable {
    var id: DeviceKey { key }
}

/// The accounts server's routes, signed in as the person.
@MainActor
enum AccountAPI {
    static let offline = "Can't reach Sikemux. Check the phone is online."

    static func send<Answer: Decodable>(
        _ method: String,
        _ path: String,
        body: (any Encodable)? = nil,
        as: Answer.Type = Answer.self
    ) async throws -> Answer {
        let data = try await request(method, path, body: body)
        return try JSONDecoder().decode(Answer.self, from: data)
    }

    static func send(_ method: String, _ path: String, body: (any Encodable)? = nil) async throws {
        _ = try await request(method, path, body: body)
    }

    /// Sends a file's bytes, read from disk as they go, with `headers` beside them.
    static func upload<Answer: Decodable>(
        _ path: String, file: URL, headers: [String: String], as: Answer.Type = Answer.self
    ) async throws -> Answer {
        var request = try await signedIn("POST", path)
        for (name, value) in headers { request.setValue(value, forHTTPHeaderField: name) }
        let (data, response) = try await reach { try await URLSession.shared.upload(for: request, fromFile: file) }
        try check(response, data)
        return try JSONDecoder().decode(Answer.self, from: data)
    }

    /// Brings a file's bytes down to a temporary file, which the caller moves where it keeps it.
    static func download(_ path: String) async throws -> URL {
        let (file, response) = try await reach {
            try await URLSession.shared.download(for: try await signedIn("GET", path))
        }
        try check(response, Data())
        return file
    }

    private static func request(_ method: String, _ path: String, body: (any Encodable)?) async throws -> Data {
        var request = try await signedIn(method, path)
        if let body {
            request.setValue("application/json", forHTTPHeaderField: "Content-Type")
            request.httpBody = try JSONEncoder().encode(body)
        }
        let (data, response) = try await reach { try await URLSession.shared.data(for: request) }
        try check(response, data)
        return (response as? HTTPURLResponse)?.statusCode == 204 ? Data("{}".utf8) : data
    }

    private static func signedIn(_ method: String, _ path: String) async throws -> URLRequest {
        var request = URLRequest(url: URL(string: Config.apiUrl.absoluteString + path)!, timeoutInterval: 30)
        request.httpMethod = method
        do {
            request.setValue("Bearer \(try await Account.shared.token())", forHTTPHeaderField: "Authorization")
        } catch {
            throw ApiProblem(status: 401, code: .unauthorized, message: "Sign in again.")
        }
        return request
    }

    private static func reach<T>(_ send: () async throws -> T) async throws -> T {
        do { return try await send() } catch { throw ApiProblem(status: nil, code: nil, message: offline) }
    }

    private static func check(_ response: URLResponse, _ data: Data) throws {
        let status = (response as? HTTPURLResponse)?.statusCode ?? 0
        guard !(200..<300).contains(status) else { return }
        let detail = (try? JSONDecoder().decode(ApiError.self, from: data))?.error
        throw ApiProblem(
            status: status,
            code: detail?.code,
            message: detail?.message ?? "The accounts server answered \(status).",
            reverifyWithinMinutes: detail?.reverifyWithinMinutes
        )
    }
}

/// The account was deleted and can still be restored, until `deletesAt`.
struct AccountDeleting: Error {
    let deletesAt: String
}

/// What this phone does on the account: join it, leave it, and read and change what is on it.
@MainActor
enum AccountCalls {
    /// A one-time value for this phone to sign, valid for two minutes.
    static func challenge() async throws -> String {
        try await AccountAPI.send("POST", "/v1/devices/challenge", as: Challenge.self).nonce
    }

    /// Adds this phone to the account, proving it holds its key; again only updates its name. A deleted account
    /// that can still be restored throws AccountDeleting.
    static func registerPhone(userId: String) async throws {
        do {
            let nonce = try await challenge()
            let me = try Identity.shared.deviceIdentity()
            let registration = DeviceRegistration(
                key: me.id(),
                role: .client,
                name: String(UIDevice.current.name.prefix(64)),
                platform: .ios,
                nonce: nonce,
                signature: try me.signRegistration(nonce: nonce, userId: userId)
            )
            _ = try await AccountAPI.send("POST", "/v1/devices", body: registration, as: WireDevice.self)
        } catch let problem as ApiProblem where problem.status == 401 {
            let account = try? await standing()
            if account?.status == .deleting, let deletesAt = account?.deletesAt {
                throw AccountDeleting(deletesAt: deletesAt)
            }
            throw problem
        }
    }

    /// Takes this phone off the account; a phone the account no longer has is already off it.
    static func removePhone() async throws {
        let key = try Identity.shared.deviceIdentity().id()
        do {
            try await AccountAPI.send("DELETE", "/v1/devices/\(key)")
        } catch let problem as ApiProblem where problem.status == 404 {}
    }

    static func hosts() async throws -> [WireDevice] {
        try await AccountAPI.send("GET", "/v1/devices?role=host", as: DeviceList.self).devices
    }

    static func device(_ key: String) async throws -> WireDevice? {
        try await AccountAPI.send("GET", "/v1/devices", as: DeviceList.self).devices.first { $0.key == key }
    }

    static func removeDevice(_ key: String) async throws {
        try await AccountAPI.send("DELETE", "/v1/devices/\(key)")
    }

    static func joinTicket(host: String) async throws -> JoinTicket {
        let key = try Identity.shared.deviceIdentity().id()
        return try await AccountAPI.send(
            "POST", "/v1/devices/\(key)/join", body: JoinRequest(host: host), as: JoinTicket.self)
    }

    static func standing() async throws -> WireAccount {
        try await AccountAPI.send("GET", "/v1/account", as: WireAccount.self)
    }

    static func restore() async throws -> WireAccount {
        try await AccountAPI.send("POST", "/v1/account/restore", as: WireAccount.self)
    }

    /// Deletes the account and every device on it, with Apple's code for the server to revoke when it signs in
    /// with Apple.
    static func delete(appleAuthorizationCode: String?) async throws {
        try await AccountAPI.send(
            "DELETE", "/v1/account", body: AccountDeletionRequest(appleAuthorizationCode: appleAuthorizationCode))
    }
}
