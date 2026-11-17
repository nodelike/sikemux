import SwiftUI

/// Agents laid out as trees by each one's lead, as the Mac's rail lays them out (`app/src/agents/tree.rs`).
enum AgentTree {
    /// How deep a row is indented at most; rows below that stay at this depth and name their lead.
    static let deepest = 3
    /// Down the middle of a root row's mark, which sits 8pt into its row, and how far each level sits right of the
    /// one above.
    static let spine: CGFloat = 18.5
    static let indent: CGFloat = 22

    /// What sits at one level to a row's left.
    enum Line {
        case blank, through, tick, corner
    }

    struct Placed<Item> {
        let item: Item
        let depth: Int
        let lines: [Line]
        /// Its lead's id, when the row does not hang under its lead's row.
        let leadApart: String?
        /// Rows hang under it right below, so a line drops from its mark.
        let open: Bool
        /// How many hang below it at any depth.
        let below: Int
    }

    /// `items` as trees, every root at `root` depth, siblings in the order they come. One whose lead is not among them
    /// is a root and names its lead; the rows under a lead in `folded` are left out.
    static func tree<Item>(
        _ items: [Item], id: (Item) -> String, lead: (Item) -> String?, root: Int, folded: Set<String>
    ) -> [Placed<Item>] {
        let ids = items.map(id)
        let leads = items.map(lead)
        let at = Dictionary(ids.enumerated().map { ($1, $0) }, uniquingKeysWith: { first, _ in first })
        let parent: [Int?] = leads.enumerated().map { index, lead in
            guard let lead, let up = at[lead], up != index else { return nil }
            return up
        }
        var children = Array(repeating: [Int](), count: items.count)
        for (index, up) in parent.enumerated() {
            if let up { children[up].append(index) }
        }
        var below = Array(repeating: 0, count: items.count)
        var placed = Array(repeating: false, count: items.count)
        var order: [(index: Int, depth: Int)] = []
        func walk(_ index: Int, _ depth: Int) -> Int {
            placed[index] = true
            order.append((index, depth))
            var count = 0
            for child in children[index] where !placed[child] {
                count += 1 + walk(child, depth + 1)
            }
            below[index] = count
            return count
        }
        for index in items.indices where parent[index] == nil { _ = walk(index, 0) }
        for index in items.indices where !placed[index] { _ = walk(index, 0) }
        var shown: [(index: Int, depth: Int)] = []
        var hiddenBelow: Int?
        for row in order {
            if let fold = hiddenBelow, row.depth > fold { continue }
            hiddenBelow = nil
            if below[row.index] > 0 && folded.contains(ids[row.index]) { hiddenBelow = row.depth }
            shown.append(row)
        }
        let depths = shown.map { min(root + $0.depth, deepest) }
        let lines = linesOf(depths)
        return shown.enumerated().map { at, row in
            let drawn = depths[at]
            let deeper = root + row.depth > drawn
            return Placed(
                item: items[row.index], depth: drawn, lines: lines[at],
                leadApart: parent[row.index] != nil && !deeper ? nil : leads[row.index],
                open: at + 1 < depths.count && depths[at + 1] > drawn,
                below: below[row.index])
        }
    }

    /// The lines to the left of each row of a tree drawn at `depths`, top to bottom.
    static func linesOf(_ depths: [Int]) -> [[Line]] {
        var later = Array(repeating: false, count: depths.max() ?? 0)
        var lines = Array(repeating: [Line](), count: depths.count)
        for (index, depth) in depths.enumerated().reversed() {
            lines[index] = (0..<depth).map { level in
                switch (level + 1 == depth, later[level]) {
                case (true, true): .tick
                case (true, false): .corner
                case (false, true): .through
                case (false, false): .blank
                }
            }
            for level in depth..<later.count { later[level] = false }
            if depth > 0 { later[depth - 1] = true }
        }
        return lines
    }

    /// The ids hanging, at any depth, above one of `from`, by each id's lead.
    static func above(_ leadOf: [String: String], from: [String]) -> Set<String> {
        var leads = Set<String>()
        for start in from {
            var at = leadOf[start]
            while let lead = at, leads.insert(lead).inserted { at = leadOf[lead] }
        }
        return leads
    }

    /// The top of the tree `id` hangs in among `ids`, by each id's lead.
    static func top(of id: String, _ leadOf: [String: String], among ids: Set<String>) -> String {
        var top = id
        for _ in 0..<ids.count {
            guard let lead = leadOf[top], ids.contains(lead) else { break }
            top = lead
        }
        return top
    }
}

/// The spines, ticks and corners that hang a row under its lead, and the line dropping from its mark to its team.
struct TreeLines: ViewModifier {
    @Environment(\.palette) private var palette
    var depth = 0
    var lines: [AgentTree.Line] = []
    var open = false

    func body(content: Content) -> some View {
        content
            .padding(.leading, AgentTree.indent * CGFloat(depth))
            .background(alignment: .topLeading) {
                TreePath(depth: depth, lines: lines, open: open).stroke(palette.treeSpine, lineWidth: 1)
            }
    }
}

private struct TreePath: Shape {
    let depth: Int
    let lines: [AgentTree.Line]
    let open: Bool
    /// The gap between rows, which the spines run across.
    private let gap: CGFloat = 2

    func path(in rect: CGRect) -> Path {
        let middle = rect.midY
        let x = { (level: Int) in AgentTree.spine + AgentTree.indent * CGFloat(level) }
        var path = Path()
        for (level, line) in lines.enumerated() {
            let spine = x(level)
            switch line {
            case .blank:
                break
            case .through, .tick:
                path.move(to: CGPoint(x: spine, y: -gap))
                path.addLine(to: CGPoint(x: spine, y: rect.maxY + gap))
                if line == .tick {
                    path.move(to: CGPoint(x: spine, y: middle))
                    path.addLine(to: CGPoint(x: spine + 8, y: middle))
                }
            case .corner:
                path.move(to: CGPoint(x: spine, y: -gap))
                path.addLine(to: CGPoint(x: spine, y: middle - 4))
                path.addQuadCurve(to: CGPoint(x: spine + 4, y: middle), control: CGPoint(x: spine, y: middle))
                path.addLine(to: CGPoint(x: spine + 8, y: middle))
            }
        }
        if open {
            path.move(to: CGPoint(x: x(depth), y: middle + 12))
            path.addLine(to: CGPoint(x: x(depth), y: rect.maxY + gap))
        }
        return path
    }
}

/// The chevron that folds the team under a row, with how many it hides while folded, as the Mac's rail counts them.
struct TreeFold: View {
    @Environment(\.palette) private var palette
    let agentId: String
    let count: Int

    var body: some View {
        let open = !Folded.shared.teams.contains(agentId)
        Button {
            Folded.shared.toggleTeam(agentId)
        } label: {
            HStack(spacing: 4) {
                if !open {
                    Text(verbatim: "+\(count)")
                        .font(Theme.ui(11, .medium))
                        .monospacedDigit()
                        .foregroundStyle(palette.tertiary)
                        .padding(.horizontal, 5)
                        .background(palette.active, in: Capsule())
                }
                IconView(icon: Icons.chevron, size: 12)
                    .foregroundStyle(palette.tertiary)
                    .rotationEffect(.degrees(open ? 90 : 0))
            }
            .padding(.leading, 6)
            .frame(minHeight: 36)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel(Text(verbatim: "+\(count)"))
        .accessibilityValue(open ? "Open" : "Folded")
    }
}
