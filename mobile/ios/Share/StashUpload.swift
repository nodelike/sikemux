import CryptoKit
import Foundation

enum StashProblem: LocalizedError {
  case signedOut
  case unreachable
  case refused(String)

  var errorDescription: String? {
    switch self {
    case .signedOut: return "Open Sikemux and sign in, then share again."
    case .unreachable: return "Check the phone is online."
    case let .refused(message): return message
    }
  }
}

/// Stashes on the account as this phone, signing each request with its key, since the extension
/// has no sign-in of its own.
final class StashUpload: Sendable {
  private let api: URL
  private let key: Curve25519.Signing.PrivateKey
  private let session = URLSession(configuration: .ephemeral)

  init?(api: String, keyHex: String) {
    guard let url = URL(string: api), let seed = Data(hex: keyHex), seed.count == 32,
          let key = try? Curve25519.Signing.PrivateKey(rawRepresentation: seed)
    else { return nil }
    self.api = url
    self.key = key
  }

  func link(_ url: String, note: String?, target: StashTarget?) async throws {
    try await json(["kind": "link", "url": url], note: note, target: target)
  }

  func text(_ text: String, note: String?, target: StashTarget?) async throws {
    try await json(["kind": "text", "text": text], note: note, target: target)
  }

  /// Sends the file as its bytes, read from disk as it goes rather than held in memory.
  func file(
    at url: URL, name: String, mime: String, note: String?, target: StashTarget?,
    progress: @escaping @Sendable @MainActor (Double) -> Void
  ) async throws {
    var request = signed(body: try Self.sha256(of: url))
    let targetJson = try target.map { String(decoding: try JSONEncoder().encode($0), as: UTF8.self) }
    for (field, value) in StashRules.headers(mime: mime, name: name, note: note, target: targetJson) {
      request.setValue(value, forHTTPHeaderField: field)
    }
    let watcher = Progress(report: progress)
    try answer(try await perform { try await self.session.upload(for: request, fromFile: url, delegate: watcher) })
  }

  private func json(_ fields: [String: String], note: String?, target: StashTarget?) async throws {
    var body: [String: Any] = fields
    if let note { body["note"] = note }
    if let target { body["target"] = try JSONSerialization.jsonObject(with: try JSONEncoder().encode(target)) }
    let data = try JSONSerialization.data(withJSONObject: body)
    var request = signed(body: Self.hex(SHA256.hash(data: data)))
    request.setValue("application/json", forHTTPHeaderField: "content-type")
    try answer(try await perform { try await self.session.upload(for: request, from: data) })
  }

  /// A POST to /v1/stash with the phone's signature over its method, path, time and body.
  private func signed(body bodyHash: String) -> URLRequest {
    let url = api.appendingPathComponent("v1/stash")
    let time = String(Int(Date().timeIntervalSince1970))
    let message = "sikemux-device-request|POST|\(url.path)|\(time)|\(bodyHash)"
    let signature = (try? key.signature(for: Data(message.utf8))).map { Self.hex($0) } ?? ""
    let publicKey = Self.hex(key.publicKey.rawRepresentation)
    var request = URLRequest(url: url, timeoutInterval: 60)
    request.httpMethod = "POST"
    request.setValue("SikemuxDevice \(publicKey).\(time).\(signature)", forHTTPHeaderField: "authorization")
    request.setValue(bodyHash, forHTTPHeaderField: "x-body-sha256")
    return request
  }

  private func perform(_ send: () async throws -> (Data, URLResponse)) async throws -> (Data, URLResponse) {
    do {
      return try await send()
    } catch {
      throw StashProblem.unreachable
    }
  }

  private func answer(_ answer: (Data, URLResponse)) throws {
    let status = (answer.1 as? HTTPURLResponse)?.statusCode ?? 0
    if (200 ..< 300).contains(status) { return }
    if status == 401 { throw StashProblem.signedOut }
    let failure = (try? JSONSerialization.jsonObject(with: answer.0)) as? [String: Any]
    let message = (failure?["error"] as? [String: Any])?["message"] as? String
    throw StashProblem.refused(message ?? "Sikemux answered \(status).")
  }

  private static func sha256(of file: URL) throws -> String {
    let handle = try FileHandle(forReadingFrom: file)
    defer { try? handle.close() }
    var hash = SHA256()
    while let chunk = try handle.read(upToCount: 1 << 20), !chunk.isEmpty {
      hash.update(data: chunk)
    }
    return hex(hash.finalize())
  }

  private static func hex<Bytes: Sequence>(_ bytes: Bytes) -> String where Bytes.Element == UInt8 {
    bytes.map { String(format: "%02x", $0) }.joined()
  }
}

/// Reports how much of one upload has gone.
private final class Progress: NSObject, URLSessionTaskDelegate, Sendable {
  private let report: @Sendable @MainActor (Double) -> Void

  init(report: @escaping @Sendable @MainActor (Double) -> Void) {
    self.report = report
  }

  func urlSession(_: URLSession, task _: URLSessionTask, didSendBodyData _: Int64, totalBytesSent: Int64, totalBytesExpectedToSend: Int64) {
    guard totalBytesExpectedToSend > 0 else { return }
    let share = Double(totalBytesSent) / Double(totalBytesExpectedToSend)
    Task { @MainActor in report(share) }
  }
}

extension Data {
  init?(hex: String) {
    guard hex.count % 2 == 0 else { return nil }
    var bytes = [UInt8]()
    bytes.reserveCapacity(hex.count / 2)
    var index = hex.startIndex
    while index < hex.endIndex {
      let next = hex.index(index, offsetBy: 2)
      guard let byte = UInt8(hex[index ..< next], radix: 16) else { return nil }
      bytes.append(byte)
      index = next
    }
    self.init(bytes)
  }
}
