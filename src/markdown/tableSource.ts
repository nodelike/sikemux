import type { MdElement, MdNode } from "./types";

type Table = Extract<MdElement, { t: "table" }>;

function inline(node: MdNode): string {
    if (typeof node === "string") return node.replace(/\|/g, "\\|");
    switch (node.t) {
        case "code":
            return `\`${node.v.replace(/\|/g, "\\|")}\``;
        case "strong":
            return `**${inlines(node.c)}**`;
        case "em":
            return `*${inlines(node.c)}*`;
        case "del":
            return `~~${inlines(node.c)}~~`;
        case "a":
            return `[${inlines(node.c)}](${node.href})`;
        case "img":
            return `![${node.alt}](${node.src})`;
        case "br":
            return "<br>";
        case "fnref":
            return `[^${node.n}]`;
        default:
            return "c" in node ? inlines(node.c) : "";
    }
}

const inlines = (nodes: readonly MdNode[]) => nodes.map(inline).join("");

const row = (cells: readonly MdNode[][]) => `| ${cells.map(inlines).join(" | ")} |`;

const rule = (align: Table["align"][number]) => (align === "center" ? ":---:" : align === "right" ? "---:" : align === "left" ? ":---" : "---");

/** The table written back as markdown, so it pastes into a doc or another agent as a table. */
export function tableSource(table: Table): string {
    const head = table.head ?? table.align.map(() => []);
    return [row(head), `| ${table.align.map(rule).join(" | ")} |`, ...table.rows.map(row)].join("\n");
}
