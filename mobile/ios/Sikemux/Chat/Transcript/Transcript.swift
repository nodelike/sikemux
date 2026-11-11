import SwiftUI

/// A whole transcript: the rows the core folded, as the person opened them, the earlier turns arriving above them, and
/// what the screen puts below the last. The chat screen and a shared chat draw the same one.
struct Transcript<Footer: View>: View {
    @Environment(\.palette) private var palette
    let rows: ChatRows
    /// Counts each change to the rows, so a redraw that changed nothing is skipped.
    let drawn: Int
    let touched: () -> Touched
    let context: TranscriptContext
    let scroll: TranscriptScroll
    var hasEarlier = false
    var earlier = Feed.Earlier.idle
    var loadEarlier: () -> Void = {}
    /// The worktree the chat runs in, said above its first row.
    var worktree: AgentWorktree?
    /// What the screen adds to each row's look: what it sent, the strip it opened, what can be edited.
    var look: (ChatMessage, inout RowLook) -> Void = { _, _ in }
    @ViewBuilder var footer: () -> Footer

    var body: some View {
        let shown = shownRows()
        TranscriptList(
            order: shown.order, entries: shown.entries, drawn: drawn, touched: touched, scroll: scroll,
            hasEarlier: hasEarlier && earlier == .idle, loadEarlier: loadEarlier,
            environment: { [palette, context] view in AnyView(view.environment(\.palette, palette).environment(context))
            },
            header: {
                TranscriptHeader(
                    hasEarlier: hasEarlier, earlier: earlier, lost: rows.head.earlierLost, retry: loadEarlier)
            },
            footer: footer,
            row: { entry in TranscriptRow(entry: entry) }
        )
        .environment(\.openURL, OpenURLAction { url in opened(url) })
        .sheet(item: Bindable(context).picture) { picture in ImageViewer(image: picture.image, name: picture.name) }
    }

    private func opened(_ url: URL) -> OpenURLAction.Result {
        guard let path = TranscriptLink.filePath(url) else { return .systemAction }
        if let image = HostFiles.imagePath(path), let core = context.core {
            Task {
                guard let file = await HostFiles.shared.read(core: core, path: image),
                    let picture = UIImage(contentsOfFile: file.path)
                else { return }
                context.showImage(picture, (image as NSString).lastPathComponent)
            }
        }
        return .handled
    }

    private func shownRows() -> (order: [String], entries: [String: RowEntry]) {
        let folds = context.folds
        let messages = rows.messages
        let runs = ShownRows.teamRows(messages) { id in
            folds.isOpen(Folds.teamAll(id), false) ? .all : folds.isOpen(Folds.team(id), false) ? .open : .closed
        }
        let liveId = rows.running ? messages.last?.id : nil
        var order: [String] = []
        var entries: [String: RowEntry] = [:]
        for (index, message) in messages.enumerated() {
            let run = runs[message.id]
            let fold = ShownRows.fold(message, { folds.isOpen(Folds.work($0), false) }, run)
            // A folded row that draws nothing stays out of the list, so it costs nothing to measure.
            guard !fold.hidden || fold.summary != nil || run?.head != nil else { continue }
            var look = RowLook()
            look.live = message.id == liveId
            look.kept = fold.kept
            look.hidden = fold.hidden
            look.summary = fold.summary
            look.workOpen = fold.summary.map { folds.isOpen(Folds.work($0.id), false) } ?? false
            if let run, let head = run.head {
                look.team = head
                look.teamId = run.id
                look.teamOpen = folds.isOpen(Folds.team(run.id), false)
                look.teamAll = folds.isOpen(Folds.teamAll(run.id), false)
            }
            if index == 0, !hasEarlier { look.worktree = worktree }
            self.look(message, &look)
            order.append(message.id)
            entries[message.id] = RowEntry(message: message, look: look)
        }
        return (order, entries)
    }
}

/// Above the first row: earlier turns arriving, or a note that the host no longer keeps them.
private struct TranscriptHeader: View {
    @Environment(\.palette) private var palette
    let hasEarlier: Bool
    let earlier: Feed.Earlier
    let lost: Bool
    let retry: () -> Void

    var body: some View {
        if hasEarlier {
            EarlierLine(failed: earlier == .failed, loading: earlier == .loading, onRetry: retry)
        } else if lost {
            Text(ChatWords.earlierLost).font(Theme.ui(12.5)).foregroundStyle(palette.tertiary)
                .frame(maxWidth: .infinity).padding(.top, 10).padding(.bottom, 6)
        } else {
            Color.clear.frame(height: 6)
        }
    }
}

/// The round button over a transcript's end while the reader is away from it.
struct JumpToLatest: View {
    @Environment(\.palette) private var palette
    let scroll: TranscriptScroll

    var body: some View {
        if scroll.away {
            Button {
                scroll.toEnd()
            } label: {
                IconView(icon: Icons.arrowDown, size: 16)
                    .foregroundStyle(palette.inkDim)
                    .frame(width: 34, height: 34)
                    .background(Circle().fill(palette.composer))
                    .overlay(Circle().strokeBorder(palette.border, lineWidth: 1))
                    .shadow(color: .black.opacity(0.4), radius: 12, y: 6)
            }
            .buttonStyle(.plain)
            .padding(.bottom, 8)
            .accessibilityLabel("Jump to latest message")
        }
    }
}
