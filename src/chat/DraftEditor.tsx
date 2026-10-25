import { useEffect, useImperativeHandle, useLayoutEffect, useRef, useState, type ClipboardEvent, type KeyboardEvent, type Ref } from "react";
import { createPortal } from "react-dom";
import { AgentFace } from "../buddies/Buddy";
import type { Agent } from "../state/types";
import { FileIcon } from "../ui/FileIcon";
import { IconImage } from "../ui/Icons";
import { tagStarts } from "@shared/chat/composerInput";
import { ContextMark } from "./ContextChip";

/** Something the draft names with a prefix, drawn as a chip in place of its text. */
export interface DraftChip {
    /** How it is written in the draft, such as `#135`. */
    text: string;
    kind: "agent" | "issue" | "pull" | "file" | "image";
    /** What follows the name on the chip: an issue's title, or a file's name. */
    label?: string;
    /** The agent a chip names, for its buddy or harness mark. */
    agent?: Pick<Agent, "id" | "type" | "buddy" | "name">;
}

/** The editor as the composer drives it: the draft as a string, and offsets into that string. */
export interface DraftBox {
    readonly value: string;
    readonly selectionStart: number;
    readonly selectionEnd: number;
    scrollTop: number;
    setSelectionRange(start: number, end: number): void;
    focus(): void;
}

type Part = string | DraftChip;

/* A caret cannot rest after a chip that ends a line, so one gets a zero-width
   space to stand in; it never reaches the draft. */
const PAD = "​";

function partsOf(value: string, chips: readonly DraftChip[]): Part[] {
    const spans: { start: number; chip: DraftChip }[] = [];
    for (const chip of chips) for (const start of tagStarts(value, chip.text)) spans.push({ start, chip });
    spans.sort((a, b) => a.start - b.start);
    const parts: Part[] = [];
    let from = 0;
    for (const { start, chip } of spans) {
        if (start < from) continue;
        if (start > from) parts.push(value.slice(from, start));
        parts.push(chip);
        from = start + chip.text.length;
    }
    if (from < value.length) parts.push(value.slice(from));
    return parts;
}

const isChip = (node: Node): node is HTMLElement => node instanceof HTMLElement && node.dataset.tag !== undefined;
const isEnd = (node: Node) => node instanceof HTMLElement && node.dataset.end !== undefined;

/** The draft a stretch of the editor holds. A block the browser wrapped a line in counts as a line break. */
export function readDraft(root: Node): string {
    let text = "";
    const walk = (node: Node) => {
        for (const child of node.childNodes) {
            if (child.nodeType === Node.TEXT_NODE) text += (child.textContent ?? "").replaceAll(PAD, "");
            else if (isChip(child)) text += child.dataset.tag;
            else if (child.nodeName === "BR") text += isEnd(child) ? "" : "\n";
            else {
                if ((child.nodeName === "DIV" || child.nodeName === "P") && text && !text.endsWith("\n")) text += "\n";
                walk(child);
            }
        }
    };
    walk(root);
    return text;
}

/** The node and offset an offset into the draft falls at, in an editor laid out by `build`. */
export function draftPoint(root: HTMLElement, offset: number): { node: Node; offset: number } {
    let left = offset;
    const nodes = [...root.childNodes];
    for (const [index, node] of nodes.entries()) {
        if (node.nodeType === Node.TEXT_NODE) {
            const text = node.textContent ?? "";
            if (text === PAD) {
                if (left === 0) return { node, offset: 1 };
                continue;
            }
            if (left <= text.length) return { node, offset: left };
            left -= text.length;
        } else if (isChip(node)) {
            if (left === 0) return { node: root, offset: index };
            const length = node.dataset.tag?.length ?? 0;
            if (left < length) return { node: root, offset: index + 1 };
            left -= length;
            if (left === 0) {
                const next = nodes[index + 1];
                return next?.nodeType === Node.TEXT_NODE
                    ? { node: next, offset: next.textContent === PAD ? 1 : 0 }
                    : { node: root, offset: index + 1 };
            }
        }
    }
    const end = nodes.findIndex(isEnd);
    return { node: root, offset: end === -1 ? nodes.length : end };
}

export function draftOffset(root: HTMLElement, node: Node, offset: number): number {
    const range = document.createRange();
    range.setStart(root, 0);
    range.setEnd(node, offset);
    return readDraft(range.cloneContents()).length;
}

function laidOut(root: HTMLElement, parts: readonly Part[], value: string): boolean {
    const nodes = [...root.childNodes].filter((node) => !(node.nodeType === Node.TEXT_NODE && node.textContent === PAD) && !isEnd(node));
    if (nodes.length !== parts.length) return false;
    if (value.endsWith("\n") !== [...root.childNodes].some(isEnd)) return false;
    return parts.every((part, index) => {
        const node = nodes[index];
        return typeof part === "string"
            ? node.nodeType === Node.TEXT_NODE && node.textContent === part
            : isChip(node) && node.dataset.tag === part.text;
    });
}

