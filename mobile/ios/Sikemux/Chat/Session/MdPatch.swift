import Foundation

/// The core's changes to a markdown tree the phone holds. A path runs from the top-level blocks down through each
/// element's children, or a table's rows, its cells and their contents.
enum MdPatch {
    static func apply(_ op: MdOp, to blocks: inout [MdElement]) {
        elements(&blocks, path(op)[...], op)
    }

    private static func path(_ op: MdOp) -> [Int] {
        switch op {
        case .list(let list): list.p
        case .text(let text): text.p
        }
    }

    private static func grown(_ text: String, _ op: MdOp.Text) -> String {
        let units = Array(text.utf16)
        let kept = String(decoding: units[..<min(op.at, units.count)], as: UTF16.self)
        return kept + op.s
    }

    private static func listed<T: Decodable>(_ list: inout [T], _ op: MdOp) {
        guard case .list(let change) = op else { return }
        let added: [T] = change.c.compactMap { decodeJSON($0) }
        list = Array(list.prefix(change.at)) + added
    }

    private static func elements(_ list: inout [MdElement], _ path: ArraySlice<Int>, _ op: MdOp) {
        guard let index = path.first else { return listed(&list, op) }
        guard list.indices.contains(index) else { return }
        element(&list[index], path.dropFirst(), op)
    }

    private static func nodes(_ list: inout [MdNode], _ path: ArraySlice<Int>, _ op: MdOp) {
        guard let index = path.first else { return listed(&list, op) }
        guard list.indices.contains(index) else { return }
        let rest = path.dropFirst()
        switch list[index] {
        case .text(let text):
            if rest.isEmpty, case .text(let grow) = op { list[index] = .text(grown(text, grow)) }
        case .element(var inner):
            element(&inner, rest, op)
            list[index] = .element(inner)
        }
    }

    private static func element(_ node: inout MdElement, _ path: ArraySlice<Int>, _ op: MdOp) {
        if path.isEmpty, case .text(let grow) = op {
            switch node {
            case .pre(var pre):
                pre.v = grown(pre.v, grow)
                node = .pre(pre)
            case .code(var code):
                code.v = grown(code.v, grow)
                node = .code(code)
            default:
                break
            }
            return
        }
        if case .table(var table) = node {
            tableRows(&table.rows, path, op)
            node = .table(table)
            return
        }
        guard var children = node.children else { return }
        nodes(&children, path, op)
        node.children = children
    }

    private static func tableRows(_ rows: inout [[[MdNode]]], _ path: ArraySlice<Int>, _ op: MdOp) {
        guard let index = path.first else { return listed(&rows, op) }
        guard rows.indices.contains(index) else { return }
        let rest = path.dropFirst()
        guard let cell = rest.first else { return listed(&rows[index], op) }
        guard rows[index].indices.contains(cell) else { return }
        nodes(&rows[index][cell], rest.dropFirst(), op)
    }
}

extension MdElement {
    /// What an element holds inside it, for those that hold nodes.
    var children: [MdNode]? {
        get {
            switch self {
            case .p(let e): e.c
            case .h(let e): e.c
            case .quote(let e): e.c
            case .ul(let e): e.c
            case .ol(let e): e.c
            case .li(let e): e.c
            case .fndef(let e): e.c
            case .em(let e): e.c
            case .strong(let e): e.c
            case .del(let e): e.c
            case .faint(let e): e.c
            case .a(let e): e.c
            case .path(let e): e.c
            case .div(let e): e.c
            case .details(let e): e.c
            default: nil
            }
        }
        set {
            guard let c = newValue else { return }
            switch self {
            case .p(var e):
                e.c = c
                self = .p(e)
            case .h(var e):
                e.c = c
                self = .h(e)
            case .quote(var e):
                e.c = c
                self = .quote(e)
            case .ul(var e):
                e.c = c
                self = .ul(e)
            case .ol(var e):
                e.c = c
                self = .ol(e)
            case .li(var e):
                e.c = c
                self = .li(e)
            case .fndef(var e):
                e.c = c
                self = .fndef(e)
            case .em(var e):
                e.c = c
                self = .em(e)
            case .faint(var e):
                e.c = c
                self = .faint(e)
            case .strong(var e):
                e.c = c
                self = .strong(e)
            case .del(var e):
                e.c = c
                self = .del(e)
            case .a(var e):
                e.c = c
                self = .a(e)
            case .path(var e):
                e.c = c
                self = .path(e)
            case .div(var e):
                e.c = c
                self = .div(e)
            case .details(var e):
                e.c = c
                self = .details(e)
            default: break
            }
        }
    }
}
