import { lazy, Suspense, useEffect, useLayoutEffect, useRef, useState, type ReactNode, type RefObject } from "react";
import { errorText } from "../lib/format";
import { dialogsApi } from "../api/dialogs";
import { ComposerPickers } from "./ComposerPickers";
import type { SessionConfig } from "./sessionConfig";
import { basename, joinPath } from "../lib/paths";
import { animate } from "../lib/motion";
import { hasPrimaryModifier, PRIMARY_SHORTCUT } from "../lib/platform";
import { registerPathDrop } from "../state/dropRegistry";
import { registerTextInsert } from "../state/textInsertRegistry";
import type { Agent, ProviderProfile } from "../state/types";
import * as cmd from "../state/commands";
import { useStore } from "../state/store";
import { IconArrowUp, IconClose, IconPlus } from "../ui/Icons";
import { FileTypeIcon } from "../ui/FileIcon";
import { usePathRoots } from "./FileRef";
import { useImagePreview } from "./imagePreview";
import { YoloToggle } from "./YoloToggle";
import type { WorktreeSwitchState } from "./worktreeSwitch";
import { DictateButton } from "./DictateButton";
import { finishDictationInto, useVoice, writesInto } from "../voice/dictation";
import { ContextMeter } from "./ContextMeter";
import { imagesInClipboard, savePastedClipboard } from "./pasteImage";
import { arrowsBrowse, recallPrompt, type HistoryPosition } from "./promptHistory";
import {
    hasTag,
    imageTag,
    MAX_MENU_ENTRIES,
    mergePaths,
    removeToken,
    tagBefore,
    tokenAt,
    withoutAttachment,
    type ComposerTrigger,
    type ProjectEntry,
} from "./composerInput";
import { isImagePath } from "../lib/imagePaths";
import { showImage } from "../state/imageViewer";
import { canSwitchMode } from "../agents/agentLaunch";
import { AgentMark } from "../agents/AgentMark";
import { receiveForAgent } from "../agents/agentInbox";
import type { AcpAvailableCommand, ChatState, ContextUsage } from "./types";
import type { OutgoingMessage } from "./queuedMessages";
import { registerComposer } from "./draftHandoff";
import type { PromptContext } from "../api/acp";
import type { RepoRef } from "../codehost/types";
import type { TrackedItem, TrackedKind } from "../codehost/tracked";
import type { TrackedMatches } from "./TrackedSource";
import { ContextMark } from "./ContextChip";
import { contextChip } from "./promptContext";
import { mentionContext, useMates, type Mate } from "./agentMentions";
import type { EarlierChat } from "./earlierChats";
import { ComposerBuddy } from "../buddies/Buddy";
import ComposerMenu, { type MenuKeys, type MenuPick } from "./ComposerMenu";
import DictationPreview from "./DictationPreview";
import DraftEditor, { type DraftBox, type DraftChip } from "./DraftEditor";

/** Puts the caret at `at`; the editor holds it until the new draft is laid out. */
function caretTo(editor: RefObject<DraftBox | null>, at: number) {
    editor.current?.focus();
    editor.current?.setSelectionRange(at, at);
}

/** The draft with `text` written over the selection as a word of its own, and where the caret goes after it. */
function writtenAtCaret(editor: DraftBox, text: string, spaceAfter: boolean): { draft: string; at: number } {
    const before = editor.value.slice(0, editor.selectionStart);
    const after = editor.value.slice(editor.selectionEnd);
    const inserted = `${before && !/\s$/.test(before) ? " " : ""}${text}${spaceAfter && !/^\s/.test(after) ? " " : ""}`;
    return { draft: `${before}${inserted}${after}`, at: before.length + inserted.length };
}

