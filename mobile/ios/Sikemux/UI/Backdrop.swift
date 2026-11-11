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

extension BackdropLook {
    /// The Mac's own look, for a host that does not say.
    static let standard = BackdropLook(
        strength: 1, inks: [], ownColours: false, pattern: .bayer8, dotSize: 2, contrast: 0, brightness: 0, glow: 0.4,
        glowSize: 0.4, coverage: .band)
}

/// The Mac's pane backdrop at the top of a host's screens: its moving dithered grain, or its picture dithered in
/// place as the host's look says, faded out down the screen as on the Mac. It draws only when it changes: the grain
/// ticks while the screen shows and nothing stills it, and the picture draws once.
struct PaneBackdrop: View {
    @Environment(\.palette) private var palette
    @Environment(\.accessibilityReduceMotion) private var still
    @Environment(\.scenePhase) private var phase
    @Environment(\.displayScale) private var scale
    /// The host's picture, saved on the phone; nil draws the grain.
    let image: URL?
    let look: BackdropLook
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
    /// As on the Mac: the grain's dots are its only paint, so it shows strongly, and the picture less so.
    private static let grainOpacity = 0.85
    private static let pictureOpacity = 0.6
    /// How much of the screen, from its top, the picture covers when it does not cover it all.
    private static let pictureBand = 0.6
    private static var revealed: Set<String> = []

    private struct Picture {
        let file: URL
        let glowSize: Double
        /// The picture over its glow, one above the other.
        let atlas: UIImage
        let aspect: Float
    }

