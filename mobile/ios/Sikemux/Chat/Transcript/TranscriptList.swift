import SwiftUI
import UIKit

/// How one row of the transcript draws, apart from the message itself, so a row is drawn again only when what it
/// shows changed.
struct RowLook: Equatable {
    var live = false
    var lit = false
    var unsent: Unsent?
    /// Which parts stay in view, or nil when the row shows whole.
    var kept: [Int]?
    /// The row draws nothing besides what goes above it.
    var hidden = false
    /// The folded work whose summary goes above this row, its first.
    var summary: WorkFold?
    var workOpen = false
    /// The run of messages between agents this row opens, and how far it is open.
    var team: TeamLine?
    var teamId: String?
    var teamOpen = false
    var teamAll = false
    /// The worktree the chat runs in, said above its first row.
    var worktree: AgentWorktree?
    var strip: Strip?
    var editable = false
    var editing = false
}

struct RowEntry: Equatable {
    var message: ChatMessage
    var look: RowLook
}

/// What a tap on the transcript asks of the screen around it.
@MainActor
final class TranscriptActions {
    var tap: (ChatMessage) -> Void = { _ in }
    var retry: (String) -> Void = { _ in }
    var edit: (ChatMessage) -> Void = { _ in }
    var toggleWork: (String) -> Void = { _ in }
    var toggleRun: (String) -> Void = { _ in }
    var showRunEarlier: (String) -> Void = { _ in }
}

/// Where the transcript stands for the screen over it: whether the reader left its end, and how to go back there or to
/// a row.
@MainActor @Observable
final class TranscriptScroll {
    /// The reader scrolled away from the newest message.
    var away = false
    /// The first row in view.
    var topRow: String?
    @ObservationIgnored fileprivate weak var controller: TranscriptController?

    func toEnd() { controller?.toEnd(animated: true) }

    /// Brings row `id` into view, near the top.
    func reveal(_ id: String) { controller?.reveal(id) }

    /// Keeps the rows in view where they are through the next change, as when a fold above them opens.
    func hold() { controller?.holdPlace() }
}

/// The transcript as one list that draws only the rows in view and reuses their cells, so a long chat scrolls as a
/// short one does and a streaming answer redraws only its own row.
struct TranscriptList<Header: View, Footer: View, Row: View>: UIViewControllerRepresentable {
    let order: [String]
    let entries: [String: RowEntry]
    /// Counts each change the session made, so a redraw that changed nothing is skipped.
    let drawn: Int
    let touched: () -> Touched
    let scroll: TranscriptScroll
    let hasEarlier: Bool
    let loadEarlier: () -> Void
    let environment: (AnyView) -> AnyView
    @ViewBuilder let header: () -> Header
    @ViewBuilder let footer: () -> Footer
    @ViewBuilder let row: (RowEntry) -> Row

    func makeUIViewController(context: Context) -> TranscriptController {
        let controller = TranscriptController()
        scroll.controller = controller
        controller.makeHeader = { [environment, header] in environment(AnyView(header())) }
        controller.makeFooter = { [environment, footer] in environment(AnyView(footer())) }
        controller.makeRow = { [environment, row] entry in environment(AnyView(row(entry))) }
        controller.scroll = scroll
        return controller
    }

    func updateUIViewController(_ controller: TranscriptController, context: Context) {
        controller.makeRow = { [environment, row] entry in environment(AnyView(row(entry))) }
        controller.makeHeader = { [environment, header] in environment(AnyView(header())) }
        controller.makeFooter = { [environment, footer] in environment(AnyView(footer())) }
        controller.loadEarlier = loadEarlier
        controller.hasEarlier = hasEarlier
        controller.update(order: order, entries: entries, drawn: drawn, touched: touched())
    }
}

final class TranscriptController: UIViewController, UICollectionViewDelegate {
    enum Item: Hashable {
        case header
        case row(String)
        case footer
    }

    /// The reader counts as at the end within this many points of it.
    private static let pinSlack: CGFloat = 48
    /// Earlier rows are asked for once the reader comes within this many points of the top.
    private static let earlierReach: CGFloat = 900
    private static let sidePadding: CGFloat = 18

    var makeRow: (RowEntry) -> AnyView = { _ in AnyView(EmptyView()) }
    var makeHeader: () -> AnyView = { AnyView(EmptyView()) }
    var makeFooter: () -> AnyView = { AnyView(EmptyView()) }
    var loadEarlier: () -> Void = {}
    var hasEarlier = false
    weak var scroll: TranscriptScroll?

    private var collection: UICollectionView!
    private var source: UICollectionViewDiffableDataSource<Int, Item>!
    private var order: [String] = []
    private var entries: [String: RowEntry] = [:]
    private var drawn = -1
    /// The reader is at the end, so the list follows what comes in.
    private var pinned = true
    /// The content's distance from its bottom to the top of the view, kept through a change above the rows in view.
    private var heldFromBottom: CGFloat?
    private var sizeWatch: NSKeyValueObservation?

