const GAP = 3;

/**
 * Lays each team's line through its rows' marks: it runs from the lead's mark down the lead's tree, stops a few
 * pixels short of each worker's mark and goes on below it, and ends at the last worker's. On a selected row it stops at
 * the row's border instead, so it never enters the box.
 */
export function drawTeamLines(root: HTMLElement): void {
    let spineX = 0;
    for (const wrap of root.querySelectorAll<HTMLElement>(".agent-row-wrap.chain-head, .agent-row-wrap.chain")) {
        const box = wrap.getBoundingClientRect();
        if (wrap.classList.contains("chain-head")) {
            const glyph = wrap.querySelector(":scope > .agent-row .agent-glyph")?.getBoundingClientRect();
            const tree = wrap.querySelector(":scope > .agent-tree")?.getBoundingClientRect();
            spineX = tree ? tree.left - box.left : glyph ? glyph.left + glyph.width / 2 - box.left : 0;
        }
        const mark = wrap.querySelector<SVGElement | HTMLElement>(":scope > .agent-row .agent-glyph svg, :scope > .agent-row .agent-glyph img");
        const row = wrap.querySelector<HTMLElement>(":scope > .agent-row");
        if (!mark || !row) continue;
        const drawn = mark.getBoundingClientRect();
        const inset = (mark.tagName.toLowerCase() === "svg" ? (drawn.height * 4) / 32 : 1) - GAP;
        const framed = row.classList.contains("active");
        const edges = row.getBoundingClientRect();
        const top = framed ? edges.top - box.top : drawn.top - box.top + inset;
        const bottom = framed ? edges.bottom - box.top : drawn.bottom - box.top - inset;
        const tree = wrap.querySelector(":scope > .agent-tree")?.getBoundingClientRect();
        const until = tree ? tree.top - box.top + 1 : box.height + 2;
        wrap.style.setProperty("--cx", `${Math.round(spineX)}px`);
        wrap.style.setProperty("--gt", `${Math.round(top)}px`);
        wrap.style.setProperty("--gb", `${Math.round(bottom)}px`);
        wrap.style.setProperty("--down", `${Math.max(0, Math.round(until - bottom))}px`);
    }
}
