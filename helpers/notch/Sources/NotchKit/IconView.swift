import SwiftUI

struct ViewBox {
    let x: CGFloat
    let y: CGFloat
    let width: CGFloat
    let height: CGFloat
}

struct GradientStop {
    let offset: CGFloat
    let hex: String
}

enum IconPaint {
    case current
    case hex(String)
    case gradient([GradientStop], start: (CGFloat, CGFloat), end: (CGFloat, CGFloat))
}

enum IconStyle {
    case stroke
    case fill(IconPaint)
}

struct IconShape {
    let d: String
    let style: IconStyle
    let opacity: Double
    let evenOdd: Bool
    let translate: (CGFloat, CGFloat)
}

struct IconDef {
    let viewBox: ViewBox
    let shapes: [IconShape]
}

/// One of the app's icons at `size` points, stroked like the app's: 1.4 units
/// on its 16-unit grid, with round caps and joins.
struct IconView: View {
    let icon: IconDef
    var size: CGFloat = 14

    var body: some View {
        ZStack {
            ForEach(Array(icon.shapes.enumerated()), id: \.offset) { _, shape in
                layer(shape)
            }
        }
        .frame(width: size, height: size)
    }

    @ViewBuilder
    private func layer(_ shape: IconShape) -> some View {
        let path = SVGPathShape(d: shape.d, viewBox: icon.viewBox, translate: shape.translate)
        switch shape.style {
        case .stroke:
            path.stroke(style: StrokeStyle(lineWidth: 1.4 * size / icon.viewBox.width, lineCap: .round, lineJoin: .round))
                .opacity(shape.opacity)
        case .fill(let paint):
            filled(path, paint: paint, evenOdd: shape.evenOdd).opacity(shape.opacity)
        }
    }

    @ViewBuilder
    private func filled(_ path: SVGPathShape, paint: IconPaint, evenOdd: Bool) -> some View {
        let style = FillStyle(eoFill: evenOdd)
        switch paint {
        case .current:
            path.fill(style: style)
        case .hex(let hex):
            path.fill(Color(hex: hex), style: style)
        case .gradient(let stops, let start, let end):
            path.fill(
                LinearGradient(
                    stops: stops.map { Gradient.Stop(color: Color(hex: $0.hex), location: $0.offset) },
                    startPoint: UnitPoint(x: start.0, y: start.1),
                    endPoint: UnitPoint(x: end.0, y: end.1)
                ),
                style: style
            )
        }
    }
}

/// An agent's mark, or the generic agent sparkle for one the app has no mark for.
struct AgentMark: View {
    let provider: String
    var size: CGFloat = 16

    var body: some View {
        if let mark = Icons.mark(provider) {
            IconView(icon: mark, size: size)
        } else {
            Circle().fill(Theme.inkDim).frame(width: size * 0.5, height: size * 0.5).frame(width: size, height: size)
        }
    }
}

struct SVGPathShape: Shape {
    let d: String
    let viewBox: ViewBox
    let translate: (CGFloat, CGFloat)

    func path(in rect: CGRect) -> Path {
        let scale = min(rect.width / viewBox.width, rect.height / viewBox.height)
        let transform = CGAffineTransform(translationX: rect.minX, y: rect.minY)
            .scaledBy(x: scale, y: scale)
            .translatedBy(x: -viewBox.x + translate.0, y: -viewBox.y + translate.1)
        return SVGPath.parse(d).applying(transform)
    }
}

/// Reads SVG path data into a `Path`: every command, absolute and relative,
/// with arcs turned into curves.
enum SVGPath {
    private static var cache: [String: Path] = [:]

