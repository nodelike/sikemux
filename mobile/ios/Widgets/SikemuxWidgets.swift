import SwiftUI
import WidgetKit

@main
struct SikemuxWidgets: WidgetBundle {
    init() { Theme.registerFonts() }

    var body: some Widget {
        NeedsYouWidget()
        LimitsWidget()
        AgentsLiveActivity()
    }
}

/// The glance as the app last left it, read again whenever the app says it changed.
struct GlanceTimeline: TimelineProvider {
    struct Entry: TimelineEntry {
        let date: Date
        let glance: Glance?
    }

    /// Drawn again this often by itself, so the time until each limit resets keeps up.
    private let redrawEvery: TimeInterval = 15 * 60

    func placeholder(in context: Context) -> Entry { Entry(date: .now, glance: nil) }

    func getSnapshot(in context: Context, completion: @escaping (Entry) -> Void) {
        completion(Entry(date: .now, glance: GlanceStore.read()))
    }

    func getTimeline(in context: Context, completion: @escaping (Timeline<Entry>) -> Void) {
        let redraw = Date.now.addingTimeInterval(redrawEvery)
        completion(Timeline(entries: [Entry(date: .now, glance: GlanceStore.read())], policy: .after(redraw)))
    }
}