    var body: some View {
        if image == nil || picture != nil {
            GeometryReader { frame in
                let band = picture == nil ? 0.5 : look.coverage == .full ? 1 : Self.pictureBand
                let size = CGSize(width: frame.size.width, height: frame.size.height * band)
                ZStack(alignment: .top) {
                    if let picture {
                        pictureField(size: size, picture: picture).opacity(shown ? 1 : 0)
                    } else {
                        grain(size: size).opacity(shown ? min(Self.grainOpacity * look.strength, 1) : 0)
                        LinearGradient(
                            stops: [
                                .init(color: palette.ground.opacity(0), location: 0),
                                .init(color: palette.ground.opacity(0.55), location: 0.4),
                                .init(color: palette.ground.opacity(0.92), location: 0.76),
                                .init(color: palette.ground, location: 1),
                            ], startPoint: .top, endPoint: .bottom)
                    }
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
            .task(id: PictureKey(file: image, glowSize: look.glowSize)) {
                guard let image else { return }
                if picture?.file == image, picture?.glowSize == look.glowSize { return }
                let glowSize = look.glowSize
                let made = await Task.detached { PictureBake.atlas(of: image, glowSize: glowSize) }.value
                picture = made.map { Picture(file: image, glowSize: glowSize, atlas: $0.image, aspect: $0.aspect) }
            }
    }

    private struct PictureKey: Hashable {
        let file: URL?
        let glowSize: Double
    }

    private func pictureField(size: CGSize, picture: Picture) -> some View {
        let inks = (0..<3).map { at -> Color in
            if look.inks.isEmpty { return [nil, palette.accent, palette.ink][at] ?? .clear }
            guard at < look.inks.count, let hex = look.inks[at], let (r, g, b, _) = Color.channels(of: hex) else {
                return .clear
            }
            return Color(.sRGB, red: r, green: g, blue: b, opacity: 1)
        }
        let pattern: Float =
            switch look.pattern {
            case .bayer2: 2
            case .bayer4: 4
            case .noise: 0
            case .bayer8, .unknown: 8
            }
        return Rectangle()
            .colorEffect(
                ShaderLibrary.backdropPicture(
                    .float2(size), .float(picture.aspect), .float(Float(look.dotSize)), .float(pattern),
                    .float(Float(look.contrast)), .float(Float(look.brightness)), .float(Float(look.glow)),
                    .float(look.ownColours ? 1 : 0), .float(look.coverage == .full ? 1 : 0),
                    .float(Float(min(Self.pictureOpacity * look.strength, 1))), .color(inks[0]), .color(inks[1]),
                    .color(inks[2]), .image(Image(uiImage: picture.atlas)))
            )
            .frame(width: size.width, height: size.height)
    }

    private func grain(size: CGSize) -> some View {
        let moving = showing && phase == .active && !still && !BackdropPause.shared.paused
        return TimelineView(.animation(minimumInterval: Self.tick, paused: !moving)) { _ in
            Rectangle()
                .colorEffect(
                    ShaderLibrary.backdropGrain(.float2(size), .float(scale), .float(time), .color(palette.shaderDot)))
        }
        .frame(width: size.width, height: size.height)
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

/// The host's picture made ready for the shader, as the Mac makes it: the picture above, and below it how much each
/// place glows, worked out on a small copy as a blur of its bright parts.
enum PictureBake {
    /// The picture is drawn no larger than this many pixels across, which is wider than any phone.
    private static let widest: CGFloat = 1600
    private static let glowLongest = 320.0
    /// How bright a place must be, from 0 to 1, before it glows.
    private static let glowThreshold = 0.45

    static func atlas(of file: URL, glowSize: Double) -> (image: UIImage, aspect: Float)? {
        guard let picture = UIImage.thumbnail(of: file, pixels: widest)?.cgImage else { return nil }
        let width = picture.width
        let height = picture.height
        guard let glow = glow(of: picture, size: glowSize),
            let atlas = CGContext(
                data: nil, width: width, height: height * 2, bitsPerComponent: 8, bytesPerRow: 0,
                space: CGColorSpace(name: CGColorSpace.sRGB)!, bitmapInfo: CGImageAlphaInfo.noneSkipLast.rawValue)
        else { return nil }
        atlas.setFillColor(gray: 0, alpha: 1)
        atlas.fill(CGRect(x: 0, y: 0, width: width, height: height * 2))
        atlas.draw(picture, in: CGRect(x: 0, y: height, width: width, height: height))
        atlas.interpolationQuality = .medium
        atlas.draw(glow, in: CGRect(x: 0, y: 0, width: width, height: height))
        guard let made = atlas.makeImage() else { return nil }
        return (UIImage(cgImage: made), Float(width) / Float(height))
    }

    private static func glow(of picture: CGImage, size: Double) -> CGImage? {
        let shrink = glowLongest / Double(max(picture.width, picture.height))
        let width = max(Int((Double(picture.width) * shrink).rounded()), 1)
        let height = max(Int((Double(picture.height) * shrink).rounded()), 1)
        var pixels = [UInt8](repeating: 0, count: width * height * 4)
        let drawn = pixels.withUnsafeMutableBytes { bytes -> Bool in
            guard
                let small = CGContext(
                    data: bytes.baseAddress, width: width, height: height, bitsPerComponent: 8,
                    bytesPerRow: width * 4, space: CGColorSpace(name: CGColorSpace.sRGB)!,
                    bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)
            else { return false }
            small.interpolationQuality = .medium
            small.draw(picture, in: CGRect(x: 0, y: 0, width: width, height: height))
            return true
        }
        guard drawn else { return nil }
        let bright = (0..<width * height).map { at -> UInt8 in
            let lum =
                (0.2126 * Double(pixels[at * 4]) + 0.7152 * Double(pixels[at * 4 + 1])
                    + 0.0722 * Double(pixels[at * 4 + 2])) / 255
            return UInt8(max((lum - glowThreshold) / (1 - glowThreshold), 0) * 255)
        }
        let spread = 1 + size * 10
        let near = blurred(bright, width: width, height: height, sigma: spread)
        let far = blurred(bright, width: width, height: height, sigma: spread * 3)
        let glow = zip(near, far).map { UInt8(min(Double($0) * 1.2 + Double($1) * 0.8, 255)) }
        guard let provider = CGDataProvider(data: Data(glow) as CFData) else { return nil }
        return CGImage(
            width: width, height: height, bitsPerComponent: 8, bitsPerPixel: 8, bytesPerRow: width,
            space: CGColorSpaceCreateDeviceGray(), bitmapInfo: CGBitmapInfo(rawValue: CGImageAlphaInfo.none.rawValue),
            provider: provider, decode: nil, shouldInterpolate: true, intent: .defaultIntent)
    }

    /// A Gaussian blur made of three box blurs, as the Mac's `fast_blur` makes it.
    private static func blurred(_ pixels: [UInt8], width: Int, height: Int, sigma: Double) -> [UInt8] {
        let passes = 3.0
        var lower = (12 * sigma * sigma / passes + 1).squareRoot().rounded(.down)
        if lower.truncatingRemainder(dividingBy: 2) == 0 { lower -= 1 }
        let upper = lower + 2
        let split = max(Int((0.25 * passes * (lower + 3) - 3 * sigma * sigma / (lower + 1)).rounded()), 0)
        let radii = (0..<Int(passes)).map { pass -> Int in
            let half = max(Int(pass < split ? lower : upper) - 1, 0) / 2
            return half % 2 == 0 ? half + 1 : half
        }
        var out = pixels
        var across = pixels
        for radius in radii {
            box(out, into: &across, count: height, length: width, step: 1, stride: width, radius: radius)
            box(across, into: &out, count: width, length: height, step: width, stride: 1, radius: radius)
        }
        return out
    }

    /// Averages each run of `length` pixels over `radius` either side, holding the run's end pixels past its ends.
    private static func box(
        _ source: [UInt8], into target: inout [UInt8], count: Int, length: Int, step: Int, stride: Int, radius: Int
    ) {
        let span = Double(2 * radius + 1)
        for run in 0..<count {
            let first = run * stride
            let at = { (index: Int) in Int(source[first + min(max(index, 0), length - 1) * step]) }
            var sum = (-radius...radius).reduce(0) { $0 + at($1) }
            for index in 0..<length {
                target[first + index * step] = UInt8((Double(sum) / span).rounded())
                sum += at(index + radius + 1) - at(index - radius)
            }
        }
    }
}
