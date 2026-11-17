import Foundation
import WidgetKit

extension GlanceStore {
    /// What a card says of its agent, kept for the widgets until the app hears from the host itself.
    static func heard(_ card: Card) {
        guard var glance = read(), let index = glance.hosts.firstIndex(where: { $0.host == card.hostKey }) else {
            return
        }
        var agents = glance.hosts[index].agents
        let shown = agents.rows.contains { $0.agentId == card.agentId }
        switch card.kind {
        case .permission, .input:
            if shown { return }
            let asking = ActivityRow(
                agentId: card.agentId, cli: card.provider, title: card.chatTitle ?? card.title, name: nil,
                teamWorking: 0)
            agents.needed += 1
            agents.projects = []
            agents.rows.insert(asking, at: 0)
            if agents.rows.count > 3 {
                agents.rows.removeLast()
                agents.more += 1
            }
        case .finished, .problem, .clear:
            guard shown else { return }
            agents.rows.removeAll { $0.agentId == card.agentId }
            agents.needed = max(0, agents.needed - 1)
        case .pairing, .unknown:
            return
        }
        glance.hosts[index].agents = agents
        write(glance)
        WidgetCenter.shared.reloadTimelines(ofKind: agentsWidget)
    }
}