function build(root: HTMLElement, parts: readonly Part[], value: string): { el: HTMLElement; chip: DraftChip }[] {
    const chips: { el: HTMLElement; chip: DraftChip }[] = [];
    const nodes: Node[] = [];
    for (const part of parts) {
        if (typeof part === "string") {
            nodes.push(document.createTextNode(part));
            continue;
        }
        const el = document.createElement("span");
        el.className = "chat-chip";
        el.contentEditable = "false";
        el.dataset.tag = part.text;
        el.dataset.kind = part.kind;
        chips.push({ el, chip: part });
        nodes.push(el);
    }
    if (nodes.length > 0 && isChip(nodes[nodes.length - 1])) nodes.push(document.createTextNode(PAD));
    if (value.endsWith("\n")) {
        const end = document.createElement("br");
        end.dataset.end = "";
        nodes.push(end);
    }
    root.replaceChildren(...nodes);
    return chips;
}

/** The selection in the editor as offsets into the draft, or nothing when it is elsewhere. */
function selectionIn(root: HTMLElement): [number, number] | null {
    const picked = window.getSelection();
    if (!picked || picked.rangeCount === 0 || !root.contains(picked.anchorNode)) return null;
    const range = picked.getRangeAt(0);
    return [draftOffset(root, range.startContainer, range.startOffset), draftOffset(root, range.endContainer, range.endOffset)];
}

function selectIn(root: HTMLElement, start: number, end: number): void {
    const range = document.createRange();
    const from = draftPoint(root, start);
    const to = draftPoint(root, end);
    range.setStart(from.node, from.offset);
    range.setEnd(to.node, to.offset);
    const picked = window.getSelection();
    picked?.removeAllRanges();
    picked?.addRange(range);
}

function ChipBody({ chip }: { chip: DraftChip }) {
    return (
        <>
            <span className="chat-chip-mark" data-kind={chip.kind}>
                {chip.kind === "agent" ? (
                    chip.agent && (
                        <AgentFace
                            agent={chip.agent}
                            size={12}
                            logoSize={12}
                            logoClass={`agent-glyph ${chip.agent.type}`}
                            buddyClass="agent-glyph buddy-mark"
                        />
                    )
                ) : chip.kind === "file" ? (
                    <FileIcon name={chip.label ?? chip.text} size={12} />
                ) : chip.kind === "image" ? (
                    <IconImage size={12} />
                ) : (
                    <ContextMark kind={chip.kind} size={12} />
                )}
            </span>
            {chip.kind === "file" ? (
                <span className="chat-chip-name">{chip.label}</span>
            ) : chip.kind === "image" ? (
                <span className="chat-chip-name">{chip.text.slice(1, -1)}</span>
            ) : (
                <code>{chip.text}</code>
            )}
            {chip.kind !== "file" && chip.kind !== "image" && chip.label && <span className="chat-chip-label">{chip.label}</span>}
        </>
    );
}

/**
 * The composer's text box. The draft stays a plain string; what it names with a
 * prefix is drawn as a chip that the caret steps over and one Backspace takes.
 */