function ComposerAttachment({ path, number, onRemove }: { path: string; number: number; onRemove: () => void }) {
    const preview = useImagePreview(path);
    const name = basename(path);
    return (
        <span className={preview ? "image" : "file"} title={path}>
            {preview ? (
                <button
                    type="button"
                    className="chat-attachment-open"
                    aria-label={`Open ${name}`}
                    onClick={() => showImage({ src: preview, name, path })}>
                    <img alt={name} src={preview} />
                </button>
            ) : (
                <FileTypeIcon name={name} size={34} />
            )}
            <span className="chat-attachment-number">#{number}</span>
            <button type="button" className="chat-attachment-remove" aria-label={`Remove ${name}`} onClick={onRemove}>
                <IconClose size={10} />
            </button>
        </span>
    );
}

/**
 * An item to hand over with the message. One picked with `#` is read in full only when the message is sent, and an
 * earlier chat picked with `@` is opened again only then.
 */
interface ComposerContext {
    uri: string;
    title: string;
    text: string | null;
    tracked: { repo: RepoRef; kind: TrackedKind; number: number } | null;
    earlier?: EarlierChat;
    /** How it is written in the draft; it goes with the message only while that is still there. */
    tag?: string;
}

/** A project file picked with `+`, attached while its tag is still in the draft. */
interface FileTag {
    tag: string;
    path: string;
}

function mergeContext(current: ComposerContext[], incoming: readonly ComposerContext[]): ComposerContext[] {
    return [...current, ...incoming.filter((item) => !current.some((other) => other.uri === item.uri))];
}

function readContext(item: ComposerContext): Promise<PromptContext> {
    if (item.text !== null) return Promise.resolve({ uri: item.uri, title: item.title, text: item.text });
    const { earlier } = item;
    if (earlier) return import("./earlierChats").then(({ wakeEarlier }) => wakeEarlier(earlier));
    if (!item.tracked) return Promise.reject(new Error(`${item.title} has nothing to send`));
    const { repo, kind, number } = item.tracked;
    return import("../codehost/tracked").then(({ loadTrackedContext }) =>
        loadTrackedContext(repo, kind, number).catch((failure: unknown) => {
            throw new Error(`Could not read #${number}: ${errorText(failure)}`);
        }),
    );
}

const TrackedSource = lazy(() => import("./TrackedSource"));
const WorktreeToggle = lazy(() => import("./ChatWorktree").then(({ WorktreeToggle }) => ({ default: WorktreeToggle })));

function Chip({
    title,
    mark,
    kind,
    label,
    removeLabel,
    onRemove,
    answering = false,
}: {
    title: string;
    mark: ReactNode;
    kind?: string;
    label: ReactNode;
    removeLabel: string;
    onRemove: () => void;
    answering?: boolean;
}) {
    return (
        <span className={`chat-context-chip${answering ? " answering" : ""}`} title={title}>
            <span className="chat-context-mark" data-kind={kind}>
                {mark}
            </span>
            <span>{label}</span>
            <button type="button" aria-label={removeLabel} onClick={onRemove}>
                <IconClose size={11} />
            </button>
        </span>
    );
}

