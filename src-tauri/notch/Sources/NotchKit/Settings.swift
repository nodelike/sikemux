import Foundation

extension NotchSettings {
    /// The Notch section of the core's preferences, or the defaults when it has none.
    static func of(preferences: Any?) -> NotchSettings {
        guard let notch = (preferences as? [String: Any])?["notch"],
              let data = try? JSONSerialization.data(withJSONObject: notch),
              let settings = try? JSONDecoder().decode(NotchSettings.self, from: data)
        else { return NotchSettings() }
        return settings
    }
}
