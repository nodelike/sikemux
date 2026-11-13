import SwiftUI

/// A list's small-caps label, with a hairline running to the right as on the Mac's rail.
struct SectionLabel<Action: View>: View {
    @Environment(\.palette) private var palette
    let text: String
    var count: Int?
    @ViewBuilder var action: Action

    var body: some View {
        HStack(spacing: 10) {
            Text(LocalizedStringKey(text))
                .textCase(.uppercase)
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
    /// The model the agent's chat runs on, beside its name while the title fits uncut beside both.
    var model: String?
    /// The linked host the agent runs on, tagged before its name as the Mac's rail tags it.
    var host: String?
    /// Quiet words after the title on its line, such as what a chief's chat is on.
    var aside: String?
    var detail: String?
    /// A detail with its own styling, in place of `detail`.
    var detailText: Text?
    /// The project the row belongs to, marked with the folder the Mac's rail puts beside it.
    var place: String?
    var under: String?
    var underText: Text?
    var dim = false
    var detailLines = 1
    var onPress: (() -> Void)?
    var onLongPress: (() -> Void)?
    @ViewBuilder var mark: Mark
    @ViewBuilder var end: End
    /// A control of its own at the row's end, beside the row's press rather than inside it.
    @ViewBuilder var after: After
    @State private var held = false

    var body: some View {
        let row = Button {
            // A finger lifted after a long press has already done what the row does when held.
            if held {
                held = false
            } else {
                onPress?()
            }
        } label: {
            content
        }
        .buttonStyle(RowPressStyle(enabled: onPress != nil))
        .disabled(onPress == nil && onLongPress == nil)
        .simultaneousGesture(
            LongPressGesture().onEnded { _ in
                guard let onLongPress else { return }
                held = true
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

    private var oneLine: Bool {
        place == nil && detail == nil && detailText == nil && under == nil && underText == nil
    }

    /// The model shows only while the title still fits beside it; the title is what the row is about.
    @ViewBuilder private var content: some View {
        if let model, title != nil, name != nil {
            ViewThatFits(in: .horizontal) {
                layout(model: model)
                layout(model: nil)
            }
        } else {
            layout(model: nil)
        }
    }

    private func layout(model: String?) -> some View {
        let handle = name.map { "@\($0)" }
        let label = title ?? handle ?? ""
        let named = title != nil ? handle : nil
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
                    if let aside {
                        Text("  \(aside)")
                            .font(Theme.ui(14.5))
                            .foregroundStyle(palette.tertiary)
                            .lineLimit(1)
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
                    .frame(idealWidth: 0, alignment: .leading)
                } else if let detail = detailText ?? detail.map({ Text($0) }) {
                    detail
                        .font(Theme.ui(12))
                        .foregroundStyle(palette.tertiary)
                        .lineLimit(detailLines)
                        .frame(idealWidth: 0, alignment: .leading)
                }
                if let under = underText ?? under.map({ Text($0) }) {
                    under.font(Theme.ui(12)).foregroundStyle(palette.tertiary).lineLimit(1)
                        .frame(idealWidth: 0, alignment: .leading)
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            if named != nil || host != nil || End.self != EmptyView.self {
                HStack(spacing: 6) {
                    if let host {
                        HStack(spacing: 4) {
                            IconView(icon: Icons.server, size: 11)
                            Text(host).font(Theme.ui(11, .medium)).lineLimit(1)
                        }
                        .foregroundStyle(palette.live)
                        .opacity(dim ? 0.55 : 1)
                        .frame(maxWidth: 150, alignment: .trailing)
                    }
                    if let named {
                        HStack(spacing: 0) {
                            Text(named).frame(maxWidth: 110, alignment: .trailing).layoutPriority(1)
                            if let model {
                                Text(" · \(model)").frame(maxWidth: 110, alignment: .trailing)
                            }
                        }
                        .font(Theme.ui(11.5))
                        .foregroundStyle(palette.tertiary)
                        .lineLimit(1)
                    }
                    end
                }
                .frame(minWidth: 20)
            }
        }
        .padding(.horizontal, 8)
        .padding(.vertical, 6)
        .frame(minHeight: oneLine ? 36 : 48)
        .contentShape(Rectangle())
    }
}

extension Row where After == EmptyView {
    init(
        title: String? = nil, name: String? = nil, model: String? = nil, host: String? = nil, aside: String? = nil,
        detail: String? = nil,
        detailText: Text? = nil, place: String? = nil, under: String? = nil, underText: Text? = nil, dim: Bool = false,
        detailLines: Int = 1, onPress: (() -> Void)? = nil, onLongPress: (() -> Void)? = nil,
        @ViewBuilder mark: () -> Mark, @ViewBuilder end: () -> End
    ) {
        self.init(
            title: title, name: name, model: model, host: host, aside: aside, detail: detail, detailText: detailText,
            place: place,
            under: under,
            underText: underText, dim: dim,
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

/// A rule one screen pixel thick.
struct Hairline: View {
    @Environment(\.displayScale) private var scale
    let color: Color

    var body: some View {
        Rectangle().fill(color).frame(height: 1 / scale)
    }
}
