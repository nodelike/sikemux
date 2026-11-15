import SwiftUI

/// The background tasks that ended in one turn, as one line with a dot for each; a tap lists them in a sheet.
struct EndedTasksLine: View {
    @Environment(\.palette) private var palette
    let tasks: EndedTasks
    @State private var shown = false

    var body: some View {
        let count = tasks.tasks.reduce(0) { $0 + $1.times }
        let named = count == 1 ? String(localized: "1 background task") : String(localized: "\(count) background tasks")
        Button {
            Haptics.select()
            shown = true
        } label: {
            HStack(spacing: 8) {
                IconView(icon: tasks.failed > 0 ? Icons.circleAlert : Icons.circleCheck, size: 15)
                    .foregroundStyle(tasks.failed > 0 ? palette.danger : palette.live)
                Text(named).font(Theme.ui(13.5, .medium)).foregroundStyle(palette.ink).lineLimit(1)
                EndedTaskDots(tasks: tasks)
                IconView(icon: Icons.chevron, size: 11).foregroundStyle(palette.tertiary)
            }
            .frame(height: 32)
            .padding(.horizontal, 6)
            .contentShape(Rectangle())
        }
        .buttonStyle(ActiveFill(radius: 8))
        .padding(.leading, -6)
        .padding(.vertical, 2)
        .frame(maxWidth: .infinity, alignment: .leading)
        .accessibilityLabel("\(named), \(EndedTaskWords.ended(tasks))")
        .liftedSheet(shown: $shown) { EndedTasksSheet(tasks: tasks) }
    }
}

/// One dot for each task, in the colour of how it ended.
private struct EndedTaskDots: View {
    @Environment(\.palette) private var palette
    let tasks: EndedTasks

    private static let most = 16

    var body: some View {
        let states = tasks.tasks.flatMap { Array(repeating: $0.state, count: $0.times) }.prefix(Self.most)
        HStack(spacing: 3) {
            ForEach(Array(states.enumerated()), id: \.offset) { _, state in
                Circle().fill(EndedTaskWords.ink(state, palette)).frame(width: 6, height: 6)
            }
        }
        .accessibilityHidden(true)
    }
}

private struct EndedTasksSheet: View {
    @Environment(\.palette) private var palette
    let tasks: EndedTasks

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Text("Background tasks").font(Theme.ui(17, .semibold)).foregroundStyle(palette.ink)
            Text(EndedTaskWords.ended(tasks)).font(Theme.ui(14)).foregroundStyle(palette.tertiary)
                .padding(.top, 4)
                .padding(.bottom, 10)
            ForEach(Array(tasks.tasks.enumerated()), id: \.offset) { at, task in
                if at > 0 { Rectangle().fill(palette.border).frame(height: 1) }
                EndedTaskRow(task: task)
            }
        }
        .padding(.horizontal, 4)
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

/// A task in the sheet; one that said how it went opens onto that.
private struct EndedTaskRow: View {
    @Environment(\.palette) private var palette
    let task: EndedTask
    @State private var open = false

    var body: some View {
        let stopped = task.state == .stopped
        let watch = task.taskType == "monitor"
        let icon =
            switch task.state {
            case .completed: Icons.check
            case .failed: Icons.close
            default: watch ? Icons.eye : Icons.run
            }
        Button {
            withAnimation(.easeOut(duration: 0.18)) { open.toggle() }
        } label: {
            VStack(alignment: .leading, spacing: 4) {
                HStack(spacing: 12) {
                    IconView(icon: icon, size: 16)
                        .foregroundStyle(stopped ? palette.inkFaint : EndedTaskWords.ink(task.state, palette))
                    HStack(spacing: 6) {
                        Text(EndedTaskWords.sentence(task.name)).lineLimit(1)
                        if task.times > 1 {
                            Text("×\(task.times)").font(Theme.mono(12)).foregroundStyle(palette.tertiary)
                                .padding(.horizontal, 4)
                                .overlay(RoundedRectangle(cornerRadius: 4).stroke(palette.border, lineWidth: 1))
                        }
                    }
                    .font(Theme.ui(15))
                    .foregroundStyle(stopped ? palette.tertiary : palette.secondary)
                    Spacer(minLength: 0)
                    if let word = EndedTaskWords.state(task.state) {
                        Text(word).font(Theme.mono(13))
                            .foregroundStyle(task.state == .failed ? palette.danger : palette.tertiary)
                    }
                    if task.summary != nil {
                        IconView(icon: Icons.chevron, size: 12).foregroundStyle(palette.inkFaint)
                            .rotationEffect(.degrees(open ? 90 : 0))
                    }
                }
                .frame(minHeight: 46)
                if open, let summary = task.summary {
                    Text(summary).font(Theme.ui(13.5)).foregroundStyle(palette.inkDim)
                        .padding(.leading, 28)
                        .padding(.bottom, 10)
                        .textSelection(.enabled)
                }
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .allowsHitTesting(task.summary != nil)
        .accessibilityValue(task.summary == nil ? "" : open ? "expanded" : "collapsed")
    }
}

private enum EndedTaskWords {
    /// How many ended each way, as "5 done · 2 watches stopped".
    static func ended(_ tasks: EndedTasks) -> String {
        var said: [String] = []
        if tasks.done > 0 { said.append(String(localized: "\(tasks.done) done")) }
        if tasks.failed > 0 { said.append(String(localized: "\(tasks.failed) failed")) }
        if tasks.stopped > 0 {
            if tasks.stopped == tasks.watchesStopped {
                said.append(
                    tasks.stopped == 1
                        ? String(localized: "1 watch stopped") : String(localized: "\(tasks.stopped) watches stopped"))
            } else {
                said.append(String(localized: "\(tasks.stopped) stopped"))
            }
        }
        return said.joined(separator: " · ")
    }

    static func state(_ state: NoticeState) -> String? {
        switch state {
        case .completed: nil
        case .failed: String(localized: "failed")
        case .stopped: String(localized: "stopped")
        case .unknown: nil
        }
    }

    static func ink(_ state: NoticeState, _ palette: Palette) -> Color {
        switch state {
        case .completed: palette.live.opacity(0.85)
        case .failed: palette.danger
        default: palette.inkFaint
        }
    }

    static func sentence(_ name: String) -> String {
        name.prefix(1).uppercased() + name.dropFirst()
    }
}
