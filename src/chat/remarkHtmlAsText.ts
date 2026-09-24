interface MdNode {
    type: string;
    value?: string;
    children?: MdNode[];
}

const BLOCK_PARENTS = new Set(["root", "blockquote", "listItem"]);

function walk(node: MdNode): void {
    node.children = node.children?.map((child) => {
        if (child.type !== "html") {
            walk(child);
            return child;
        }
        const text = { type: "text", value: child.value ?? "" };
        return BLOCK_PARENTS.has(node.type) ? { type: "paragraph", children: [text] } : text;
    });
}

/** Shows markup a person pasted as the characters they pasted, where markdown would drop it. */
export function remarkHtmlAsText() {
    return (tree: MdNode) => walk(tree);
}
