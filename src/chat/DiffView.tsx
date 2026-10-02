import { useMemo, useState } from "react";
import { FileIcon } from "../ui/FileIcon";
import { basename } from "../lib/paths";
import { collapseDiff, type DiffLine, type ToolDiff } from "./diff";
import { CodeRun, splitAtMark, useDiffTokens } from "./codeHighlight";
import type { CodeLine } from "./types";

/* The text of one diff line: its runs, with the span that changed inside the
   one mark that shows it. A line with no colours yet is its own single run, so
   the marking is the same either way. */
export function DiffText({ line, tokens }: { line: DiffLine; tokens?: CodeLine }) {
    const { pre, marked, post } = splitAtMark(tokens ?? [{ text: line.text }], line.mark);
    return (
        <span>
            <CodeRun tokens={pre} />
            {marked.length > 0 && (
                <mark>
                    <CodeRun tokens={marked} />
                </mark>
            )}
            <CodeRun tokens={post} />
        </span>
    );
}

export function DiffBody({ diff }: { diff: ToolDiff }) {
    const [expanded, setExpanded] = useState(false);
    const view = useMemo(() => collapseDiff(diff.lines, expanded ? Number.MAX_SAFE_INTEGER : 3), [diff.lines, expanded]);
    const coloured = useDiffTokens(diff.lines, diff.path);
    return (
        <div className="chat-diff">
            <div className="chat-diff-head">
                <FileIcon name={basename(diff.path)} size={12} />
                <span className="chat-diff-path" title={diff.path}>
                    {diff.path}
                </span>
                <span className="chat-diff-adds">+{diff.adds}</span>
                <span className="chat-diff-dels">−{diff.dels}</span>
            </div>
            <div className="chat-diff-body">
                {view.rows.map((row, index) =>
                    "gap" in row ? (
                        <button type="button" className="chat-diff-gap" key={`gap-${index}`} onClick={() => setExpanded(true)}>
                            {row.gap} unchanged {row.gap === 1 ? "line" : "lines"}
                        </button>
                    ) : (
                        <div className={`chat-diff-line${row.sign === "+" ? " add" : row.sign === "-" ? " del" : ""}`} key={index}>
                            <span className="chat-diff-ln">{row.sign === "+" ? row.newLine : row.oldLine}</span>
                            <span className="chat-diff-sign">{row.sign}</span>
                            <DiffText line={row} tokens={coloured?.get(row)} />
                        </div>
                    ),
                )}
            </div>
        </div>
    );
}
