import { lazy, Suspense, useEffect, useLayoutEffect, useMemo, useRef, useState, type ReactNode, type RefObject } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { agentSupportsSkipPermissions } from "../state/commands/agentLogic";
import { ComposerPickers } from "./ComposerPickers";
import type { SessionConfig } from "./sessionConfig";
import { basename, dirname, joinPath } from "../lib/paths";
import { animate } from "../lib/motion";
import { hasPrimaryModifier, PRIMARY_SHORTCUT } from "../lib/platform";
import { registerPathDrop } from "../state/dropRegistry";
import { registerTextInsert } from "../state/textInsertRegistry";
import type { Agent, ProviderProfile } from "../state/types";
import * as cmd from "../state/commands";
import { IconArrowUp, IconClose, IconFolder, IconPlus } from "../ui/Icons";
import { FileIcon, FileTypeIcon } from "../ui/FileIcon";
import { useResourceEnabled } from "../state/resources";
import { filesListR } from "../state/resources.defs";
import { usePathRoots } from "./FileRef";
import { useImagePreview } from "./imagePreview";
import { YoloToggle } from "./YoloToggle";
import type { WorktreeSwitchState } from "./worktreeSwitch";
import { DictateButton } from "./DictateButton";
import { ContextMeter } from "./ContextMeter";
import { imagesInClipboard, savePastedClipboard } from "./pasteImage";
import { arrowsBrowse, recallPrompt, type HistoryPosition } from "./promptHistory";
import { entryName, mergePaths, projectEntries, rankEntries, removeToken, tokenAt, type ProjectEntry } from "./composerInput";
import { receiveForAgent } from "../agents/agentInbox";
import type { AcpAvailableCommand, ChatState, ContextUsage } from "./types";
import type { OutgoingMessage } from "./queuedMessages";
import type { PromptContext } from "../api/acp";
import type { RepoRef } from "../codehost/types";
import type { TrackedItem, TrackedKind } from "../codehost/tracked";
import type { TrackedMatches } from "./TrackedSource";
import { ContextMark } from "./ContextChip";
import { contextChip } from "./promptContext";

function ComposerAttachment({ path, onRemove }: { path: string; onRemove: () => void }) {
    const preview = useImagePreview(path);
    return (
        <span className={preview ? "image" : "file"} title={path}>
            {preview ? <img alt={basename(path)} src={preview} /> : <FileTypeIcon name={basename(path)} size={34} />}
            <button type="button" aria-label={`Remove ${basename(path)}`} onClick={onRemove}>
                <IconClose size={10} />
            </button>
        </span>
    );
}

interface MenuRow {
    key: string;
    content: ReactNode;
    choose: () => void;
}

interface ComposerMenuModel {
    label: string;
    heading: string;
    aside: string;
    variant: "commands" | "entries";
    rows: MenuRow[];
    empty: string | null;
}

function ComposerMenu({ menu, selected }: { menu: ComposerMenuModel; selected: number }) {
    return (
        <div className={`chat-slash-menu ${menu.variant}`} role="listbox" aria-label={menu.label}>
            <div className="chat-slash-heading">
                <span>{menu.heading}</span>
                <span>{menu.aside}</span>
            </div>
            {menu.rows.length === 0 && menu.empty && <div className="chat-slash-empty">{menu.empty}</div>}
            {menu.rows.map((row, index) => (
                <button
                    key={row.key}
                    type="button"
                    role="option"
                    aria-selected={index === selected}
                    className={index === selected ? "selected" : ""}
                    onMouseDown={(event) => event.preventDefault()}
                    onClick={row.choose}>
                    {row.content}
                </button>
            ))}
        </div>
    );
}

const MAX_ENTRIES = 8;

/** An item to hand over with the message. One picked with `#` is read in full only when the message is sent. */
interface ComposerContext {
    uri: string;
    title: string;
    text: string | null;
    tracked: { repo: RepoRef; kind: TrackedKind; number: number } | null;
}

function mergeContext(current: ComposerContext[], incoming: readonly ComposerContext[]): ComposerContext[] {
    return [...current, ...incoming.filter((item) => !current.some((other) => other.uri === item.uri))];
}

function readContext(item: ComposerContext): Promise<PromptContext> {
    if (item.text !== null) return Promise.resolve({ uri: item.uri, title: item.title, text: item.text });
    if (!item.tracked) return Promise.reject(new Error(`${item.title} has nothing to send`));
    const { repo, kind, number } = item.tracked;
    return Promise.all([import("../codehost/tracked"), import("../codehost/api")]).then(([{ loadTrackedContext }, { failureMessage }]) =>
        loadTrackedContext(repo, kind, number).catch((failure: unknown) => {
            throw new Error(`Could not read #${number}: ${failureMessage(failure)}`);
        }),
    );
}