    override func viewDidLoad() {
        super.viewDidLoad()
        var config = UICollectionLayoutListConfiguration(appearance: .plain)
        config.showsSeparators = false
        config.backgroundColor = .clear
        let layout = UICollectionViewCompositionalLayout.list(using: config)
        collection = UICollectionView(frame: view.bounds, collectionViewLayout: layout)
        collection.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        collection.backgroundColor = .clear
        collection.showsVerticalScrollIndicator = false
        collection.keyboardDismissMode = .interactive
        collection.contentInsetAdjustmentBehavior = .automatic
        collection.selfSizingInvalidation = .enabledIncludingConstraints
        collection.delegate = self
        collection.allowsSelection = false
        view.addSubview(collection)

        let row = UICollectionView.CellRegistration<UICollectionViewCell, String> { [weak self] cell, _, id in
            guard let self, let entry = entries[id] else { return }
            cell.contentConfiguration = UIHostingConfiguration { self.makeRow(entry) }
                .margins(.horizontal, Self.sidePadding).margins(.vertical, 0)
            cell.backgroundConfiguration = .clear()
        }
        let edge = UICollectionView.CellRegistration<UICollectionViewCell, Item> { [weak self] cell, _, item in
            guard let self else { return }
            let content = item == .header ? makeHeader() : makeFooter()
            cell.contentConfiguration = UIHostingConfiguration { content }
                .margins(.horizontal, Self.sidePadding).margins(.vertical, 0)
            cell.backgroundConfiguration = .clear()
        }
        source = UICollectionViewDiffableDataSource(collectionView: collection) { collection, path, item in
            switch item {
            case .row(let id): collection.dequeueConfiguredReusableCell(using: row, for: path, item: id)
            default: collection.dequeueConfiguredReusableCell(using: edge, for: path, item: item)
            }
        }
        sizeWatch = collection.observe(\.contentSize, options: [.old, .new]) { [weak self] _, change in
            MainActor.assumeIsolated { self?.sizeChanged(from: change.oldValue, to: change.newValue) }
        }
    }

    func update(order: [String], entries: [String: RowEntry], drawn: Int, touched: Touched) {
        let before = self.entries
        let reordered = order != self.order
        self.entries = entries
        guard isViewLoaded else { return }
        var snapshot: NSDiffableDataSourceSnapshot<Int, Item>
        if reordered || source.snapshot().numberOfSections == 0 {
            let prepended = !self.order.isEmpty && order.first != self.order.first && self.order.first.map(order.contains) == true
            if prepended { holdPlace() }
            self.order = order
            snapshot = NSDiffableDataSourceSnapshot()
            snapshot.appendSections([0])
            snapshot.appendItems([.header] + order.map(Item.row) + [.footer])
        } else {
            snapshot = source.snapshot()
        }
        let changed = order.filter { id in
            guard let old = before[id], let new = entries[id] else { return false }
            if old.look != new.look { return true }
            return (touched.all || touched.rows.contains(id)) && old.message != new.message
        }
        if !changed.isEmpty { snapshot.reconfigureItems(changed.map(Item.row)) }
        if touched.head || drawn != self.drawn { snapshot.reconfigureItems([.header, .footer]) }
        self.drawn = drawn
        source.apply(snapshot, animatingDifferences: false)
        if pinned { stickToEnd() }
    }

    // MARK: Following the end

    private var bottomOffset: CGFloat {
        let inset = collection.adjustedContentInset
        return max(-inset.top, collection.contentSize.height - collection.bounds.height + inset.bottom)
    }

    private func stickToEnd() {
        collection.layoutIfNeeded()
        let bottom = bottomOffset
        if abs(collection.contentOffset.y - bottom) > 0.5 { collection.contentOffset.y = bottom }
    }

    func toEnd(animated: Bool) {
        pinned = true
        scroll?.away = false
        collection.layoutIfNeeded()
        collection.setContentOffset(CGPoint(x: 0, y: bottomOffset), animated: animated)
    }

    func reveal(_ id: String) {
        guard let path = source.indexPath(for: .row(id)) else { return }
        pinned = false
        collection.scrollToItem(at: path, at: .top, animated: true)
    }

    func holdPlace() {
        guard !pinned else { return }
        heldFromBottom = collection.contentSize.height - collection.contentOffset.y
    }

    private func sizeChanged(from old: CGSize?, to new: CGSize?) {
        guard old != new else { return }
        if let held = heldFromBottom {
            heldFromBottom = nil
            collection.contentOffset.y = (new?.height ?? 0) - held
        } else if pinned, !collection.isDragging, !collection.isDecelerating {
            let bottom = bottomOffset
            if abs(collection.contentOffset.y - bottom) > 0.5 { collection.contentOffset.y = bottom }
        }
    }

    override func viewDidLayoutSubviews() {
        super.viewDidLayoutSubviews()
        if pinned { stickToEnd() }
    }

    // MARK: The reader scrolling

    func scrollViewDidScroll(_ scrollView: UIScrollView) {
        let fromEnd = bottomOffset - scrollView.contentOffset.y
        if scrollView.isDragging || scrollView.isDecelerating {
            pinned = fromEnd < Self.pinSlack
        }
        let away = !pinned && fromEnd > Self.pinSlack
        if scroll?.away != away { scroll?.away = away }
        if hasEarlier, scrollView.contentOffset.y + scrollView.adjustedContentInset.top < Self.earlierReach {
            loadEarlier()
        }
        noteTopRow()
    }

    func scrollViewDidEndScrollingAnimation(_ scrollView: UIScrollView) {
        if bottomOffset - scrollView.contentOffset.y < Self.pinSlack { pinned = true }
    }

    private func noteTopRow() {
        let top = collection.indexPathsForVisibleItems.min()
        guard let top, case .row(let id) = source.itemIdentifier(for: top) else { return }
        if scroll?.topRow != id { scroll?.topRow = id }
    }
}