function ComposerContextChip({ item, onRemove }: { item: ComposerContext; onRemove: () => void }) {
    if (item.earlier) {
        const { agent, title } = item.earlier.chat;
        return (
            <Chip
                title="Sending this opens the chat again under a new name and tells the agent who it is"
                mark={<AgentMark cli={agent} />}
                label={title}
                removeLabel={`Remove ${title}`}
                onRemove={onRemove}
            />
        );
    }
    const chip = contextChip(item);
    return (
        <Chip
            title={item.uri}
            mark={<ContextMark kind={chip.kind} size={14} />}
            kind={chip.kind ?? "other"}
            label={
                <>
                    {chip.number && <span className="chat-context-number">#{chip.number} </span>}
                    {chip.title}
                </>
            }
            removeLabel={`Remove ${chip.number ? `#${chip.number}` : chip.title}`}
            onRemove={onRemove}
        />
    );
}

export interface ComposerAnswering {
    agentId: string;
    name: string;
    cli: string;
    question: string;
    onRemove: () => void;
}

/* The composer keeps the draft to itself: a keystroke redraws these few rows
   rather than the transcript above them. */
export function ChatComposer({
    agent,
    profile,
    paneRef,
    visible,
    connection,
    running,
    steerable,
    commands,
    setup,
    awaitingPermission,
    agentLocked,
    changingConfig,
    changingPermissions,
    permissionApplied,
    placeholder,
    error,
    onError,
    onSend,
    onSteerQueued,
    onStop,
    queuedCount,
    usage,
    onConfig,
    history,
    worktree,
    answering,
    onQuestionKey,
}: {
    agent: Agent;
    profile?: ProviderProfile;
    paneRef: RefObject<HTMLDivElement | null>;
    visible: boolean;
    connection: ChatState["connection"];
    running: boolean;
    steerable: boolean;
    commands: AcpAvailableCommand[];
    setup: Record<string, unknown>;
    awaitingPermission: boolean;
    agentLocked: boolean;
    changingConfig: boolean;
    changingPermissions: boolean;
    permissionApplied: boolean;
    placeholder: string;
    error: string | null;
    onError: (message: string | null) => void;
    onSend: (message: OutgoingMessage, steerNow: boolean) => boolean;
    onSteerQueued: () => void;
    onStop: () => void;
    queuedCount: number;
    usage: ContextUsage | null;
    onConfig: (config: SessionConfig, value: string) => void;
    history: readonly string[];
    worktree?: { state: WorktreeSwitchState; toggle: () => void };
    /** The question an agent asked that whatever is sent answers. */
    answering?: ComposerAnswering | null;
    /** A key pressed in the empty box while the agent waits on questions; true when it did something with it. */
    onQuestionKey?: (key: string, primary: boolean) => boolean;
}) {
    const [draft, setDraft] = useState("");
    const draftNow = useRef(draft);
    draftNow.current = draft;
    useEffect(
        () =>
            registerComposer(agent.id, {
                take: () => {
                    const taken = draftNow.current;
                    draftNow.current = "";
                    setDraft("");
                    return taken;
                },
                put: (text) => setDraft((now) => (now.trim() ? `${now}\n${text}` : text)),
            }),
        [agent.id],
    );
    const [historyPosition, setHistoryPosition] = useState<HistoryPosition | null>(null);
    const recalledCaret = useRef<"start" | "end" | null>(null);
    const [caret, setCaret] = useState(0);
    const [attachments, setAttachments] = useState<string[]>([]);
    const attachmentsNow = useRef(attachments);
    attachmentsNow.current = attachments;
    const [contexts, setContexts] = useState<ComposerContext[]>([]);
    const [fileTags, setFileTags] = useState<FileTag[]>([]);
    const [reading, setReading] = useState(false);
    const [menuSelection, setMenuSelection] = useState(0);
    const [menuDismissed, setMenuDismissed] = useState(false);
    const editorRef = useRef<DraftBox>(null);
    const { cwd } = usePathRoots();

    /** Adds files to the message; with `inline`, each picture is also named where the caret is, which is where the agent sees it. */
    const attach = (paths: readonly string[], inline: boolean) => {
        const current = attachmentsNow.current;
        const merged = mergePaths(current, paths);
        if (merged.length === current.length) return;
        attachmentsNow.current = merged;
        setAttachments(merged);
        onError(null);
        const tags = merged.flatMap((path, index) => (index >= current.length && isImagePath(path) ? [imageTag(index + 1)] : []));
        const editor = editorRef.current;
        if (!inline || !editor || tags.length === 0) return;
        const written = writtenAtCaret(editor, tags.join(" "), true);
        setDraft(written.draft);
        setCaret(written.at);
        caretTo(editorRef, written.at);
    };
    const attachNow = useRef(attach);
    attachNow.current = attach;

    const detach = (path: string) => {
        const index = attachments.indexOf(path);
        setAttachments((current) => current.filter((candidate) => candidate !== path));
        if (index !== -1) setDraft((current) => withoutAttachment(current, index));
    };

    // A recalled message opens with the caret where the next arrow press keeps browsing.
    useLayoutEffect(() => {
        const editor = editorRef.current;
        if (!editor || !recalledCaret.current) return;
        const at = recalledCaret.current === "start" ? 0 : draft.length;
        editor.setSelectionRange(at, at);
        setCaret(at);
        recalledCaret.current = null;
    }, [draft]);

    useEffect(() => {
        const element = paneRef.current;
        if (!element) return;
        return registerPathDrop(element, (paths) => {
            attachNow.current(paths, true);
            window.requestAnimationFrame(() => editorRef.current?.focus());
        });
    }, [paneRef]);

    useEffect(() => {
        if (!visible) return;
        return receiveForAgent(agent.id, ({ text, paths, context }) => {
            if (paths?.length) attachNow.current(paths, false);
            if (context?.length)
                setContexts((current) =>
                    mergeContext(
                        current,
                        context.map((item) => ({ ...item, tracked: null })),
                    ),
                );
            const editor = editorRef.current;
            const current = editor?.value ?? "";
            const next = text ? `${current.trimEnd()}${current.trim() ? "\n\n" : ""}${text}` : current;
            setDraft(next);
            setCaret(next.length);
            caretTo(editorRef, next.length);
        });
    }, [agent.id, visible]);

    useEffect(() => {
        const element = paneRef.current;
        if (!element) return;
        return registerTextInsert(element, (text) => {
            const editor = editorRef.current;
            if (!editor) return;
            const written = writtenAtCaret(editor, text, false);
            setDraft(written.draft);
            setCaret(written.at);
            caretTo(editorRef, written.at);
        });
    }, [paneRef]);

    /* A chat is focused again once its session is ready, not only when its pane
       appears: a pane opened while the agent was still starting would otherwise
       keep the caret wherever it was. */
    useEffect(() => {
        if (!visible) return;
        const focusIsElsewhere = () => {
            const held = document.activeElement;
            if (held?.closest('input, textarea, [contenteditable="true"], [data-desk]') && !paneRef.current?.contains(held)) return true;
            return Boolean(held?.closest(".chat-picker-menu"));
        };
        if (focusIsElsewhere()) return;
        // Asked again when the frame runs: a menu opened since this was queued keeps its focus.
        const frame = window.requestAnimationFrame(() => {
            if (!focusIsElsewhere()) editorRef.current?.focus();
        });
        return () => window.cancelAnimationFrame(frame);
    }, [connection, paneRef, visible]);

    const mates = useMates(agent.id);
    const chips: DraftChip[] = [
        ...contexts.flatMap((item) =>
            item.tag && item.tracked ? [{ text: item.tag, kind: item.tracked.kind, label: contextChip(item).title }] : [],
        ),
        ...contexts.flatMap(({ tag, earlier }) =>
            tag && earlier?.named
                ? [
                      {
                          text: tag,
                          kind: "agent" as const,
                          agent: { id: earlier.named.agentId, type: earlier.chat.agent, buddy: earlier.named.buddy, name: earlier.named.name },
                      },
                  ]
                : [],
        ),
        ...fileTags.map((file) => ({ text: file.tag, kind: "file" as const, label: basename(file.path) })),
        ...attachments.flatMap((path, index) =>
            isImagePath(path) ? [{ text: imageTag(index + 1), kind: "image" as const, label: basename(path) }] : [],
        ),
        ...mates.map((mate) => ({ text: `@${mate.name}`, kind: "agent" as const, agent: mate })),
    ];
    const tagTexts = chips.map((chip) => chip.text);
    const typed = menuDismissed ? null : tokenAt(draft, caret);
    // A finished tag is not a search: the menu stays shut when the caret lands right after one.
    const token = typed && !tagTexts.includes(`${typed.trigger}${typed.needle}`) ? typed : null;
    const sentContexts = contexts.filter((item) => !item.tag || hasTag(draft, item.tag));
    const sentFiles = fileTags.filter((file) => hasTag(draft, file.tag));

    const placeCaret = (position: number) => {
        setCaret(position);
        setMenuDismissed(true);
        caretTo(editorRef, position);
    };

    const takeToken = () => {
        if (!token) return;
        const next = removeToken(draft, token, caret);
        setDraft(next.text);
        placeCaret(next.caret);
    };

    const [trackedList, setTrackedList] = useState<TrackedMatches>({ state: "loading" });
    const menuKeys = useRef<MenuKeys | null>(null);

    const selectCommand = (command: AcpAvailableCommand) => {
        if (!token) return;
        const spaced = Boolean(command.input?.hint) && !/^\s/.test(draft.slice(caret));
        const written = `/${command.name}${spaced ? " " : ""}`;
        setDraft(`${draft.slice(0, token.start)}${written}${draft.slice(caret)}`);
        placeCaret(token.start + written.length);
    };

    const selectMate = (mate: Mate) => {
        if (!token) return;
        const written = `@${mate.name} `;
        setDraft(`${draft.slice(0, token.start)}${written}${draft.slice(caret).replace(/^ /, "")}`);
        placeCaret(token.start + written.length);
    };

    const selectEarlier = (earlier: EarlierChat, uri: string) => {
        if (earlier.named) {
            const tag = `@${earlier.named.name}`;
            writeTag(tag);
            setContexts((current) => mergeContext(current, [{ uri, title: earlier.chat.title, text: null, tracked: null, earlier, tag }]));
            return;
        }
        takeToken();
        setContexts((current) => mergeContext(current, [{ uri, title: earlier.chat.title, text: null, tracked: null, earlier }]));
        onError(null);
    };

    /** Writes a picked item over the token being typed, as a tag followed by a space. */
    const writeTag = (tag: string) => {
        if (!token) return;
        setDraft(`${draft.slice(0, token.start)}${tag} ${draft.slice(caret).replace(/^ /, "")}`);
        placeCaret(token.start + tag.length + 1);
        onError(null);
    };

    const selectEntry = (entry: ProjectEntry) => {
        const path = joinPath(cwd, entry.path);
        const named = `+${basename(entry.path)}`;
        const clash = fileTags.some((file) => file.tag === named && file.path !== path);
        const tag = clash ? `+${entry.path.replace(/\/$/, "")}` : named;
        writeTag(tag);
        setFileTags((current) => [...current.filter((file) => file.tag !== tag), { tag, path }]);
    };

    const selectTracked = (item: TrackedItem, repo: RepoRef) => {
        const tag = `${item.kind === "pull" ? "!" : "#"}${item.number}`;
        writeTag(tag);
        const chosen = {
            uri: item.url,
            title: `#${item.number} ${item.title}`,
            text: null,
            tracked: { repo, kind: item.kind, number: item.number },
            tag,
        };
        setContexts((current) => mergeContext(current, [chosen]));
    };

    /** Moves the search to another prefix, keeping what was typed after it. */
    const switchScope = (trigger: ComposerTrigger) => {
        if (!token || token.trigger === trigger) return;
        setDraft(`${draft.slice(0, token.start)}${trigger}${draft.slice(token.start + 1)}`);
        setMenuSelection(0);
        caretTo(editorRef, caret);
    };

    const pick = (choice: MenuPick) => {
        if (choice.kind === "command") selectCommand(choice.command);
        else if (choice.kind === "mate") selectMate(choice.mate);
        else if (choice.kind === "earlier") selectEarlier(choice.earlier, choice.uri);
        else if (choice.kind === "entry") selectEntry(choice.entry);
        else selectTracked(choice.item, choice.repo);
    };

    const blocked = changingConfig || changingPermissions || !permissionApplied || worktree?.state.kind === "preparing";
    const drafted = Boolean(draft.trim()) || attachments.length > 0 || sentContexts.length > 0;

    // Send and stop are one button: when it changes job, the new icon turns in rather than swapping in place.
    const dictating = useVoice(
        (state) => (state.phase === "listening" || state.phase === "transcribing") && writesInto(state.target, paneRef.current),
    );
    const heard = useVoice((state) =>
        (state.phase === "listening" || state.phase === "transcribing") && writesInto(state.target, paneRef.current) ? state.partial : "",
    );
    const stopping = running && !drafted && !dictating;
    useEffect(() => {
        if (dictating) editorRef.current?.focus();
    }, [dictating]);
    const sendButton = useRef<HTMLButtonElement>(null);
    const wasStopping = useRef(stopping);
    useLayoutEffect(() => {
        if (wasStopping.current === stopping) return;
        wasStopping.current = stopping;
        animate(
            sendButton.current?.firstElementChild,
            [
                { opacity: 0, transform: `scale(0.5) rotate(${stopping ? -90 : 90}deg)` },
                { opacity: 1, transform: "none" },
            ],
            {
                duration: 150,
            },
        );
    }, [stopping]);

    const canSteerQueued = running && steerable && queuedCount > 0;
    const steersByDefault = useStore((s) => s.midTurnSend) === "steer";

    const clear = () => {
        setDraft("");
        setCaret(0);
        setMenuSelection(0);
        setAttachments([]);
        setContexts([]);
        setFileTags([]);
        setMenuDismissed(false);
        setHistoryPosition(null);
    };

    // Sending while the mic is on stops it first, then sends once what was said is in the box.
    const sendAfterDictation = useRef<boolean | null>(null);
    useEffect(() => {
        if (dictating || sendAfterDictation.current === null) return;
        const steerNow = sendAfterDictation.current;
        sendAfterDictation.current = null;
        send(steerNow);
    });

    const send = (steerNow = false) => {
        if (finishDictationInto(paneRef.current)) {
            sendAfterDictation.current = steerNow;
            return;
        }
        const text = draft.trim();
        if (!drafted || blocked || reading) return;
        const paths = mergePaths(
            attachments,
            sentFiles.map((file) => file.path),
        );
        const mentioned = mentionContext(text, mates);
        if (sentContexts.every((item) => item.text !== null)) {
            const context = [...sentContexts.map((item) => ({ uri: item.uri, title: item.title, text: item.text ?? "" })), ...mentioned];
            if (onSend({ text, paths, context }, steerNow)) clear();
            return;
        }
        setReading(true);
        void Promise.all(sentContexts.map(readContext))
            .then((context) => {
                if (onSend({ text, paths, context: [...context, ...mentioned] }, steerNow)) clear();
            })
            .catch((failure: unknown) => onError(errorText(failure)))
            .finally(() => setReading(false));
    };

    const chooseFiles = async () => {
        try {
            const selection = await dialogsApi.pickFiles();
            if (selection.length === 0) return;
            attach(selection, true);
            window.requestAnimationFrame(() => editorRef.current?.focus());
        } catch (failure) {
            onError(errorText(failure));
        }
    };

    return (
        <div className="chat-composer">
            {(token?.trigger === "#" || token?.trigger === "!") && (
                <Suspense fallback={null}>
                    <TrackedSource
                        cwd={cwd}
                        kind={token.trigger === "#" ? "issue" : "pull"}
                        needle={token.needle}
                        limit={MAX_MENU_ENTRIES}
                        onMatches={setTrackedList}
                    />
                </Suspense>
            )}
            {token && (
                <ComposerMenu
                    token={token}
                    agentId={agent.id}
                    cwd={cwd}
                    commands={commands}
                    mates={mates}
                    taken={contexts.map((item) => item.uri)}
                    tracked={trackedList}
                    selected={menuSelection}
                    keys={menuKeys}
                    onPick={pick}
                    onScope={switchScope}
                    peek={<ComposerBuddy agent={agent} />}
                />
            )}
            <div className="chat-field">
                {(attachments.length > 0 || contexts.some((item) => !item.tag) || answering) && (
                    <div className="chat-attachments">
                        {answering && (
                            <Chip
                                answering
                                title={answering.question}
                                mark={<AgentMark agentId={answering.agentId} cli={answering.cli} />}
                                label={`Answering ${answering.name} · ${answering.question}`}
                                removeLabel="Stop answering"
                                onRemove={answering.onRemove}
                            />
                        )}
                        {attachments.map((path, index) => (
                            <ComposerAttachment key={path} path={path} number={index + 1} onRemove={() => detach(path)} />
                        ))}
                        {contexts
                            .filter((item) => !item.tag)
                            .map((item) => (
                                <ComposerContextChip
                                    key={item.uri}
                                    item={item}
                                    onRemove={() => setContexts((current) => current.filter((candidate) => candidate.uri !== item.uri))}
                                />
                            ))}
                    </div>
                )}
                <DraftEditor
                    box={editorRef}
                    value={draft}
                    chips={chips}
                    label="Message agent"
                    placeholder={heard ? "" : placeholder}
                    onChange={(value, at) => {
                        setDraft(value);
                        setCaret(at);
                        setMenuSelection(0);
                        setMenuDismissed(false);
                        onError(null);
                    }}
                    onCaret={setCaret}
                    onPaste={(event) => {
                        if (imagesInClipboard(event.clipboardData).length > 0) event.preventDefault();
                        void savePastedClipboard(event.clipboardData)
                            .then((paths) => attachNow.current(paths, true))
                            .catch((failure) => onError(errorText(failure)));
                    }}
                    onKeyDown={(event) => {
                        const box = editorRef.current;
                        if (!box) return;
                        if (event.key === "Backspace" && !event.shiftKey && !event.altKey && !event.metaKey && !event.ctrlKey) {
                            const start = box.selectionStart === box.selectionEnd ? tagBefore(box.value, box.selectionStart, tagTexts) : null;
                            if (start !== null) {
                                event.preventDefault();
                                box.setSelectionRange(start, box.selectionStart);
                                return;
                            }
                        }
                        const menu = token && menuKeys.current;
                        if (menu) {
                            if (event.key === "Escape") {
                                event.preventDefault();
                                setMenuDismissed(true);
                                return;
                            }
                            if (menu.count > 0 && !event.nativeEvent.isComposing) {
                                if (event.key === "ArrowDown") {
                                    event.preventDefault();
                                    setMenuSelection((current) => (current + 1) % menu.count);
                                    return;
                                }
                                if (event.key === "ArrowUp") {
                                    event.preventDefault();
                                    setMenuSelection((current) => (current - 1 + menu.count) % menu.count);
                                    return;
                                }
                                if (event.key === "Tab" || event.key === "Enter") {
                                    event.preventDefault();
                                    menu.choose(Math.min(menuSelection, menu.count - 1));
                                    return;
                                }
                            }
                        }
                        const plainKey = !event.shiftKey && !event.altKey && !event.metaKey && !event.ctrlKey && !event.nativeEvent.isComposing;
                        const primary = hasPrimaryModifier(event.nativeEvent);
                        const questionKey = (plainKey || (primary && !event.shiftKey && !event.altKey)) && !drafted;
                        if (questionKey && onQuestionKey?.(event.key, primary)) {
                            event.preventDefault();
                            return;
                        }
                        // Escape first lets go of a selection. It stops a running turn only from an empty box, so a half-written message never stops the agent.
                        if (event.key === "Escape" && plainKey) {
                            if (box.selectionStart !== box.selectionEnd) {
                                event.preventDefault();
                                box.setSelectionRange(box.selectionEnd, box.selectionEnd);
                                return;
                            }
                            if (running && !drafted) {
                                event.preventDefault();
                                onStop();
                                return;
                            }
                        }
                        const direction = event.key === "ArrowUp" ? "older" : event.key === "ArrowDown" ? "newer" : null;
                        if (direction && plainKey) {
                            const { value } = box;
                            const recalled = arrowsBrowse(value, history, historyPosition, direction)
                                ? recallPrompt(history, historyPosition, value, direction)
                                : null;
                            if (recalled) {
                                event.preventDefault();
                                recalledCaret.current = direction === "older" ? "start" : "end";
                                setHistoryPosition(recalled.position);
                                setDraft(recalled.draft);
                                setMenuDismissed(true);
                                return;
                            }
                        }
                        if (event.key === "Enter" && !event.shiftKey && !event.nativeEvent.isComposing) {
                            event.preventDefault();
                            if (hasPrimaryModifier(event.nativeEvent) && !drafted && canSteerQueued) {
                                onSteerQueued();
                                return;
                            }
                            if (finishDictationInto(paneRef.current)) return;
                            send(hasPrimaryModifier(event.nativeEvent) !== steersByDefault);
                        }
                    }}
                />
                {heard && <DictationPreview draft={draft} caret={caret} heard={heard} editor={editorRef} />}
            </div>
            {error && <div className="chat-composer-error">{error}</div>}
            <div className="chat-composer-bar">
                <button type="button" className="chat-composer-icon" aria-label="Add files" onClick={() => void chooseFiles()}>
                    <IconPlus size={17} />
                </button>
                {canSwitchMode(agent.type) && (
                    <YoloToggle
                        agent={agent}
                        relaunches={false}
                        disabled={
                            connection !== "ready" || changingConfig || running || awaitingPermission || changingPermissions || !permissionApplied
                        }
                    />
                )}
                {(worktree?.state.kind === "preparing" || worktree?.state.kind === "in") && (
                    <Suspense fallback={null}>
                        <WorktreeToggle state={worktree.state} onToggle={worktree.toggle} />
                    </Suspense>
                )}
                <ComposerPickers
                    agent={agent}
                    profile={profile}
                    setup={setup}
                    agentLocked={agentLocked}
                    running={running}
                    moveDisabled={connection !== "ready" || changingPermissions || changingConfig}
                    disabled={connection !== "ready" || running || changingPermissions || changingConfig || awaitingPermission}
                    onAgent={(type, profileId, model, modelProvider) => {
                        if (agentLocked || running || awaitingPermission) return;
                        cmd.configureEmptyAgent(agent.id, type, profileId, model, modelProvider);
                    }}
                    onConfig={onConfig}
                />
                <ContextMeter usage={usage} agent={agent.type} />
                <span className="chat-composer-spacer" />
                <DictateButton into={paneRef} />
                {stopping ? (
                    <button ref={sendButton} type="button" className="chat-send stop" aria-label="Stop agent" onClick={onStop}>
                        <span />
                    </button>
                ) : (
                    <button
                        ref={sendButton}
                        type="button"
                        className="chat-send"
                        aria-label="Send message"
                        title={
                            canSteerQueued
                                ? `${PRIMARY_SHORTCUT}↵ steers ${queuedCount === 1 ? "the queued message" : "every queued message"} into this turn`
                                : running && steerable
                                  ? steersByDefault
                                      ? `Steers into this turn — ${PRIMARY_SHORTCUT}↵ queues behind it`
                                      : `Queues behind this turn — ${PRIMARY_SHORTCUT}↵ steers into it`
                                  : undefined
                        }
                        disabled={blocked || reading || (!drafted && !dictating)}
                        onClick={() => send(steersByDefault)}>
                        <IconArrowUp size={15} />
                    </button>
                )}
            </div>
        </div>
    );
}
