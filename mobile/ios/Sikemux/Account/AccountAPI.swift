import Foundation
import UIKit

/// Why a request to the accounts server failed, in words for the person.
struct ApiProblem: LocalizedError {
    /// The server's status, or nil when it could not be reached at all.
    let status: Int?
    let code: String?
    let message: String
    /// Only with reverify_required: how recently the person must have proved who they are.
    var reverifyWithinMinutes: Int?

    var errorDescription: String? { message }

    /// Nothing changed on the server: it was out of reach, or failed on its side.
    var unreachable: Bool { status == nil || status! >= 500 }
}

/// A device or host on the account.
struct AccountDevice: Codable, Hashable, Identifiable {
    let key: String
    let role: String
    let name: String
    let platform: String
    let channel: String?
    let createdAt: String
    let lastSeenAt: String?
    var id: String { key }
}

/// The server's signed word that this phone and a host are on the same account.
struct JoinTicket: Codable {
    let v: Int
    let keyId: String
    let account: String
    let host: String
    let phone: String
    let issuedAt: Int
    let expiresAt: Int
    let via: String?
    let viaName: String?
    let signature: String
}

struct AccountStanding: Codable {
    let status: String
    let deletesAt: String?
    let restorableDays: Int
}

/// The accounts server's routes, signed in as the person.
@MainActor
enum AccountAPI {
    static let offline = "Can't reach Sikemux. Check the phone is online."

    private struct Failure: Decodable {
        struct Detail: Decodable {
            let code: String?
            let message: String?
            let reverifyWithinMinutes: Int?
        }
        let error: Detail?
    }

    private struct Nothing: Decodable {}

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

    private static func request(_ method: String, _ path: String, body: (any Encodable)?) async throws -> Data {
        var request = URLRequest(url: URL(string: Config.apiUrl.absoluteString + path)!, timeoutInterval: 30)
        request.httpMethod = method
        do {
            request.setValue("Bearer \(try await Account.shared.token())", forHTTPHeaderField: "Authorization")
        } catch {
            throw ApiProblem(status: 401, code: "unauthorized", message: "Sign in again.")
        }
        if let body {
            request.setValue("application/json", forHTTPHeaderField: "Content-Type")
            request.httpBody = try JSONEncoder().encode(body)
        }
        let (data, response): (Data, URLResponse)
        do {
            (data, response) = try await URLSession.shared.data(for: request)
        } catch {
            throw ApiProblem(status: nil, code: nil, message: offline)
        }
        let status = (response as? HTTPURLResponse)?.statusCode ?? 0
        guard (200..<300).contains(status) else {
            let detail = (try? JSONDecoder().decode(Failure.self, from: data))?.error
            throw ApiProblem(
                status: status,
                code: detail?.code,
                message: detail?.message ?? "The accounts server answered \(status).",
                reverifyWithinMinutes: detail?.reverifyWithinMinutes
            )
        }
        return status == 204 ? Data("{}".utf8) : data
    }
}

/// The account was deleted and can still be restored, until `deletesAt`.
struct AccountDeleting: Error {
    let deletesAt: String
}

/// What this phone does on the account: join it, leave it, and read and change what is on it.
@MainActor
enum AccountCalls {
    private struct Challenge: Decodable { let nonce: String }
    private struct DeviceList: Decodable { let devices: [AccountDevice] }
    private struct Registration: Encodable {
        let key: String
        let role = "client"
        let name: String
        let platform = "ios"
        let nonce: String
        let signature: String
    }

    /// Adds this phone to the account, proving it holds its key; again only updates its name. A deleted account
    /// that can still be restored throws AccountDeleting.
    static func registerPhone(userId: String) async throws {
        do {
            let challenge = try await AccountAPI.send("POST", "/v1/devices/challenge", as: Challenge.self)
            let me = try Identity.shared.deviceIdentity()
            let registration = Registration(
                key: me.id(),
                name: String(UIDevice.current.name.prefix(64)),
                nonce: challenge.nonce,
                signature: try me.signRegistration(nonce: challenge.nonce, userId: userId)
            )
            _ = try await AccountAPI.send("POST", "/v1/devices", body: registration, as: AccountDevice.self)
        } catch let problem as ApiProblem where problem.status == 401 {
            let account = try? await standing()
            if account?.status == "deleting", let deletesAt = account?.deletesAt { throw AccountDeleting(deletesAt: deletesAt) }
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

    static func hosts() async throws -> [AccountDevice] {
        try await AccountAPI.send("GET", "/v1/devices?role=host", as: DeviceList.self).devices
    }

    static func device(_ key: String) async throws -> AccountDevice? {
        try await AccountAPI.send("GET", "/v1/devices", as: DeviceList.self).devices.first { $0.key == key }
    }

    static func removeDevice(_ key: String) async throws {
        try await AccountAPI.send("DELETE", "/v1/devices/\(key)")
    }

    static func joinTicket(host: String) async throws -> JoinTicket {
        let key = try Identity.shared.deviceIdentity().id()
        return try await AccountAPI.send("POST", "/v1/devices/\(key)/join", body: ["host": host], as: JoinTicket.self)
    }

    static func standing() async throws -> AccountStanding {
        try await AccountAPI.send("GET", "/v1/account", as: AccountStanding.self)
    }

    static func restore() async throws -> AccountStanding {
        try await AccountAPI.send("POST", "/v1/account/restore", as: AccountStanding.self)
    }

    /// Deletes the account and every device on it, with Apple's code for the server to revoke when it signs in
    /// with Apple.
    static func delete(appleAuthorizationCode: String?) async throws {
        let body = appleAuthorizationCode.map { ["appleAuthorizationCode": $0] }
        try await AccountAPI.send("DELETE", "/v1/account", body: body)
    }
}
