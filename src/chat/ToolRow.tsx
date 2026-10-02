import { memo, useContext, useRef, useState } from "react";
import { CopyButton } from "../ui/CopyButton";
import { hasPrimaryModifier } from "../lib/platform";
import { IconAgent, IconChevron, IconCommand, IconFile, IconGlobe, IconPencil, IconPlug, IconSearch, IconWarning } from "../ui/Icons";
import type { ToolOutput } from "./toolOutput";
import { ChatFileRef, useFileRef } from "./FileRef";
import { ChatAgentContext, openLink } from "./chatAgent";
import { ChatImage } from "./ChatImage";
import { DiffBody } from "./DiffView";
import { LiveSeconds } from "./LiveSeconds";
import { useCutOff } from "./useCutOff";
import { durationLabel } from "./durationLabel";
import { toolKind, toolLabel, toolPath, toolRunning, toolTarget, toolUrl } from "./toolLabels";
import { attachmentName } from "./transcript";
import type { AcpToolCall, ChatPart } from "./types";

function ToolKindIcon({ tool, kind }: { tool: AcpToolCall; kind?: string }) {
    if (tool.status === "failed") return <IconWarning size={11} />;
    switch (kind) {
        case "mcp":
            return <IconPlug size={11} />;
        case "read":
            return <IconFile size={11} />;
        case "search":
            return <IconSearch size={11} />;
        case "edit":
        case "move":
        case "delete":
            return <IconPencil size={11} />;
        case "execute":
            return <IconCommand size={11} />;
        case "fetch":
            return <IconGlobe size={11} />;
        default:
            return <IconAgent size={11} />;
    }
}

function ToolTarget({ text }: { text: string }) {
    const agentId = useContext(ChatAgentContext).id;
    const link = toolUrl(text);
    if (!link) return <>{text}</>;
    return (
        <>
            {link.before}
            <a
                className="chat-tool-link"
                href={link.url}
                onClick={(event) => {
                    event.preventDefault();
                    openLink(link.url, agentId, hasPrimaryModifier(event));
                }}>
                {link.raw}
            </a>
            {link.after}
        </>
    );
}

const OUTPUT_FOLD_LINES = 12;

/* A command and what it printed, the way a terminal would have shown them. */
function ToolTerminal({ command, output, failed }: { command: string | null; output?: ToolOutput; failed: boolean }) {
    const [whole, setWhole] = useState(false);
    const lines = output?.text ? output.text.split("\n") : [];
    const folds = lines.length > OUTPUT_FOLD_LINES;
    const folded = folds && !whole;
    return (
        <div className="chat-tool-terminal">
            {command !== null && (
                <div className="chat-tool-command">
                    <span className="chat-tool-prompt" aria-hidden="true">
                        $
                    </span>
                    <pre>{command}</pre>
                    <CopyButton value={command} label="command" size={12} />
                </div>
            )}
            {output?.image && (
                <div className="chat-tool-picture">
                    <ChatImage src={`data:${output.image.mimeType};base64,${output.image.data}`} name={attachmentName(output.image.mimeType)} />
                </div>
            )}
            {output &&
                (output.text ? (
                    <div className={`chat-tool-output${folded ? " folded" : ""}`}>
                        <pre>{folded ? lines.slice(0, OUTPUT_FOLD_LINES).join("\n") : output.text}</pre>
                        <CopyButton value={output.text} label="output" size={12} />
                    </div>
                ) : (
                    !output.image && <div className="chat-tool-output empty">No output</div>
                ))}
            {folds && (
                <button type="button" className="chat-tool-more" onClick={() => setWhole(!whole)}>
                    {folded ? `Show all ${lines.length} lines` : "Show fewer lines"}
                </button>
            )}
            {output?.cut && (!folds || whole) && <div className="chat-tool-note">The rest of the output was not kept</div>}
            {failed && output?.exitCode !== undefined && <div className="chat-tool-exit">exit {output.exitCode}</div>}
        </div>
    );
}

