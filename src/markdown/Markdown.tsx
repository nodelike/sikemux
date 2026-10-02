import { createContext, createElement, memo, useContext, type ComponentType, type ReactNode } from "react";
import type { MarkdownOptions, MdElement, MdNode } from "./types";
import { useMarkdownBlocks } from "./useMarkdownBlocks";

/** Where a surface draws something its own way. Anything left out is drawn as plain HTML. */
export interface MarkdownComponents {
    readonly link?: ComponentType<{ href: string; title?: string; children: ReactNode }>;
    /** A fenced or indented code block. `lang` is the first word after the opening fence. */
    readonly fence?: ComponentType<{ lang?: string; text: string }>;
    /** Code between backticks, outside a link. */
    readonly code?: ComponentType<{ text: string }>;
    /** A run of text outside a link. */
    readonly text?: ComponentType<{ text: string }>;
    readonly table?: ComponentType<{ children: ReactNode }>;
    readonly heading?: ComponentType<{ level: number; children: ReactNode }>;
    /** `inLink` is set when the picture is itself the content of a link. */
    readonly img?: ComponentType<{ src: string; alt: string; title?: string; inLink: boolean }>;
}

export const MARKDOWN_GFM: MarkdownOptions = { gfm: true, htmlAsText: false, fileLinks: false };
export const MARKDOWN_PLAIN: MarkdownOptions = { gfm: false, htmlAsText: false, fileLinks: false };

const NONE: MarkdownComponents = {};
const ComponentsContext = createContext<MarkdownComponents>(NONE);

function renderAll(nodes: readonly MdNode[], components: MarkdownComponents, inLink: boolean): ReactNode[] {
    return nodes.map((node, index) => renderNode(node, components, inLink, index));
}

function taskBox(checked: boolean): ReactNode {
    return <input key="task" type="checkbox" checked={checked} readOnly disabled />;
}

/* A task item's box sits inside its first paragraph when the list is loose,
   and at the front of the item when it is tight, as GitHub draws it. */
function taskItem(checked: boolean, children: readonly MdNode[], components: MarkdownComponents, inLink: boolean): ReactNode[] {
    const [first, ...rest] = children;
    if (first && typeof first !== "string" && first.t === "p") {
        return [
            <p key={0}>
                {taskBox(checked)}
                {first.c.length > 0 && " "}
                {renderAll(first.c, components, inLink)}
            </p>,
            ...rest.map((node, index) => renderNode(node, components, inLink, index + 1)),
        ];
    }
    return [taskBox(checked), children.length > 0 && " ", ...renderAll(children, components, inLink)];
}

function footnote(n: number, children: readonly MdNode[], components: MarkdownComponents): ReactNode[] {
    const [first, ...rest] = children;
    const mark = <sup key="mark">{n}</sup>;
    if (first && typeof first !== "string" && first.t === "p") {
        return [
            <p key={0}>
                {mark} {renderAll(first.c, components, false)}
            </p>,
            ...rest.map((node, index) => renderNode(node, components, false, index + 1)),
        ];
    }
    return [mark, ...renderAll(children, components, false)];
}

function renderTable(node: Extract<MdElement, { t: "table" }>, components: MarkdownComponents, key: number): ReactNode {
    const style = (column: number) => {
        const align = node.align[column];
        return align ? { textAlign: align } : undefined;
    };
    const inner = [
        node.head && (
            <thead key="head">
                <tr>
                    {node.head.map((cell, column) => (
                        <th key={column} style={style(column)}>
                            {renderAll(cell, components, false)}
                        </th>
                    ))}
                </tr>
            </thead>
        ),
        node.rows.length > 0 && (
            <tbody key="body">
                {node.rows.map((row, index) => (
                    <tr key={index}>
                        {row.map((cell, column) => (
                            <td key={column} style={style(column)}>
                                {renderAll(cell, components, false)}
                            </td>
                        ))}
                    </tr>
                ))}
            </tbody>
        ),
    ];
    const Table = components.table;
    return Table ? <Table key={key}>{inner}</Table> : <table key={key}>{inner}</table>;
}