export default function DraftEditor({
    value,
    chips,
    label,
    placeholder,
    box,
    onChange,
    onCaret,
    onKeyDown,
    onPaste,
}: {
    value: string;
    chips: readonly DraftChip[];
    label: string;
    placeholder: string;
    box: Ref<DraftBox>;
    onChange: (value: string, caret: number) => void;
    onCaret: (caret: number) => void;
    onKeyDown: (event: KeyboardEvent<HTMLDivElement>) => void;
    onPaste: (event: ClipboardEvent<HTMLDivElement>) => void;
}) {
    const root = useRef<HTMLDivElement>(null);
    const [mounted, setMounted] = useState<{ el: HTMLElement; chip: DraftChip }[]>([]);
    const latest = useRef({ value, chips, onChange, onCaret });
    latest.current = { value, chips, onChange, onCaret };

    /* A caret placed along with a new draft waits for the draft to be laid out, or the
       layout would put it back at the start; a frame later it is no longer wanted. */
    const pending = useRef<[number, number] | null>(null);
    const select = (start: number, end: number) => {
        if (!root.current) return;
        selectIn(root.current, Math.min(start, readDraft(root.current).length), Math.min(end, readDraft(root.current).length));
        pending.current = [start, end];
        window.requestAnimationFrame(() => {
            pending.current = null;
        });
    };

    /** Lays the editor out again for `next` when its nodes no longer match, keeping the caret where it was in the draft. */
    const settle = (next: string, caret?: [number, number] | null) => {
        const el = root.current;
        if (!el) return;
        const parts = partsOf(next, latest.current.chips);
        if (readDraft(el) === next && laidOut(el, parts, next)) return;
        const focused = document.activeElement === el;
        const kept = caret === undefined ? (focused && readDraft(el) === next ? selectionIn(el) : null) : caret;
        setMounted(build(el, parts, next));
        if (kept && focused) selectIn(el, Math.min(kept[0], next.length), Math.min(kept[1], next.length));
    };
    const settleRef = useRef(settle);
    settleRef.current = settle;

    useImperativeHandle(box, (): DraftBox => ({
        get value() {
            return latest.current.value;
        },
        get selectionStart() {
            return root.current ? (selectionIn(root.current)?.[0] ?? latest.current.value.length) : 0;
        },
        get selectionEnd() {
            return root.current ? (selectionIn(root.current)?.[1] ?? latest.current.value.length) : 0;
        },
        get scrollTop() {
            return root.current?.scrollTop ?? 0;
        },
        set scrollTop(top: number) {
            if (root.current) root.current.scrollTop = top;
        },
        setSelectionRange: select,
        focus: () => root.current?.focus(),
    }));

    const chipTexts = chips.map((chip) => chip.text).join("\n");
    useLayoutEffect(() => {
        settleRef.current(value);
        const el = root.current;
        if (!el || !pending.current) return;
        selectIn(el, Math.min(pending.current[0], value.length), Math.min(pending.current[1], value.length));
        pending.current = null;
    }, [value, chipTexts]);

    /** Puts text in place of the selection, the way a typed key would. */
    const insert = (text: string) => {
        const el = root.current;
        if (!el) return;
        const current = latest.current.value;
        const [start, end] = selectionIn(el) ?? [current.length, current.length];
        const next = `${current.slice(0, start)}${text.replace(/\r\n?/g, "\n")}${current.slice(end)}`;
        const caret = start + text.length;
        settle(next, [caret, caret]);
        latest.current.onChange(next, caret);
    };
    const insertRef = useRef(insert);
    insertRef.current = insert;

    useEffect(() => {
        const el = root.current;
        if (!el) return;
        const insert = (text: string) => insertRef.current(text);
        const beforeInput = (event: InputEvent) => {
            if (event.inputType.startsWith("format")) event.preventDefault();
            else if (event.inputType === "insertParagraph" || event.inputType === "insertLineBreak") {
                event.preventDefault();
                insert("\n");
            } else if (event.inputType === "insertFromDrop" || event.inputType === "insertFromPaste") {
                event.preventDefault();
                insert(event.dataTransfer?.getData("text/plain") ?? "");
            }
        };
        const selected = () => {
            const picked = window.getSelection();
            if (!picked || picked.rangeCount === 0 || !el.contains(picked.anchorNode)) return;
            const range = picked.getRangeAt(0);
            for (const chip of el.querySelectorAll<HTMLElement>(".chat-chip"))
                chip.toggleAttribute("data-selected", !range.collapsed && range.intersectsNode(chip));
            latest.current.onCaret(draftOffset(el, range.startContainer, range.startOffset));
        };
        el.addEventListener("beforeinput", beforeInput);
        document.addEventListener("selectionchange", selected);
        return () => {
            el.removeEventListener("beforeinput", beforeInput);
            document.removeEventListener("selectionchange", selected);
        };
    }, []);

    const changed = (composing: boolean) => {
        const el = root.current;
        if (!el || composing) return;
        const next = readDraft(el);
        const kept = selectionIn(el) ?? [next.length, next.length];
        settle(next, kept);
        latest.current.onChange(next, kept[0]);
    };

    return (
        <>
            <div
                ref={root}
                className="chat-draft"
                role="textbox"
                aria-multiline="true"
                aria-label={label}
                aria-placeholder={placeholder}
                data-placeholder={placeholder}
                data-empty={value === "" ? "" : undefined}
                contentEditable="true"
                tabIndex={0}
                suppressContentEditableWarning
                spellCheck
                onInput={(event) => changed((event.nativeEvent as InputEvent).isComposing)}
                onCompositionEnd={() => changed(false)}
                onKeyDown={onKeyDown}
                onPaste={(event) => {
                    onPaste(event);
                    if (event.defaultPrevented) return;
                    event.preventDefault();
                    insert(event.clipboardData.getData("text/plain"));
                }}
            />
            {mounted.map(({ el, chip }, index) =>
                createPortal(<ChipBody chip={chips.find((current) => current.text === chip.text) ?? chip} />, el, index),
            )}
        </>
    );
}
