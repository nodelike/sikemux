const chord = /^([⌘⌥⇧⌃]+)(.*)$/;

function keysOf(node) {
  if (node.tagName !== "code" || node.children.length !== 1 || node.children[0].type !== "text") return undefined;
  const match = node.children[0].value.match(chord);
  if (!match) return undefined;
  return [...match[1], ...(match[2] ? [match[2]] : [])];
}

function walk(node) {
  if (!node.children || node.tagName === "pre") return;
  node.children = node.children.map((child) => {
    const keys = child.type === "element" && keysOf(child);
    if (!keys) {
      walk(child);
      return child;
    }
    return {
      type: "element",
      tagName: "span",
      properties: { className: ["keys"] },
      children: keys.map((key) => ({ type: "element", tagName: "kbd", properties: {}, children: [{ type: "text", value: key }] })),
    };
  });
}

export function rehypeKeys() {
  return walk;
}
