import Foundation
import SwiftUI

/// A buddy's SVG as shapes SwiftUI can draw: the subset the shared buddies use (paths, circles, ellipses, rects,
/// groups, clip paths and bounding-box gradients). iOS cannot read SVG into an image, so the phone draws them with this.
struct SVGDrawing: @unchecked Sendable {
    enum Paint {
        case color(Color)
        case linear(Gradient, start: CGPoint, end: CGPoint)
        case radial(Gradient, center: CGPoint, radius: CGFloat)
    }

    struct Item {
        var path: Path
        var fill: Paint?
        var stroke: Paint?
        var style: StrokeStyle
        var opacity: Double
        var fillOpacity: Double
        var clip: Path?
    }

    let viewBox: CGRect
    let items: [Item]

    init?(_ svg: String) {
        let reader = Reader()
        let parser = XMLParser(data: Data(svg.utf8))
        parser.delegate = reader
        guard parser.parse(), let box = reader.viewBox else { return nil }
        viewBox = box
        items = reader.items
    }

    func draw(in context: inout GraphicsContext, size: CGSize) {
        let scale = min(size.width / viewBox.width, size.height / viewBox.height)
        let fit = CGAffineTransform(translationX: -viewBox.minX, y: -viewBox.minY)
            .concatenating(CGAffineTransform(scaleX: scale, y: scale))
        for item in items {
            var layer = context
            layer.opacity = item.opacity
            if let clip = item.clip { layer.clip(to: clip.applying(fit)) }
            let path = item.path.applying(fit)
            let bounds = path.boundingRect
            if let fill = item.fill {
                var filling = layer
                filling.opacity = item.opacity * item.fillOpacity
                filling.fill(path, with: shading(fill, bounds))
            }
            if let stroke = item.stroke {
                var style = item.style
                style.lineWidth *= scale
                style.dash = style.dash.map { $0 * scale }
                layer.stroke(path, with: shading(stroke, bounds), style: style)
            }
        }
    }

    private func shading(_ paint: Paint, _ bounds: CGRect) -> GraphicsContext.Shading {
        func at(_ unit: CGPoint) -> CGPoint {
            CGPoint(x: bounds.minX + unit.x * bounds.width, y: bounds.minY + unit.y * bounds.height)
        }
        switch paint {
        case .color(let color): return .color(color)
        case .linear(let gradient, let start, let end): return .linearGradient(gradient, startPoint: at(start), endPoint: at(end))
        case .radial(let gradient, let center, let radius):
            return .radialGradient(gradient, center: at(center), startRadius: 0, endRadius: radius * max(bounds.width, bounds.height))
        }
    }
}

/// An SVG document drawn to fill its frame.
struct SVGView: View {
    let drawing: SVGDrawing

    var body: some View {
        Canvas { context, size in drawing.draw(in: &context, size: size) }
    }
}

private final class Reader: NSObject, XMLParserDelegate {
    struct Group {
        var fill: String?
        var stroke: String?
        var strokeWidth: String?
        var opacity: Double = 1
        var clip: Path?
        var transform: CGAffineTransform = .identity
    }

    var viewBox: CGRect?
    var items: [SVGDrawing.Item] = []
    private var groups: [Group] = [Group()]
    private var gradients: [String: SVGDrawing.Paint] = [:]
    private var clips: [String: Path] = [:]
    private var gradient: (id: String, attributes: [String: String], stops: [Gradient.Stop])?
    private var clipping: (id: String, path: Path)?

    func parser(_ parser: XMLParser, didStartElement name: String, namespaceURI: String?, qualifiedName: String?, attributes: [String: String] = [:]) {
        let a = attributes
        switch name {
        case "svg":
            let numbers = (a["viewBox"] ?? "0 0 24 24").split(separator: " ").compactMap { Double($0) }
            if numbers.count == 4 { viewBox = CGRect(x: numbers[0], y: numbers[1], width: numbers[2], height: numbers[3]) }
        case "g":
            var group = groups.last ?? Group()
            if let fill = a["fill"] { group.fill = fill }
            if let stroke = a["stroke"] { group.stroke = stroke }
            if let width = a["stroke-width"] { group.strokeWidth = width }
            group.opacity *= number(a["opacity"]) ?? 1
            if let clip = reference(a["clip-path"]).flatMap({ clips[$0] }) { group.clip = clip }
            group.transform = transform(a["transform"]).concatenating(group.transform)
            groups.append(group)
        case "linearGradient", "radialGradient":
            gradient = (a["id"] ?? "", a.merging(["kind": name]) { $1 }, [])
        case "stop":
            guard let color = a["stop-color"] else { return }
            gradient?.stops.append(.init(color: Color(svg: color), location: number(a["offset"]) ?? 0))
        case "clipPath":
            clipping = (a["id"] ?? "", Path())
        default:
            guard let shape = shape(name, a) else { return }
            let group = groups.last ?? Group()
            let path = shape.applying(transform(a["transform"]).concatenating(group.transform))
            if clipping != nil {
                clipping?.path.addPath(path)
                return
            }
            let fill = a["fill"] ?? group.fill ?? "#000"
            let stroke = a["stroke"] ?? group.stroke
            let dash = (a["stroke-dasharray"] ?? "").split(whereSeparator: { $0 == " " || $0 == "," }).compactMap { Double($0) }.map { CGFloat($0) }
            items.append(SVGDrawing.Item(
                path: path,
                fill: paint(fill),
                stroke: stroke.flatMap(paint),
                style: StrokeStyle(
                    lineWidth: number(a["stroke-width"] ?? group.strokeWidth) ?? 1,
                    lineCap: a["stroke-linecap"] == "round" ? .round : a["stroke-linecap"] == "square" ? .square : .butt,
                    lineJoin: a["stroke-linejoin"] == "round" ? .round : a["stroke-linejoin"] == "bevel" ? .bevel : .miter,
                    dash: dash),
                opacity: group.opacity * (number(a["opacity"]) ?? 1),
                fillOpacity: number(a["fill-opacity"]) ?? 1,
                clip: group.clip))
        }
    }