function renderNode(node: MdNode, components: MarkdownComponents, inLink: boolean, key: number): ReactNode {
    if (typeof node === "string") {
        const Text = components.text;
        return Text && !inLink ? <Text key={key} text={node} /> : node;
    }
    switch (node.t) {
        case "p":
            return <p key={key}>{renderAll(node.c, components, inLink)}</p>;
        case "h": {
            const Heading = components.heading;
            const children = renderAll(node.c, components, inLink);
            return Heading ? (
                <Heading key={key} level={node.l}>
                    {children}
                </Heading>
            ) : (
                createElement(`h${node.l}`, { key }, children)
            );
        }
        case "quote":
            return <blockquote key={key}>{renderAll(node.c, components, inLink)}</blockquote>;
        case "ul":
        case "ol": {
            const tasks = node.c.some((item) => typeof item !== "string" && item.t === "li" && item.checked !== undefined);
            const className = tasks ? "contains-task-list" : undefined;
            const items = renderAll(node.c, components, inLink);
            return node.t === "ol" ? (
                <ol key={key} className={className} start={node.start}>
                    {items}
                </ol>
            ) : (
                <ul key={key} className={className}>
                    {items}
                </ul>
            );
        }
        case "li":
            return node.checked === undefined ? (
                <li key={key}>{renderAll(node.c, components, inLink)}</li>
            ) : (
                <li key={key} className="task-list-item">
                    {taskItem(node.checked, node.c, components, inLink)}
                </li>
            );
        case "pre": {
            const Fence = components.fence;
            if (Fence) return <Fence key={key} lang={node.lang} text={node.v} />;
            return (
                <pre key={key}>
                    <code className={node.lang ? `language-${node.lang}` : undefined}>{node.v}</code>
                </pre>
            );
        }
        case "hr":
            return <hr key={key} />;
        case "br":
            return <br key={key} />;
        case "table":
            return renderTable(node, components, key);
        case "fndef":
            return footnote(node.n, node.c, components);
        case "em":
            return <em key={key}>{renderAll(node.c, components, inLink)}</em>;
        case "strong":
            return <strong key={key}>{renderAll(node.c, components, inLink)}</strong>;
        case "del":
            return <del key={key}>{renderAll(node.c, components, inLink)}</del>;
        case "code": {
            const Code = components.code;
            return Code && !inLink ? <Code key={key} text={node.v} /> : <code key={key}>{node.v}</code>;
        }
        case "a": {
            const Link = components.link;
            const children = renderAll(node.c, components, true);
            return Link ? (
                <Link key={key} href={node.href} title={node.title}>
                    {children}
                </Link>
            ) : (
                <a key={key} href={node.href} title={node.title}>
                    {children}
                </a>
            );
        }
        case "img": {
            const Img = components.img;
            return Img ? (
                <Img key={key} src={node.src} alt={node.alt} title={node.title} inLink={inLink} />
            ) : (
                <img key={key} src={node.src} alt={node.alt} title={node.title} />
            );
        }
        case "fnref":
            return <sup key={key}>{node.n}</sup>;
    }
}

const Block = memo(function Block({ block }: { block: MdElement }) {
    return renderNode(block, useContext(ComponentsContext), false, 0);
});

/** Draws already-read blocks. Each block is drawn again only when it changed. */
export function MarkdownBlocks({ blocks, components = NONE }: { blocks: readonly MdElement[]; components?: MarkdownComponents }) {
    return (
        <ComponentsContext.Provider value={components}>
            {blocks.map((block, index) => (
                <Block key={index} block={block} />
            ))}
        </ComponentsContext.Provider>
    );
}

/**
 * Reads `text` as markdown and draws it. The elements land directly in the
 * parent, with no wrapper, so the parent's styles reach them.
 */
export function Markdown({
    text,
    options,
    live = false,
    components,
}: {
    text: string;
    options: MarkdownOptions;
    /** The message is still being written. */
    live?: boolean;
    components?: MarkdownComponents;
}) {
    const blocks = useMarkdownBlocks(text, options, live);
    return blocks ? <MarkdownBlocks blocks={blocks} components={components} /> : null;
}
