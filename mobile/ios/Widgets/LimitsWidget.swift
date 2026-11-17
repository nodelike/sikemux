import SwiftUI
import WidgetKit

/// How much of each agent account's plan is used, as the Mac's Limits panel lists it.
struct LimitsWidget: Widget {
    var body: some WidgetConfiguration {
        StaticConfiguration(kind: GlanceStore.limitsWidget, provider: GlanceTimeline()) { entry in
            LimitsView(limits: entry.glance?.limits ?? [])
                .containerBackground(palette.sunken, for: .widget)
        }
        .configurationDisplayName("Limits")
        .description("How much of each agent account's plan is used, and when it resets.")
        .supportedFamilies([.systemSmall, .systemMedium, .systemLarge])
    }
}

private struct LimitsView: View {
    let limits: [LimitGlance]
    @Environment(\.widgetFamily) private var family

    var body: some View {
        if family == .systemSmall, let tightest = limits.max(by: { $0.used < $1.used }) {
            Tightest(limit: tightest)
        } else {
            VStack(alignment: .leading, spacing: 0) {
                HStack(spacing: 6) {
                    BrandMark(size: 12)
                    Text("Limits").font(Theme.ui(11.5)).foregroundStyle(palette.tertiary)
                }
                .padding(.bottom, 6)
                if limits.isEmpty {
                    Text("Open Sikemux to read your plan limits")
                        .font(Theme.ui(12)).foregroundStyle(palette.secondary)
                }
                ForEach(limits.prefix(family == .systemLarge ? 12 : 4)) { limit in
                    LimitLine(limit: limit).frame(height: 25)
                }
                Spacer(minLength: 0)
            }
        }
    }
}

private struct LimitLine: View {
    let limit: LimitGlance

    var body: some View {
        HStack(spacing: 7) {
            AgentMark(provider: limit.provider, size: 14)
            Text("\(limit.account) · \(limit.window)")
                .font(Theme.ui(12)).foregroundStyle(palette.secondary).lineLimit(1)
                .frame(width: 110, alignment: .leading)
            Meter(used: limit.used)
            Text(percent(limit.used)).font(Theme.ui(12)).monospacedDigit().foregroundStyle(palette.ink)
                .frame(width: 34, alignment: .trailing)
            ResetsIn(at: limit.resetsAt).frame(width: 34, alignment: .trailing)
        }
        .opacity(limit.stale ? 0.5 : 1)
    }
}

private struct Tightest: View {
    let limit: LimitGlance

    var body: some View {
        VStack(spacing: 6) {
            HStack(spacing: 6) {
                AgentMark(provider: limit.provider, size: 12)
                Text([limit.account, limit.plan].compactMap { $0 }.joined(separator: " · "))
                    .font(Theme.ui(11.5)).foregroundStyle(palette.tertiary).lineLimit(1)
                Spacer(minLength: 0)
            }
            ZStack {
                Circle().stroke(palette.border, lineWidth: 6)
                Circle().trim(from: 0, to: min(limit.used, 100) / 100)
                    .stroke(Meter.colour(limit.used), style: StrokeStyle(lineWidth: 6, lineCap: .round))
                    .rotationEffect(.degrees(-90))
                Text(percent(limit.used)).font(Theme.ui(17, .semibold)).foregroundStyle(palette.ink)
            }
            .frame(width: 76, height: 76)
            HStack(spacing: 3) {
                Text(limit.window)
                ResetsIn(at: limit.resetsAt)
            }
            .font(Theme.ui(11)).foregroundStyle(palette.tertiary).lineLimit(1)
        }
        .opacity(limit.stale ? 0.5 : 1)
    }
}

private struct Meter: View {
    let used: Double

    /// Amber past 80%, red once it is used up, as the Mac's ruler turns.
    static func colour(_ used: Double) -> Color {
        used >= 100 ? palette.danger : used >= 80 ? palette.warn : palette.ink
    }

    var body: some View {
        GeometryReader { space in
            ZStack(alignment: .leading) {
                Capsule().fill(palette.border)
                Capsule().fill(Self.colour(used)).frame(width: space.size.width * min(max(used, 0), 100) / 100)
            }
        }
        .frame(height: 5)
    }
}

private func percent(_ used: Double) -> String { "\(Int(used.rounded()))%" }

/// How long until a window resets, in a tight column: 40m, 2h, 4d.
private struct ResetsIn: View {
    let at: Date?

    var body: some View {
        Text(at.map { Format.age(.now, now: $0) } ?? "").font(Theme.ui(12)).foregroundStyle(palette.tertiary)
    }
}