const TrackedSource = lazy(() => import("./TrackedSource"));
const WorktreeToggle = lazy(() => import("./ChatWorktree").then(({ WorktreeToggle }) => ({ default: WorktreeToggle })));

function ComposerContextChip({ item, onRemove }: { item: ComposerContext; onRemove: () => void }) {
    const chip = contextChip(item);
    const name = chip.number ? `#${chip.number}` : chip.title;
    return (
        <span className="chat-context-chip" title={item.uri}>
            <span className="chat-context-mark" data-kind={chip.kind ?? "other"}>
                <ContextMark kind={chip.kind} size={14} />
            </span>
            <span>
                {chip.number && <span className="chat-context-number">#{chip.number} </span>}
                {chip.title}
            </span>
            <button type="button" aria-label={`Remove ${name}`} onClick={onRemove}>
                <IconClose size={11} />
            </button>
        </span>
    );
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
}) {
    const [draft, setDraft] = useState("");
    const [historyPosition, setHistoryPosition] = useState<HistoryPosition | null>(null);
    const recalledCaret = useRef<"start" | "end" | null>(null);
    const [caret, setCaret] = useState(0);
    const [attachments, setAttachments] = useState<string[]>([]);
    const [contexts, setContexts] = useState<ComposerContext[]>([]);
    const [reading, setReading] = useState(false);
    const [menuSelection, setMenuSelection] = useState(0);
    const [menuDismissed, setMenuDismissed] = useState(false);
    const editorRef = useRef<HTMLTextAreaElement>(null);
    const { cwd } = usePathRoots();

    /* The field grows with what is typed until it reaches its CSS max-height,
       and scrolls from there. */
    useLayoutEffect(() => {
        const editor = editorRef.current;
        if (!editor) return;
        editor.style.height = "auto";
        editor.style.height = `${editor.scrollHeight}px`;
        // A recalled message opens with the caret where the next arrow press keeps browsing.
        if (recalledCaret.current) {
            const at = recalledCaret.current === "start" ? 0 : draft.length;
            editor.setSelectionRange(at, at);
            setCaret(at);
            recalledCaret.current = null;
        }
    }, [draft]);

    useEffect(() => {
        const element = paneRef.current;
        if (!element) return;
        return registerPathDrop(element, (paths) => {
            setAttachments((current) => mergePaths(current, paths));
            onError(null);
            window.requestAnimationFrame(() => editorRef.current?.focus());
        });
    }, [onError, paneRef]);

    useEffect(() => {
        if (!visible) return;
        return receiveForAgent(agent.id, ({ text, paths, context }) => {
            if (paths?.length) setAttachments((current) => mergePaths(current, paths));
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
            window.requestAnimationFrame(() => {
                editorRef.current?.focus();
                editorRef.current?.setSelectionRange(next.length, next.length);
            });
        });
    }, [agent.id, visible]);

    useEffect(() => {
        const element = paneRef.current;
        if (!element) return;
        return registerTextInsert(element, (text) => {
            const editor = editorRef.current;
            if (!editor) return;
            const before = editor.value.slice(0, editor.selectionStart);
            const after = editor.value.slice(editor.selectionEnd);
            const inserted = `${before && !/\s$/.test(before) ? " " : ""}${text}`;
            const at = before.length + inserted.length;
            setDraft(`${before}${inserted}${after}`);
            setCaret(at);
            window.requestAnimationFrame(() => {
                editor.focus();
                editor.setSelectionRange(at, at);
            });
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

    const token = menuDismissed ? null : tokenAt(draft, caret);

    const placeCaret = (position: number) => {
        setCaret(position);
        setMenuDismissed(true);
        window.requestAnimationFrame(() => {
            const editor = editorRef.current;
            if (!editor) return;
            editor.focus();
            editor.setSelectionRange(position, position);
        });
    };

    const takeToken = () => {
        if (!token) return;
        const next = removeToken(draft, token, caret);
        setDraft(next.text);
        placeCaret(next.caret);
    };

    const slashCommands = useMemo(() => {
        if (token?.trigger !== "/") return [];
        const needle = token.needle.toLowerCase();
        return commands.filter((command) => command.name.toLowerCase().includes(needle)).slice(0, 8);
    }, [commands, token?.trigger, token?.needle]);

    const files = useResourceEnabled(token?.trigger === "@" && !!cwd, filesListR, cwd);
    const entries = useMemo(() => projectEntries(files.data ?? []), [files.data]);
    const entryMatches = useMemo(
        () => (token?.trigger === "@" ? rankEntries(token.needle, entries, MAX_ENTRIES) : []),
        [entries, token?.trigger, token?.needle],
    );

    const [trackedList, setTrackedList] = useState<TrackedMatches>({ state: "loading" });
    const trackedMatches = token?.trigger === "#" && trackedList.state === "ready" ? trackedList.matches : [];

    const selectCommand = (command: AcpAvailableCommand) => {
        if (!token) return;
        const spaced = Boolean(command.input?.hint) && !/^\s/.test(draft.slice(caret));
        const written = `/${command.name}${spaced ? " " : ""}`;
        setDraft(`${draft.slice(0, token.start)}${written}${draft.slice(caret)}`);
        placeCaret(token.start + written.length);
    };

    const selectEntry = (entry: ProjectEntry) => {
        takeToken();
        setAttachments((current) => mergePaths(current, [joinPath(cwd, entry.path)]));
        onError(null);
    };

    const selectTracked = (item: TrackedItem) => {
        if (trackedList.state !== "ready") return;
        takeToken();
        const chosen = {
            uri: item.url,
            title: `#${item.number} ${item.title}`,
            text: null,
            tracked: { repo: trackedList.repo, kind: item.kind, number: item.number },
        };
        setContexts((current) => mergeContext(current, [chosen]));
        onError(null);
    };

    const menu: ComposerMenuModel | null =
        token?.trigger === "/" && slashCommands.length > 0
            ? {
                  label: "Session commands",
                  heading: "Session commands",
                  aside: "ACP",
                  variant: "commands",
                  empty: null,
                  rows: slashCommands.map((command) => ({
                      key: command.name,
                      choose: () => selectCommand(command),
                      content: (
                          <>
                              <code>/{command.name}</code>
                              <span>{command.description}</span>
                              {command.input?.hint && <em>{command.input.hint}</em>}
                          </>
                      ),
                  })),
              }
            : token?.trigger === "@" && cwd
              ? {
                    label: "Project files",
                    heading: "Files and folders",
                    aside: "attach",
                    variant: "entries",
                    empty: files.data === undefined ? "Reading the project…" : "Nothing in the project matches",
                    rows: entryMatches.map((entry) => ({
                        key: entry.path,
                        choose: () => selectEntry(entry),
                        content: (
                            <>
                                <span className="chat-menu-mark">
                                    {entry.folder ? <IconFolder size={13} /> : <FileIcon name={entryName(entry)} size={13} />}
                                </span>
                                <code>{entryName(entry)}</code>
                                <span>{dirname(entry.path.replace(/\/$/, "")) || "."}</span>
                            </>
                        ),
                    })),
                }
              : token?.trigger === "#"
                ? {
                      label: "Issues and pull requests",
                      heading: "Issues and pull requests",
                      aside: trackedList.state === "ready" ? `${trackedList.repo.owner}/${trackedList.repo.name}` : "",
                      variant: "entries",
                      empty:
                          trackedList.state === "loading"
                              ? "Reading open issues and pull requests…"
                              : trackedList.state === "unavailable"
                                ? trackedList.message
                                : "No open issue or pull request matches",
                      rows: trackedMatches.map((item) => ({
                          key: `${item.kind}-${item.number}`,
                          choose: () => selectTracked(item),
                          content: (
                              <>
                                  <span className="chat-menu-mark" data-kind={item.kind}>
                                      <ContextMark kind={item.kind} size={13} />
                                  </span>
                                  <code>#{item.number}</code>
                                  <span>{item.title}</span>
                              </>
                          ),
                      })),
                  }
                : null;
    const menuRows = menu?.rows ?? [];
    const selected = Math.min(menuSelection, Math.max(0, menuRows.length - 1));

    const blocked = changingConfig || changingPermissions || !permissionApplied || worktree?.state.kind === "preparing";
    const drafted = Boolean(draft.trim()) || attachments.length > 0 || contexts.length > 0;

    // Send and stop are one button: when it changes job, the new icon turns in rather than swapping in place.
    const stopping = running && !drafted;
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

    const clear = () => {
        setDraft("");
        setCaret(0);
        setMenuSelection(0);
        setAttachments([]);
        setContexts([]);
        setMenuDismissed(false);
        setHistoryPosition(null);
    };

    const send = (steerNow = false) => {
        const text = draft.trim();
        if (!drafted || blocked || reading) return;
        const paths = attachments;
        if (contexts.every((item) => item.text !== null)) {
            if (onSend({ text, paths, context: contexts.map((item) => ({ uri: item.uri, title: item.title, text: item.text ?? "" })) }, steerNow))
                clear();
            return;
        }
        setReading(true);
        void Promise.all(contexts.map(readContext))
            .then((context) => {
                if (onSend({ text, paths, context }, steerNow)) clear();
            })
            .catch((failure: unknown) => onError(failure instanceof Error ? failure.message : String(failure)))
            .finally(() => setReading(false));
    };

    const chooseFiles = async () => {
        try {
            const selection = await open({ multiple: true, directory: false });
            if (!selection) return;
            setAttachments((current) => mergePaths(current, Array.isArray(selection) ? selection : [selection]));
            window.requestAnimationFrame(() => editorRef.current?.focus());
        } catch (failure) {
            onError(failure instanceof Error ? failure.message : String(failure));
        }
    };

    return (
        <div className="chat-composer">
            {token?.trigger === "#" && (
                <Suspense fallback={null}>
                    <TrackedSource cwd={cwd} needle={token.needle} limit={MAX_ENTRIES} onMatches={setTrackedList} />
                </Suspense>
            )}
            {menu && <ComposerMenu menu={menu} selected={selected} />}
            <div className="chat-field">
                {(attachments.length > 0 || contexts.length > 0) && (
                    <div className="chat-attachments">
                        {attachments.map((path) => (
                            <ComposerAttachment
                                key={path}
                                path={path}
                                onRemove={() => setAttachments((current) => current.filter((candidate) => candidate !== path))}
                            />
                        ))}
                        {contexts.map((item) => (
                            <ComposerContextChip
                                key={item.uri}
                                item={item}
                                onRemove={() => setContexts((current) => current.filter((candidate) => candidate.uri !== item.uri))}
                            />
                        ))}
                    </div>
                )}
                <textarea
                    ref={editorRef}
                    value={draft}
                    aria-label="Message agent"
                    placeholder={placeholder}
                    rows={2}
                    onChange={(event) => {
                        setDraft(event.target.value);
                        setCaret(event.target.selectionStart);
                        setMenuSelection(0);
                        setMenuDismissed(false);
                        onError(null);
                    }}
                    onSelect={(event) => setCaret(event.currentTarget.selectionStart)}
                    onPaste={(event) => {
                        if (imagesInClipboard(event.clipboardData).length > 0) event.preventDefault();
                        void savePastedClipboard(event.clipboardData)
                            .then((paths) => {
                                if (paths.length === 0) return;
                                setAttachments((current) => mergePaths(current, paths));
                                onError(null);
                            })
                            .catch((failure) => onError(failure instanceof Error ? failure.message : String(failure)));
                    }}
                    onKeyDown={(event) => {
                        if (menu) {
                            if (event.key === "Escape") {
                                event.preventDefault();
                                setMenuDismissed(true);
                                return;
                            }
                            if (menuRows.length > 0 && !event.nativeEvent.isComposing) {
                                if (event.key === "ArrowDown") {
                                    event.preventDefault();
                                    setMenuSelection((current) => (current + 1) % menuRows.length);
                                    return;
                                }
                                if (event.key === "ArrowUp") {
                                    event.preventDefault();
                                    setMenuSelection((current) => (current - 1 + menuRows.length) % menuRows.length);
                                    return;
                                }
                                if (event.key === "Tab" || event.key === "Enter") {
                                    event.preventDefault();
                                    menuRows[selected].choose();
                                    return;
                                }
                            }
                        }
                        const plainKey = !event.shiftKey && !event.altKey && !event.metaKey && !event.ctrlKey && !event.nativeEvent.isComposing;
                        // As in an agent's terminal, Escape stops the turn it is running; whatever is typed stays.
                        if (event.key === "Escape" && running && plainKey) {
                            event.preventDefault();
                            onStop();
                            return;
                        }
                        const direction = event.key === "ArrowUp" ? "older" : event.key === "ArrowDown" ? "newer" : null;
                        if (direction && plainKey) {
                            const { value } = event.currentTarget;
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
                            send(hasPrimaryModifier(event.nativeEvent));
                        }
                    }}
                />
            </div>
            {error && <div className="chat-composer-error">{error}</div>}
            <div className="chat-composer-bar">
                <button type="button" className="chat-composer-icon" aria-label="Add files" onClick={() => void chooseFiles()}>
                    <IconPlus size={17} />
                </button>
                {agentSupportsSkipPermissions(agent.type) && (
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
                    disabled={connection !== "ready" || running || changingPermissions || changingConfig || awaitingPermission}
                    onAgent={(type, profileId) => {
                        if (agentLocked || running || awaitingPermission) return;
                        cmd.configureEmptyAgent(agent.id, type, profileId);
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
                                  ? `Queues behind this turn — ${PRIMARY_SHORTCUT}↵ steers into it`
                                  : undefined
                        }
                        disabled={blocked || reading || !drafted}
                        onClick={() => send()}>
                        <IconArrowUp size={15} />
                    </button>
                )}
            </div>
        </div>
    );
}
