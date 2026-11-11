import Foundation

/// What every sender of a stash upload agrees on with the accounts server, as `server/protocol/vectors/stash.json`
/// pins it: the limits, and the headers a file's upload carries beside its bytes.
enum StashRules {
    static let mostBytes: Int64 = 25 * 1024 * 1024
    /// The longest text the server takes as text; longer text goes as a file.
    static let mostText = 16000

    /// A file's headers: its name, note and target URI-encoded, and the prompt it belongs to.
    static func headers(mime: String, name: String, note: String? = nil, target: String? = nil, prompt: String? = nil)
        -> [String: String]
    {
        var headers = ["content-type": mime, "x-stash-name": uriEncoded(name)]
        headers["x-stash-note"] = note.map(uriEncoded)
        headers["x-stash-target"] = target.map(uriEncoded)
        headers["x-stash-prompt"] = prompt
        return headers
    }

    /// As JavaScript's encodeURIComponent, which the server decodes with.
    private static func uriEncoded(_ text: String) -> String {
        let allowed = CharacterSet(
            charactersIn: "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_.!~*'()")
        return text.addingPercentEncoding(withAllowedCharacters: allowed) ?? text
    }
}
