import SwiftUI

/// A list's small-caps label, with a hairline running to the right as on the Mac's rail.
struct SectionLabel<Action: View>: View {
    @Environment(\.palette) private var palette
    let text: String
    var count: Int?
    @ViewBuilder var action: Action

    var body: some View {
        HStack(spacing: 10) {
            Text(text.uppercased())
                .font(Theme.ui(11, .medium))
                .tracking(1.76)
                .foregroundStyle(palette.tertiary)
            if let count {
                Text("\(count)")
                    .font(Theme.ui(11, .medium))
                    .monospacedDigit()
                    .foregroundStyle(palette.inkDim)
                    .padding(.leading, -4)
            }
            Rectangle().fill(palette.ink.opacity(0.09)).frame(height: 1)
            action
        }
        .padding(.top, 18)
        .padding(.bottom, 6)
        .padding(.horizontal, 8)
    }
}

extension SectionLabel where Action == EmptyView {
    init(_ text: String, count: Int? = nil) {
        self.init(text: text, count: count, action: { EmptyView() })
    }
}

/// Rows stacked as the Mac's rail stacks them.
struct Rows<Content: View>: View {
    @ViewBuilder var content: Content

    var body: some View {
        VStack(spacing: 2) { content }
    }
}

/// One row of a list, laid out like the Mac's rail rows: a mark, the title with its place and detail under it, and
/// the agent's name with the row's counts and state at its end.
struct Row<Mark: View, End: View, After: View>: View {
    @Environment(\.palette) private var palette
    var title: String?
    /// The agent's name, small and dim at the row's end.
    var name: String?
    /// The linked host the agent runs on, set after its name.
    var host: String?
    var detail: String?
    /// The project the row belongs to, marked with the folder the Mac's rail puts beside it.
    var place: String?
    var under: String?
    var dim = false
    var detailLines = 1
    var onPress: (() -> Void)?
    var onLongPress: (() -> Void)?
    @ViewBuilder var mark: Mark
    @ViewBuilder var end: End
    /// A control of its own at the row's end, beside the row's press rather than inside it.
    @ViewBuilder var after: After

    var body: some View {
        let row = Button {
            onPress?()
        } label: {
            content
        }
        .buttonStyle(RowPressStyle(enabled: onPress != nil))
        .disabled(onPress == nil && onLongPress == nil)
        .simultaneousGesture(LongPressGesture().onEnded { _ in
            guard let onLongPress else { return }
            Haptics.tap()
            onLongPress()
        }, including: onLongPress == nil ? .subviews : .all)

        if After.self == EmptyView.self {
            row
        } else {
            HStack(spacing: 0) {
                row.frame(maxWidth: .infinity)
                after
            }
            .padding(.trailing, 8)
        }
    }

    private var content: some View {
        let handle = name.map { "@\($0)" }
        let where_ = host.map { "on \($0)" }
        let label = title ?? handle ?? ""
        let named = title != nil && handle != nil ? [handle, where_].compactMap { $0 }.joined(separator: " ") : nil
        return HStack(spacing: 10) {
            if Mark.self != EmptyView.self {
                mark.frame(width: 20, height: 20)
            }
            VStack(alignment: .leading, spacing: 1) {
                HStack(alignment: .firstTextBaseline, spacing: 0) {
                    Text(label)
                        .font(Theme.ui(14.5, .medium))
                        .tracking(-0.15)
                        .foregroundStyle(dim ? palette.tertiary : palette.ink)
                        .lineLimit(1)
                    if let where_, named == nil {
                        Text(" \(where_)")
                            .font(Theme.ui(14.5))
                            .foregroundStyle(palette.tertiary)
                            .lineLimit(1)
                            .layoutPriority(1)
                    }
                }
                if let place {
                    HStack(spacing: 4) {
                        IconView(icon: Icons.folder, size: 11).foregroundStyle(palette.live)
                        Text(detail.map { "\(place) · \($0)" } ?? place)
                            .font(Theme.ui(12))
                            .foregroundStyle(palette.tertiary)
                            .lineLimit(1)
                    }
                } else if let detail {
                    Text(detail)
                        .font(Theme.ui(12))
                        .foregroundStyle(palette.tertiary)
                        .lineLimit(detailLines)
                }
                if let under {
                    Text(under).font(Theme.ui(12)).foregroundStyle(palette.tertiary).lineLimit(1)
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            if named != nil || End.self != EmptyView.self {
                HStack(spacing: 6) {
                    if let named {
                        Text(named)
                            .font(Theme.ui(11.5))
                            .foregroundStyle(palette.tertiary)
                            .lineLimit(1)
                            .frame(maxWidth: 110, alignment: .trailing)
                    }
                    end
                }
                .frame(minWidth: 20)
            }
        }
        .padding(.horizontal, 8)
        .padding(.vertical, 6)
        .frame(minHeight: 48)
        .contentShape(Rectangle())
    }
}

extension Row where After == EmptyView {
    init(
        title: String? = nil, name: String? = nil, host: String? = nil, detail: String? = nil, place: String? = nil,
        under: String? = nil, dim: Bool = false, detailLines: Int = 1,
        onPress: (() -> Void)? = nil, onLongPress: (() -> Void)? = nil,
        @ViewBuilder mark: () -> Mark, @ViewBuilder end: () -> End
    ) {
        self.init(
            title: title, name: name, host: host, detail: detail, place: place, under: under, dim: dim,
            detailLines: detailLines, onPress: onPress, onLongPress: onLongPress, mark: mark, end: end,
            after: { EmptyView() })
    }
}

/// A row lights the Mac's neutral pressed surface while a finger is on it.
struct RowPressStyle: ButtonStyle {
    @Environment(\.palette) private var palette
    var enabled = true

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .background(
                RoundedRectangle(cornerRadius: 9, style: .continuous)
                    .fill(configuration.isPressed && enabled ? palette.active : .clear)
            )
    }
}