export const ToolRow = memo(function ToolRow({ part }: { part: Extract<ChatPart, { kind: "tool" }> }) {
    const [open, setOpen] = useState(false);
    const targetRef = useRef<HTMLSpanElement>(null);
    const tool = part.tool;
    // An MCP call is named for the server it went to, whatever kind it claims.
    const rowKind = toolLabel(tool.title).scope !== undefined ? "mcp" : tool.kind;
    const { diff, output, failure } = part;
    const status = tool.status ?? "pending";
    const running = toolRunning(tool);
    const file = useFileRef(toolPath(tool));
    const target = toolTarget(tool);
    const linked = !file && toolUrl(target) !== null;
    const command = tool.kind === "execute" ? tool.title.trim() : null;
    /* The row holds one line of a command, so the whole of it is worth
       opening when the line ends in an ellipsis or leaves lines out. */
    const overflowing = useCutOff(targetRef, command !== null && !running);
    const cutOff = command !== null && !running && (overflowing || command !== target);
    const opens = Boolean(diff || output || failure) || cutOff;
    /* A call the turn cut off has a duration, but printing it would read as a
       call that ran that long and then finished. It says why it stopped. */
    const measured = part.startedAt !== undefined && part.endedAt !== undefined ? part.endedAt - part.startedAt : null;
    const elapsed = status === "cancelled" ? "stopped" : measured !== null ? durationLabel(measured) : null;
    const toggle = () => setOpen((current) => !current);
    const lead = (
        <>
            <span className="chat-tool-tick" aria-hidden="true" />
            <span className="chat-tool-icon">
                <ToolKindIcon tool={tool} kind={rowKind} />
            </span>
            <span className="chat-tool-kind">{toolKind(tool)}</span>
            <span className="chat-tool-target" ref={targetRef}>
                {file ? <ChatFileRef refers={file.ref} state={file.state} label={target} size={17} /> : <ToolTarget text={target} />}
            </span>
        </>
    );
    // Only a change or a failure asks to be opened; a command's output waits to be pointed at.
    const quiet = !diff && status !== "failed";
    const end = (
        <>
            {diff && (
                <span className="chat-tool-stat">
                    <span className="chat-diff-adds">+{diff.adds}</span>
                    <span className="chat-diff-dels">−{diff.dels}</span>
                </span>
            )}
            {running ? <LiveSeconds since={part.startedAt} /> : elapsed}
            {opens && <IconChevron size={10} className={`chat-tool-chevron${quiet ? " quiet" : ""}`} />}
        </>
    );
    const rowProps = { className: `chat-tool status-${status}${running ? " live" : ""}`, "data-kind": rowKind };
    return (
        <div className="chat-tool-node">
            {/* A row whose target opens a file or a page cannot itself be a
                button, so what is left of it opens the detail instead. */}
            {opens && !file && !linked ? (
                <button type="button" {...rowProps} aria-expanded={open} onClick={toggle}>
                    <span className="chat-tool-line">
                        {lead}
                        <span className="chat-tool-end">{end}</span>
                    </span>
                </button>
            ) : (
                <div {...rowProps}>
                    <span className="chat-tool-line">
                        {lead}
                        {opens ? (
                            <button
                                type="button"
                                className="chat-tool-end"
                                aria-expanded={open}
                                aria-label={open ? "Hide what the call did" : "Show what the call did"}
                                onClick={toggle}>
                                {end}
                            </button>
                        ) : (
                            <span className="chat-tool-end">{end}</span>
                        )}
                    </span>
                </div>
            )}
            {opens && open && (
                <div className="chat-tool-detail">
                    {diff ? (
                        <DiffBody diff={diff} />
                    ) : command !== null || output ? (
                        <ToolTerminal command={command} output={output} failed={status === "failed"} />
                    ) : (
                        <div className="chat-tool-out">{failure}</div>
                    )}
                </div>
            )}
        </div>
    );
});
