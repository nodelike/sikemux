import SwiftUI

extension HostLive {
    /// What this host's core says of the agent `type`.
    func agent(_ type: String) -> CatalogAgent? {
        catalog.first { $0.type == type }
    }
}

/// The agents each paired host can run, as its core's catalog says. An agent is named and coloured alike on every
/// host, so where no host is in view any host's word for it does.
@MainActor
enum Catalog {
    static func entry(_ type: String, core: String? = nil) -> CatalogAgent? {
        let hub = Hub.shared
        if let core, let found = hub.live(core).agent(type) { return found }
        for device in hub.devices {
            if let found = hub.live(device.core).agent(type) { return found }
        }
        return nil
    }

    /// The agent's name; anything not in the catalog keeps the id it came with.
    static func label(_ type: String, core: String? = nil) -> String {
        entry(type, core: core)?.label ?? type
    }

    static func brand(_ type: String, core: String? = nil) -> Color? {
        entry(type, core: core).map { Color(hex: $0.brand) }
    }

    /// Whether how much of the agent's plan is used can be read, which earns its context ring the agent's own colour.
    static func readsUsage(_ type: String) -> Bool {
        entry(type)?.usage == true
    }

    /// The core switches a running chat between safe and YOLO for every agent whose CLI on it can start without
    /// asking.
    static func canSwitchMode(_ type: String, core: String) -> Bool {
        Hub.shared.live(core).agent(type)?.bypass == true
    }
}