    static func parse(_ d: String) -> Path {
        if let known = cache[d] { return known }
        var reader = Reader(Array(d.utf8))
        var path = Path()
        var current = CGPoint.zero
        var start = CGPoint.zero
        var lastControl: CGPoint?
        var lastQuad: CGPoint?
        var command: UInt8 = 0
        while true {
            reader.skipSeparators()
            guard let byte = reader.peek() else { break }
            if Reader.isCommand(byte) {
                command = byte
                reader.advance()
            } else if command == 0 {
                break
            }
            let relative = command >= 97
            let base = relative ? current : .zero
            func point() -> CGPoint? {
                guard let x = reader.number(), let y = reader.number() else { return nil }
                return CGPoint(x: base.x + x, y: base.y + y)
            }
            var curveControl: CGPoint?
            var quadControl: CGPoint?
            switch command | 0x20 {
            case UInt8(ascii: "m"):
                guard let p = point() else { return path }
                path.move(to: p)
                current = p
                start = p
                command = relative ? UInt8(ascii: "l") : UInt8(ascii: "L")
            case UInt8(ascii: "l"):
                guard let p = point() else { return path }
                path.addLine(to: p)
                current = p
            case UInt8(ascii: "h"):
                guard let x = reader.number() else { return path }
                current = CGPoint(x: (relative ? current.x : 0) + x, y: current.y)
                path.addLine(to: current)
            case UInt8(ascii: "v"):
                guard let y = reader.number() else { return path }
                current = CGPoint(x: current.x, y: (relative ? current.y : 0) + y)
                path.addLine(to: current)
            case UInt8(ascii: "c"):
                guard let c1 = point(), let c2 = point(), let p = point() else { return path }
                path.addCurve(to: p, control1: c1, control2: c2)
                curveControl = c2
                current = p
            case UInt8(ascii: "s"):
                guard let c2 = point(), let p = point() else { return path }
                let c1 = lastControl.map { CGPoint(x: 2 * current.x - $0.x, y: 2 * current.y - $0.y) } ?? current
                path.addCurve(to: p, control1: c1, control2: c2)
                curveControl = c2
                current = p
            case UInt8(ascii: "q"):
                guard let c = point(), let p = point() else { return path }
                path.addQuadCurve(to: p, control: c)
                quadControl = c
                current = p
            case UInt8(ascii: "t"):
                guard let p = point() else { return path }
                let c = lastQuad.map { CGPoint(x: 2 * current.x - $0.x, y: 2 * current.y - $0.y) } ?? current
                path.addQuadCurve(to: p, control: c)
                quadControl = c
                current = p
            case UInt8(ascii: "a"):
                guard let rx = reader.number(), let ry = reader.number(), let rotation = reader.number(),
                      let large = reader.flag(), let sweep = reader.flag(), let p = point()
                else { return path }
                addArc(&path, from: current, to: p, rx: rx, ry: ry, rotation: rotation, large: large, sweep: sweep)
                current = p
            case UInt8(ascii: "z"):
                path.closeSubpath()
                current = start
            default:
                return path
            }
            lastControl = curveControl
            lastQuad = quadControl
        }
        cache[d] = path
        return path
    }

