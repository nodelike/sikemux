import SwiftUI

/// A host's open agents as the transcript names them: each one's `@name`, and the mark its chip wears, as the Mac draws
/// an agent its prose names.
struct Crew: Equatable {
    struct Named: Hashable, Sendable {
        let id: String
        /// What prose writes for it, such as `@Otter`.
        let written: String
        let chief: Bool
        let provider: String
        let face: AgentFace?
        let project: String?
    }

    /// Longest name first, so `@Otterly` is not read as `@Otter`.
    let named: [Named]

    @MainActor init?(_ view: DeviceView?) {
        guard let view else { return nil }
        let faces = Faces(view)
        named = view.inbox.agents
            .compactMap { agent in
                (agent.chief == true ? "Chief" : agent.name).map {
                    Named(
                        id: agent.agentId, written: "@\($0)", chief: agent.chief == true, provider: agent.cli,
                        face: agent.face ?? faces?.face(agent.agentId), project: agent.project)
                }
            }
            .sorted { $0.written.count > $1.written.count }
    }

    private init(named: [Named]) { self.named = named }

    /// The crew as `writer` names it: where two projects share a name, its own project's agent comes first.
    func writtenBy(_ writer: String?) -> Crew {
        guard let own = agent(writer)?.project else { return self }
        let ranked = named.enumerated().sorted { a, b in
            if a.element.written.count != b.element.written.count {
                return a.element.written.count > b.element.written.count
            }
            let (ownA, ownB) = (a.element.project == own, b.element.project == own)
            return ownA != ownB ? ownA : a.offset < b.offset
        }
        return Crew(named: ranked.map(\.element))
    }

    /// `text` cut where it names an agent: the words between, and each agent it names.
    func split(_ text: String) -> [(words: String, agent: Named?)] {
        guard text.contains("@") else { return [(text, nil)] }
        var out: [(String, Named?)] = []
        var rest = Substring(text)
        var plain = ""
        while let at = rest.firstIndex(of: "@") {
            plain += rest[..<at]
            let tail = rest[at...]
            let found = named.first { agent in
                guard tail.hasPrefix(agent.written) else { return false }
                let after = tail.dropFirst(agent.written.count).first
                return after.map { !$0.isLetter && !$0.isNumber && $0 != "_" && $0 != "-" } ?? true
            }
            if let found {
                if !plain.isEmpty { out.append((plain, nil)) }
                plain = ""
                out.append((found.written, found))
                rest = tail.dropFirst(found.written.count)
            } else {
                plain += "@"
                rest = tail.dropFirst()
            }
        }
        plain += rest
        if !plain.isEmpty { out.append((plain, nil)) }
        return out
    }
}

extension EnvironmentValues {
    @Entry var crew: Crew?
}

/// Sikemux's S, as big as an agent's face of `size`.
struct ChiefMark: View {
    let size: CGFloat
    let color: Color

    var body: some View {
        IconView(icon: Icons.brand, size: size * 0.9).foregroundStyle(color).frame(width: size, height: size)
    }
}

/// An agent's own mark: Sikemux's S on a project's chief, else the face the host gave it, else its CLI's logo.
struct OwnMark: View {
    @Environment(\.palette) private var palette
    let agentId: String?
    let provider: String
    let chief: Bool
    var turn: TurnState?
    let size: CGFloat

    var body: some View {
        if chief {
            ChiefMark(size: size, color: palette.accent)
        } else {
            AgentFaceView(agentId: agentId, provider: provider, turn: turn, size: size)
        }
    }
}

/// The still mark an agent's chip wears inside a line of words, drawn once into a picture.
struct NamedMark: View {
    let agent: Crew.Named
    let accent: Color
    let size: CGFloat

    var body: some View {
        if agent.chief {
            ChiefMark(size: size, color: accent)
        } else if let face = agent.face, let art = BuddyArt.shared.art(face, state: .idle) {
            BuddyFigure(art: art, state: .idle, moves: false, size: size)
        } else {
            AgentMark(provider: agent.provider, size: size)
        }
    }
}

/// An agent's chip as the transcript's prose draws it: its mark and its name on a grey lift, opening its chat.
enum AgentChip {
    static let scheme = "sikemux-agent"

    static func run(_ agent: Crew.Named, palette: Palette, size: CGFloat) -> AttributedString {
        var run =
            AttributedString("\u{2009}") + FileGlyph.agent(agent, accent: palette.accent, size: size)
            + AttributedString("\u{2009}\(agent.written)\u{2009}")
        run.foregroundColor = palette.ink
        run.backgroundColor = palette.raised
        var parts = URLComponents()
        parts.scheme = scheme
        parts.host = "chat"
        parts.queryItems = [URLQueryItem(name: "agent", value: agent.id)]
        run.link = parts.url
        return run
    }

    /// The agent whose chat a tapped chip opens.
    static func agent(_ url: URL) -> String? {
        guard url.scheme == scheme else { return nil }
        return URLComponents(url: url, resolvingAgainstBaseURL: false)?.queryItems?.first { $0.name == "agent" }?.value
    }
}

extension Crew {
    func agent(_ id: String?) -> Named? { id.flatMap { id in named.first { $0.id == id } } }
}

/// An agent's chip outside prose, as the Mac's turn line and header draw it at their own size: its mark and name on an
/// opaque lift; a tap opens its chat.
struct AgentPill: View {
    @Environment(\.palette) private var palette
    @Environment(\.hostCore) private var core
    let agent: Crew.Named
    let size: CGFloat

    var body: some View {
        Button {
            Haptics.select()
            Router.shared.push(.chat(core: core, agent: agent.id))
        } label: {
            HStack(spacing: 5) {
                NamedMark(agent: agent, accent: palette.accent, size: size + 1)
                Text(agent.written).lineLimit(1)
            }
            .font(Theme.ui(size, .medium))
            .foregroundStyle(palette.ink)
            .padding(.leading, 4)
            .padding(.trailing, 6)
            .frame(height: size + 8)
            .background(palette.raised, in: RoundedRectangle(cornerRadius: Radius.control * 0.5))
        }
        .buttonStyle(.plain)
        .accessibilityLabel(agent.written)
    }
}