    func parser(_ parser: XMLParser, didEndElement name: String, namespaceURI: String?, qualifiedName: String?) {
        switch name {
        case "g": if groups.count > 1 { groups.removeLast() }
        case "linearGradient", "radialGradient":
            guard let made = gradient else { return }
            let a = made.attributes
            let stops = Gradient(stops: made.stops)
            gradients[made.id] = a["kind"] == "linearGradient"
                ? .linear(stops, start: CGPoint(x: number(a["x1"]) ?? 0, y: number(a["y1"]) ?? 0),
                          end: CGPoint(x: number(a["x2"]) ?? 1, y: number(a["y2"]) ?? 0))
                : .radial(stops, center: CGPoint(x: number(a["cx"]) ?? 0.5, y: number(a["cy"]) ?? 0.5), radius: number(a["r"]) ?? 0.5)
            gradient = nil
        case "clipPath":
            if let made = clipping { clips[made.id] = made.path }
            clipping = nil
        default: break
        }
    }

    private func shape(_ name: String, _ a: [String: String]) -> Path? {
        func n(_ key: String) -> CGFloat { CGFloat(number(a[key]) ?? 0) }
        switch name {
        case "path": return a["d"].map(SVGPath.parse)
        case "circle": return Path(ellipseIn: CGRect(x: n("cx") - n("r"), y: n("cy") - n("r"), width: n("r") * 2, height: n("r") * 2))
        case "ellipse": return Path(ellipseIn: CGRect(x: n("cx") - n("rx"), y: n("cy") - n("ry"), width: n("rx") * 2, height: n("ry") * 2))
        case "rect":
            let rect = CGRect(x: n("x"), y: n("y"), width: n("width"), height: n("height"))
            let rx = a["rx"] != nil ? n("rx") : n("ry")
            let ry = a["ry"] != nil ? n("ry") : rx
            return Path(roundedRect: rect, cornerSize: CGSize(width: min(rx, rect.width / 2), height: min(ry, rect.height / 2)))
        default: return nil
        }
    }

    private func paint(_ value: String) -> SVGDrawing.Paint? {
        if value == "none" { return nil }
        if let id = reference(value) { return gradients[id] }
        return .color(Color(svg: value))
    }

    private func reference(_ value: String?) -> String? {
        guard let value, value.hasPrefix("url(#") else { return nil }
        return String(value.dropFirst(5).dropLast())
    }

    private func number(_ value: String?) -> Double? {
        value.flatMap { Double($0.trimmingCharacters(in: .whitespaces)) }
    }

    /// The `rotate(a cx cy)` and `translate(x y)` the buddies set.
    private func transform(_ value: String?) -> CGAffineTransform {
        guard let value, let open = value.firstIndex(of: "("), let close = value.firstIndex(of: ")") else { return .identity }
        let kind = value[..<open].trimmingCharacters(in: .whitespaces)
        let args = value[value.index(after: open)..<close].split(whereSeparator: { $0 == " " || $0 == "," }).compactMap { Double($0) }
        switch kind {
        case "rotate" where !args.isEmpty:
            let angle = args[0] * .pi / 180
            let (cx, cy) = args.count >= 3 ? (args[1], args[2]) : (0, 0)
            return CGAffineTransform(translationX: -cx, y: -cy)
                .concatenating(CGAffineTransform(rotationAngle: angle))
                .concatenating(CGAffineTransform(translationX: cx, y: cy))
        case "translate" where !args.isEmpty:
            return CGAffineTransform(translationX: args[0], y: args.count > 1 ? args[1] : 0)
        case "scale" where !args.isEmpty:
            return CGAffineTransform(scaleX: args[0], y: args.count > 1 ? args[1] : args[0])
        default: return .identity
        }
    }
}

private extension Color {
    /// `#rgb` or `#rrggbb`, else white or black by name.
    init(svg value: String) {
        var hex = value.trimmingCharacters(in: .whitespaces).lowercased()
        switch hex {
        case "white": hex = "#ffffff"
        case "black": hex = "#000000"
        default: break
        }
        if hex.hasPrefix("#") { hex.removeFirst() }
        if hex.count == 3 { hex = hex.map { "\($0)\($0)" }.joined() }
        let number = UInt64(hex, radix: 16) ?? 0
        self.init(.sRGB, red: Double((number >> 16) & 0xff) / 255, green: Double((number >> 8) & 0xff) / 255, blue: Double(number & 0xff) / 255)
    }
}