    /// The endpoint arc of the SVG spec, as cubic curves of at most a quarter turn.
    private static func addArc(
        _ path: inout Path, from p0: CGPoint, to p1: CGPoint,
        rx: CGFloat, ry: CGFloat, rotation: CGFloat, large: Bool, sweep: Bool
    ) {
        var rx = abs(rx)
        var ry = abs(ry)
        guard rx > 0, ry > 0, p0 != p1 else {
            path.addLine(to: p1)
            return
        }
        let phi = rotation * .pi / 180
        let cosPhi = cos(phi)
        let sinPhi = sin(phi)
        let dx = (p0.x - p1.x) / 2
        let dy = (p0.y - p1.y) / 2
        let x1 = cosPhi * dx + sinPhi * dy
        let y1 = -sinPhi * dx + cosPhi * dy
        let lambda = (x1 * x1) / (rx * rx) + (y1 * y1) / (ry * ry)
        if lambda > 1 {
            rx *= sqrt(lambda)
            ry *= sqrt(lambda)
        }
        let numerator = rx * rx * ry * ry - rx * rx * y1 * y1 - ry * ry * x1 * x1
        let denominator = rx * rx * y1 * y1 + ry * ry * x1 * x1
        var coefficient = sqrt(max(0, numerator / denominator))
        if large == sweep { coefficient = -coefficient }
        let cx1 = coefficient * rx * y1 / ry
        let cy1 = -coefficient * ry * x1 / rx
        let cx = cosPhi * cx1 - sinPhi * cy1 + (p0.x + p1.x) / 2
        let cy = sinPhi * cx1 + cosPhi * cy1 + (p0.y + p1.y) / 2
        func angle(_ ux: CGFloat, _ uy: CGFloat, _ vx: CGFloat, _ vy: CGFloat) -> CGFloat {
            let sign: CGFloat = ux * vy - uy * vx < 0 ? -1 : 1
            let dot = ux * vx + uy * vy
            let length = sqrt(ux * ux + uy * uy) * sqrt(vx * vx + vy * vy)
            return sign * acos(max(-1, min(1, dot / length)))
        }
        let theta = angle(1, 0, (x1 - cx1) / rx, (y1 - cy1) / ry)
        var delta = angle((x1 - cx1) / rx, (y1 - cy1) / ry, (-x1 - cx1) / rx, (-y1 - cy1) / ry)
        if !sweep, delta > 0 { delta -= 2 * .pi }
        if sweep, delta < 0 { delta += 2 * .pi }
        let segments = Int(ceil(abs(delta) / (.pi / 2)))
        let step = delta / CGFloat(segments)
        let kappa = 4 / 3 * tan(step / 4)
        var t = theta
        func on(_ angle: CGFloat) -> CGPoint {
            CGPoint(
                x: cx + rx * cos(angle) * cosPhi - ry * sin(angle) * sinPhi,
                y: cy + rx * cos(angle) * sinPhi + ry * sin(angle) * cosPhi
            )
        }
        func tangent(_ angle: CGFloat) -> CGPoint {
            CGPoint(
                x: -rx * sin(angle) * cosPhi - ry * cos(angle) * sinPhi,
                y: -rx * sin(angle) * sinPhi + ry * cos(angle) * cosPhi
            )
        }
        for _ in 0..<segments {
            let a = on(t)
            let b = on(t + step)
            let ta = tangent(t)
            let tb = tangent(t + step)
            path.addCurve(
                to: b,
                control1: CGPoint(x: a.x + kappa * ta.x, y: a.y + kappa * ta.y),
                control2: CGPoint(x: b.x - kappa * tb.x, y: b.y - kappa * tb.y)
            )
            t += step
        }
    }

    private struct Reader {
        let bytes: [UInt8]
        var index = 0

        init(_ bytes: [UInt8]) { self.bytes = bytes }

        static func isCommand(_ byte: UInt8) -> Bool {
            "MmLlHhVvCcSsQqTtAaZz".utf8.contains(byte)
        }

        func peek() -> UInt8? { index < bytes.count ? bytes[index] : nil }

        mutating func advance() { index += 1 }

        mutating func skipSeparators() {
            while let byte = peek(), byte == 32 || byte == 44 || byte == 10 || byte == 13 || byte == 9 {
                index += 1
            }
        }

        /// Arc flags are single digits and may run straight into the next number.
        mutating func flag() -> Bool? {
            skipSeparators()
            guard let byte = peek(), byte == 48 || byte == 49 else { return nil }
            index += 1
            return byte == 49
        }

        mutating func number() -> CGFloat? {
            skipSeparators()
            let begin = index
            if let byte = peek(), byte == 43 || byte == 45 { index += 1 }
            var seenDot = false
            var seenDigit = false
            while let byte = peek() {
                if byte >= 48 && byte <= 57 {
                    seenDigit = true
                    index += 1
                } else if byte == 46 && !seenDot {
                    seenDot = true
                    index += 1
                } else if (byte == 101 || byte == 69) && seenDigit {
                    index += 1
                    if let sign = peek(), sign == 43 || sign == 45 { index += 1 }
                } else {
                    break
                }
            }
            guard seenDigit, let text = String(bytes: bytes[begin..<index], encoding: .ascii) else {
                index = begin
                return nil
            }
            return Double(text).map { CGFloat($0) }
        }
    }
}
