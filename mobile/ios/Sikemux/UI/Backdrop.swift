import Observation
import SwiftUI

/// Stills the backdrop while something covers it or the screen moves: a sheet, a scrolling list, a running chat.
@MainActor @Observable
final class BackdropPause {
    static let shared = BackdropPause()

    private(set) var holds = 0

    var paused: Bool { holds > 0 }

    /// Stills the backdrop until the returned function is called.
    func hold() -> () -> Void {
        holds += 1
        var released = false
        return { [weak self] in
            guard !released else { return }
            released = true
            self?.holds -= 1
        }
    }
}

extension View {
    /// Stills the backdrop while this scroll view moves.
    func pausesBackdrop() -> some View {
        modifier(ScrollPause())
    }
}

private struct ScrollPause: ViewModifier {
    @State private var release: (() -> Void)?

    func body(content: Content) -> some View {
        if #available(iOS 18, *) {
            content
                .onScrollPhaseChange { _, phase in
                    if phase == .idle {
                        release?()
                        release = nil
                    } else if release == nil {
                        release = BackdropPause.shared.hold()
                    }
                }
                .onDisappear {
                    release?()
                    release = nil
                }
        } else {
            content
        }
    }
}

/// The Mac's pane backdrop at the top of a host's screens: its moving dithered grain, or its picture dithered in
/// place, faded out down the screen as on the Mac. It draws only when it changes: the grain ticks while the screen
/// shows and nothing stills it, and the picture draws once.
struct PaneBackdrop: View {
    @Environment(\.palette) private var palette
    @Environment(\.accessibilityReduceMotion) private var still
    @Environment(\.scenePhase) private var phase
    @Environment(\.displayScale) private var scale
    let texture: Bool
    /// The host's picture, saved on the phone; nil draws the grain.
    let image: URL?
    @State private var showing = false
    @State private var shown = false
    @State private var picture: Picture?

    /// Every backdrop reads one clock, so the grain carries on across screens instead of restarting.
    private static let origin = Date()
    /// The Mac ticks its grain at 30 fps, 20 on battery; the phone ticks slower still.
    private static let tick: TimeInterval = 0.08
    private static let speed = 0.5
    /// Where the grain rests when motion is reduced, as on the Mac.
    private static let stillAt = 2.5
    private static var revealed: Set<String> = []
    /// The picture is drawn no larger than this many pixels across, which is wider than any phone.
    nonisolated private static let widest: CGFloat = 1600

    private struct Picture {
        let file: URL
        let image: UIImage
        let aspect: Float
    }

    var body: some View {
        if texture, image == nil || picture != nil {
            let band = picture == nil ? 0.5 : 0.6
            GeometryReader { frame in
                let size = CGSize(width: frame.size.width, height: frame.size.height * band)
                ZStack(alignment: .top) {
                    field(size: size, picture: picture)
                        .opacity(shown ? (picture == nil ? 0.85 : 0.38) : 0)
                    LinearGradient(
                        stops: [
                            .init(color: palette.ground.opacity(0), location: 0),
                            .init(color: palette.ground.opacity(0.55), location: 0.4),
                            .init(color: palette.ground.opacity(0.92), location: 0.76),
                            .init(color: palette.ground, location: 1),
                        ], startPoint: .top, endPoint: .bottom)
                }
                .frame(width: size.width, height: size.height)
                .clipped()
            }
            .ignoresSafeArea()
            .allowsHitTesting(false)
            .accessibilityHidden(true)
            .onAppear {
                showing = true
                reveal(picture?.file.lastPathComponent ?? "grain")
            }
            .onDisappear { showing = false }
        }
        Color.clear.frame(width: 0, height: 0)
            .task(id: image) {
                guard let image, picture?.file != image else { return }
                let made = await Task.detached { Self.downscaled(image) }.value
                picture = made.map { Picture(file: image, image: $0, aspect: Float($0.size.width / $0.size.height)) }
            }
    }

    nonisolated private static func downscaled(_ file: URL) -> UIImage? {
        guard let source = CGImageSourceCreateWithURL(file as CFURL, nil) else { return nil }
        let options: [CFString: Any] = [
            kCGImageSourceCreateThumbnailFromImageAlways: true,
            kCGImageSourceCreateThumbnailWithTransform: true,
            kCGImageSourceThumbnailMaxPixelSize: widest,
        ]
        return CGImageSourceCreateThumbnailAtIndex(source, 0, options as CFDictionary).map { UIImage(cgImage: $0) }
    }

    @ViewBuilder private func field(size: CGSize, picture: Picture?) -> some View {
        if let picture {
            Rectangle()
                .colorEffect(
                    ShaderLibrary.backdropPicture(
                        .float2(size), .float(scale), .float(picture.aspect), .image(Image(uiImage: picture.image)))
                )
                .frame(width: size.width, height: size.height)
        } else {
            let moving = showing && phase == .active && !still && !BackdropPause.shared.paused
            TimelineView(.animation(minimumInterval: Self.tick, paused: !moving)) { _ in
                Rectangle()
                    .colorEffect(
                        ShaderLibrary.backdropGrain(
                            .float2(size), .float(scale), .float(time), .color(palette.shaderDot)))
            }
            .frame(width: size.width, height: size.height)
        }
    }

    private var time: Float {
        Float((still ? Self.stillAt : Date().timeIntervalSince(Self.origin)) * Self.speed)
    }

    private func reveal(_ key: String) {
        if still || Self.revealed.contains(key) {
            shown = true
        } else {
            withAnimation(.easeOut(duration: 0.4)) { shown = true }
        }
        Self.revealed.insert(key)
    }
}
