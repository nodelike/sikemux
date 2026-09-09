import { memo, useEffect, useLayoutEffect, useMemo, useReducer, useRef, useState } from "react";
import Markdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { useVirtualizer } from "@tanstack/react-virtual";
import { open } from "@tauri-apps/plugin-dialog";
import { acpApi, type AcpEvent } from "../api/acp";
import { invokeCommand as invoke } from "../api/invoke";
import { ComposerPickers, sessionConfigs, type SessionConfig } from "./ComposerPickers";
import { permissionCopyForType } from "../agentLaunch";
import { basename } from "../lib/paths";
import { registerPathDrop } from "../state/dropRegistry";
import type { Agent, ProviderProfile } from "../state/types";
import * as cmd from "../state/commands";
import { swallow } from "../state/toast";
import { IconArrowDown, IconArrowUp, IconCheck, IconClose, IconCommand, IconFile, IconPlus, IconShieldBolt, IconWarning } from "../components/Icons";
import { chatReducer, initialChatState } from "./reducer";
import type { AcpAvailableCommand, AcpPermissionRequest, AcpToolCall, ChatMessage, ChatPart } from "./types";

const MAX_ATTACHMENTS = 32;
const MAX_DETAIL_CHARS = 120_000;

function recordOf(value: unknown): Record<string, unknown> | null {
    return typeof value === "object" && value !== null && !Array.isArray(value) ? (value as Record<string, unknown>) : null;
}

function eventMessage(event: AcpEvent): string {
    return typeof event.payload.message === "string" ? event.payload.message : "ACP session failed";
}

function formatDetail(value: unknown): string {
    let formatted: string;
    try {
        formatted = JSON.stringify(value, null, 2) ?? String(value);
    } catch {
        formatted = String(value);
    }
    return formatted.length > MAX_DETAIL_CHARS ? `${formatted.slice(0, MAX_DETAIL_CHARS)}\n… output truncated` : formatted;
}

function permissionRequest(payload: Record<string, unknown>): AcpPermissionRequest | null {
    const requestId = typeof payload.requestId === "string" ? payload.requestId : null;
    const sessionId = typeof payload.sessionId === "string" ? payload.sessionId : null;
    const toolCall = recordOf(payload.toolCall);
    const options = Array.isArray(payload.options) ? payload.options : null;
    if (!requestId || !sessionId || !toolCall || typeof toolCall.toolCallId !== "string" || !options) return null;
    return {
        requestId,
        sessionId,
        toolCall: { ...toolCall, toolCallId: toolCall.toolCallId, title: typeof toolCall.title === "string" ? toolCall.title : "Agent tool" },
        options: options.flatMap((option) => {
            const row = recordOf(option);
            return row && typeof row.optionId === "string" && typeof row.name === "string" && typeof row.kind === "string"
                ? [{ optionId: row.optionId, name: row.name, kind: row.kind }]
                : [];
        }),
    };
}

function statusFromEvent(event: AcpEvent): "connecting" | "installing" | "starting" | "initializing" | "ready" | "stopped" | "error" {
    const value = event.payload.state;
    return value === "installing" || value === "starting" || value === "initializing" || value === "ready" || value === "stopped" || value === "error"
        ? value
        : "connecting";
}

function mergePaths(current: string[], incoming: readonly string[]): string[] {
    const merged = [...current];
    for (const path of incoming) {
        if (!path || path.includes("\0") || merged.includes(path)) continue;
        if (merged.length === MAX_ATTACHMENTS) break;
        merged.push(path);
    }
    return merged;
}

// Splits `mcp__server__tool` so the server name can be de-emphasized.
function toolLabel(title: string): { scope?: string; name: string } {
    const segments = title.split("__");
    return segments[0] === "mcp" && segments.length > 2 ? { scope: segments[1], name: segments.slice(2).join("__") } : { name: title };
}

function ToolPart({ tool }: { tool: AcpToolCall }) {
    const status = tool.status ?? "pending";
    const complete = status === "completed";
    const failed = status === "failed";
    const detail = tool.rawOutput ?? tool.rawInput ?? tool.content;
    const { scope, name } = toolLabel(tool.title);
    const head = (
        <>
            <span className="chat-tool-mark">
                {complete ? <IconCheck size={11} /> : failed ? <IconWarning size={11} /> : <IconCommand size={11} />}
            </span>
            {scope && <span className="chat-tool-scope">{scope}</span>}
            <span className="chat-tool-name">{name}</span>
            {!complete && <span className="chat-tool-status">{status.replace(/_/g, " ")}</span>}
        </>
    );
    if (detail === undefined) return <div className={`chat-tool status-${status} bare`}>{head}</div>;
    return (
        <details className={`chat-tool status-${status}`}>
            <summary>{head}</summary>
            <pre>{formatDetail(detail)}</pre>
        </details>
    );
}

function ContentPart({ part }: { part: Extract<ChatPart, { kind: "content" }> }) {
    const content = part.content;
    if (content.type === "resource_link") {
        return (
            <div className="chat-resource">
                <IconFile size={13} />
                <span>{content.title || content.name || content.uri || "Resource"}</span>
            </div>
        );
    }
    if (content.type === "image" && typeof content.data === "string" && typeof content.mimeType === "string") {
        return <img className="chat-image" alt="Agent attachment" src={`data:${content.mimeType};base64,${content.data}`} />;
    }
    return <pre className="chat-unknown-part">{formatDetail(content)}</pre>;
}

function MessagePart({ part }: { part: ChatPart }) {
    if (part.kind === "text") {
        return (
            <div className="chat-markdown">
                <Markdown
                    remarkPlugins={[remarkGfm]}
                    skipHtml
                    components={{
                        a: ({ href, children }) => (
                            <a
                                href={href}
                                onClick={(event) => {
                                    event.preventDefault();
                                    if (href) void invoke("open_url", { url: href, app: null, shortcut: null }).catch(swallow("open chat link"));
                                }}>
                                {children}
                            </a>
                        ),
                    }}>
                    {part.text}
                </Markdown>
            </div>
        );
    }
    if (part.kind === "thought") {
        return (
            <details className="chat-thought">
                <summary>Reasoning</summary>
                <div className="chat-markdown">
                    <Markdown
                        remarkPlugins={[remarkGfm]}
                        skipHtml
                        components={{
                            a: ({ href, children }) => (
                                <a
                                    href={href}
                                    onClick={(event) => {
                                        event.preventDefault();
                                        if (href) void invoke("open_url", { url: href, app: null, shortcut: null }).catch(swallow("open chat link"));
                                    }}>
                                    {children}
                                </a>
                            ),
                        }}>
                        {part.text}
                    </Markdown>
                </div>
            </details>
        );
    }
    if (part.kind === "tool") return <ToolPart tool={part.tool} />;
    return <ContentPart part={part} />;
}

type PartGroup = { id: string; tools: Extract<ChatPart, { kind: "tool" }>[] } | { id: string; part: ChatPart };

function groupParts(parts: ChatPart[]): PartGroup[] {
    const groups: PartGroup[] = [];
    for (const part of parts) {
        const last = groups.at(-1);
        if (part.kind !== "tool") groups.push({ id: part.id, part });
        else if (last && "tools" in last) last.tools.push(part);
        else groups.push({ id: part.id, tools: [part] });
    }
    return groups;
}

const ChatMessageRow = memo(function ChatMessageRow({ message }: { message: ChatMessage }) {
    return (
        <article className={`chat-message ${message.role}`}>
            <div className="chat-message-content">
                {message.attachments && message.attachments.length > 0 && (
                    <div className="chat-message-attachments">
                        {message.attachments.map((path) => (
                            <span key={path} title={path}>
                                <IconFile size={12} />
                                {basename(path)}
                            </span>
                        ))}
                    </div>
                )}
                {groupParts(message.parts).map((group) =>
                    "tools" in group ? (
                        <div className="chat-tools" key={group.id}>
                            {group.tools.map((part) => (
                                <ToolPart key={part.id} tool={part.tool} />
                            ))}
                        </div>
                    ) : (
                        <MessagePart key={group.id} part={group.part} />
                    ),
                )}
            </div>
        </article>
    );
});

function SlashCommands({
    commands,
    selected,
    onSelect,
}: {
    commands: AcpAvailableCommand[];
    selected: number;
    onSelect: (command: AcpAvailableCommand) => void;
}) {
    return (
        <div className="chat-slash-menu" role="listbox" aria-label="Session commands">
            <div className="chat-slash-heading">
                <span>Session commands</span>
                <span>ACP</span>
            </div>
            {commands.map((command, index) => (
                <button
                    key={command.name}
                    type="button"
                    role="option"
                    aria-selected={index === selected}
                    className={index === selected ? "selected" : ""}
                    onMouseDown={(event) => event.preventDefault()}
                    onClick={() => onSelect(command)}>
                    <code>/{command.name}</code>
                    <span>{command.description}</span>
                    {command.input?.hint && <em>{command.input.hint}</em>}
                </button>
            ))}
        </div>
    );
}

function PermissionRequest({ request, busy, onReply }: { request: AcpPermissionRequest; busy: boolean; onReply: (optionId?: string) => void }) {
    return (
        <section className="chat-permission" aria-label={`Permission required for ${request.toolCall.title}`}>
            <div className="chat-permission-copy">
                <IconShieldBolt size={16} />
                <div>
                    <strong>{request.toolCall.title}</strong>
                    <span>Agent needs permission before this tool can continue.</span>
                </div>
            </div>
            <div className="chat-permission-actions">
                {request.options.map((option) => (
                    <button
                        key={option.optionId}
                        type="button"
                        disabled={busy}
                        className={option.kind.startsWith("reject") ? "reject" : "allow"}
                        onClick={() => onReply(option.optionId)}>
                        {option.name}
                    </button>
                ))}
                {!request.options.some((option) => option.kind.startsWith("reject")) && (
                    <button type="button" disabled={busy} className="reject" onClick={() => onReply()}>
                        Cancel
                    </button>
                )}
            </div>
        </section>
    );
}

export function AgentChatPane({
    agent,
    profile,
    cwd,
    active,
    onBusyChange,
}: {
    agent: Agent;
    profile?: ProviderProfile;
    cwd: string;
    active: boolean;
    onBusyChange: (busy: boolean) => void;
}) {
    const [state, dispatch] = useReducer(chatReducer, initialChatState);
    const [draft, setDraft] = useState("");
    const [attachments, setAttachments] = useState<string[]>([]);
    const [slashSelection, setSlashSelection] = useState(0);
    const [slashDismissed, setSlashDismissed] = useState(false);
    const [composerError, setComposerError] = useState<string | null>(null);
    const [replyingPermission, setReplyingPermission] = useState<string | null>(null);
    const [atBottom, setAtBottom] = useState(true);
    const [restartKey, setRestartKey] = useState(0);
    const paneRef = useRef<HTMLDivElement>(null);
    const scrollRef = useRef<HTMLDivElement>(null);
    const editorRef = useRef<HTMLTextAreaElement>(null);
    const queuedUpdatesRef = useRef<Record<string, unknown>[]>([]);
    const updateFrameRef = useRef<number | null>(null);
    const agentRef = useRef(agent);
    agentRef.current = agent;
    const agentLockedRef = useRef(false);
    if (state.messages.length > 0) agentLockedRef.current = true;
    const sessionIdRef = useRef<string | null>(null);
    const lifecycleRef = useRef<Promise<unknown>>(Promise.resolve());
    const [changingConfig, setChangingConfig] = useState(false);
    const configPending = useRef(false);
    const [changingPermissions, setChangingPermissions] = useState(false);
    const [appliedPermissionMode, setAppliedPermissionMode] = useState<string | null>(null);
    const environmentKeys = JSON.stringify(profile?.environmentKeys ?? []);
    const permissionMode = agent.permissionMode ?? (agent.skipPermissions ? "bypass" : "workspace-write");

    const virtualizer = useVirtualizer({
        count: state.messages.length,
        getScrollElement: () => scrollRef.current,
        estimateSize: () => 76,
        overscan: 8,
        getItemKey: (index) => state.messages[index]?.id ?? index,
    });

    useEffect(() => {
        if (!active) return;
        const backendState =
            state.connection === "error" || state.connection === "stopped"
                ? "stopped"
                : state.permissions.length > 0
                  ? "blocked"
                  : state.running
                    ? "working"
                    : state.connection === "ready"
                      ? "idle"
                      : "unknown";
        cmd.noteAcpAgentState(agent.id, backendState);
    }, [active, agent.id, state.connection, state.running, state.permissions.length]);

    useEffect(() => onBusyChange(state.running), [onBusyChange, state.running]);

    useEffect(() => {
        const element = paneRef.current;
        if (!element) return;
        return registerPathDrop(element, (paths) => {
            setAttachments((current) => mergePaths(current, paths));
            setComposerError(null);
            window.requestAnimationFrame(() => editorRef.current?.focus());
        });
    }, []);

    useEffect(() => {
        if (!active) return;
        const controller = new AbortController();
        let mounted = true;
        dispatch({ type: "reset" });
        setAppliedPermissionMode(null);
        setChangingPermissions(false);
        sessionIdRef.current = null;

        const flushUpdates = () => {
            if (updateFrameRef.current !== null) {
                window.cancelAnimationFrame(updateFrameRef.current);
                updateFrameRef.current = null;
            }
            const updates = queuedUpdatesRef.current.splice(0);
            for (const update of updates) dispatch({ type: "session_update", update });
        };

        const queueUpdate = (update: Record<string, unknown>) => {
            queuedUpdatesRef.current.push(update);
            if (updateFrameRef.current === null) updateFrameRef.current = window.requestAnimationFrame(flushUpdates);
        };

        const handleEvent = (event: AcpEvent) => {
            if (!mounted || event.agentId !== agent.id) return;
            if (event.kind !== "session_update") flushUpdates();
            if (event.kind === "status") dispatch({ type: "status", state: statusFromEvent(event) });
            else if (event.kind === "ready") {
                dispatch({
                    type: "ready",
                    capabilities: recordOf(event.payload.capabilities) ?? {},
                    setup: recordOf(event.payload.setup) ?? {},
                });
            } else if (event.kind === "session_update") {
                const update = recordOf(event.payload.update);
                if (update) queueUpdate(update);
            } else if (event.kind === "turn_started") {
                if (sessionIdRef.current && agentRef.current.resumeId !== sessionIdRef.current) {
                    cmd.attachAgentSession(agent.id, sessionIdRef.current);
                }
                dispatch({ type: "turn_started" });
            } else if (event.kind === "turn_completed") {
                dispatch({
                    type: "turn_completed",
                    stopReason: typeof event.payload.stopReason === "string" ? event.payload.stopReason : undefined,
                });
            } else if (event.kind === "permission_request") {
                const request = permissionRequest(event.payload);
                if (request) dispatch({ type: "permission_requested", request });
            } else if (event.kind === "error") dispatch({ type: "error", message: eventMessage(event) });
        };

        const lifecycle = lifecycleRef.current
            .catch(() => {})
            .then(async () => {
                if (!mounted) return;
                await acpApi.subscribe(handleEvent, controller.signal);
                if (!mounted) return;
                const current = agentRef.current;
                const initialMode = current.permissionMode ?? (current.skipPermissions ? "bypass" : "workspace-write");
                const response = await acpApi.start({
                    agentId: current.id,
                    provider: current.type,
                    cwd,
                    resumeId: current.resumeId,
                    permissionMode: initialMode,
                    configPath: profile?.configPath,
                    executablePath: profile?.executablePath || current.executablePath,
                    model: current.model,
                    effort: current.effort,
                    environmentKeys: JSON.parse(environmentKeys) as string[],
                });
                if (!mounted) return;
                sessionIdRef.current = response.sessionId;
                dispatch({ type: "ready", capabilities: response.capabilities, setup: response.setup });
                setAppliedPermissionMode(initialMode);
            })
            .catch((error: unknown) => {
                if (!controller.signal.aborted && mounted) {
                    dispatch({ type: "error", message: error instanceof Error ? error.message : String(error) });
                }
            });

        lifecycleRef.current = lifecycle;
        return () => {
            mounted = false;
            sessionIdRef.current = null;
            if (updateFrameRef.current !== null) window.cancelAnimationFrame(updateFrameRef.current);
            updateFrameRef.current = null;
            queuedUpdatesRef.current = [];
            controller.abort();
            lifecycleRef.current = lifecycle.finally(() => acpApi.stop(agent.id).catch(() => {}));
        };
    }, [
        active,
        agent.id,
        agent.type,
        agent.profileId,
        agent.executablePath,
        cwd,
        profile?.configPath,
        profile?.executablePath,
        environmentKeys,
        restartKey,
    ]);

    useEffect(() => {
        if (state.connection !== "ready" || changingPermissions || appliedPermissionMode === null || permissionMode === appliedPermissionMode) return;
        const sessionId = sessionIdRef.current;
        setChangingPermissions(true);
        void acpApi
            .setPermissionMode(agent.id, permissionMode)
            .then(() => {
                if (sessionIdRef.current === sessionId) setAppliedPermissionMode(permissionMode);
            })
            .catch((error: unknown) => {
                if (sessionIdRef.current !== sessionId) return;
                const currentMode = agentRef.current.permissionMode ?? (agentRef.current.skipPermissions ? "bypass" : "workspace-write");
                if (currentMode === permissionMode)
                    cmd.setAgentPermissionMode(agent.id, appliedPermissionMode as NonNullable<Agent["permissionMode"]>);
                setComposerError(error instanceof Error ? error.message : String(error));
            })
            .finally(() => {
                if (sessionIdRef.current === sessionId) setChangingPermissions(false);
            });
    }, [agent.id, state.connection, permissionMode, appliedPermissionMode, changingPermissions]);

    useEffect(() => {
        if (state.title && state.title !== agent.title) cmd.setAgentTitle(agent.id, state.title);
    }, [agent.id, agent.title, state.title]);

    useLayoutEffect(() => {
        if (!atBottom || state.messages.length === 0) return;
        const frame = window.requestAnimationFrame(() => virtualizer.scrollToIndex(state.messages.length - 1, { align: "end" }));
        return () => window.cancelAnimationFrame(frame);
    }, [atBottom, state.messages.length, state.revision, virtualizer]);

    const slashCommands = useMemo(() => {
        if (slashDismissed || !draft.startsWith("/") || /\s/.test(draft.slice(1))) return [];
        const needle = draft.slice(1).toLowerCase();
        return state.commands.filter((command) => command.name.toLowerCase().includes(needle)).slice(0, 8);
    }, [draft, slashDismissed, state.commands]);

    useEffect(() => setSlashSelection(0), [draft]);

    const selectCommand = (command: AcpAvailableCommand) => {
        setDraft(`/${command.name}${command.input?.hint ? " " : ""}`);
        setSlashDismissed(true);
        window.requestAnimationFrame(() => editorRef.current?.focus());
    };

    const send = async () => {
        const text = draft.trim();
        if (
            (!text && attachments.length === 0) ||
            state.running ||
            configPending.current ||
            state.connection !== "ready" ||
            changingPermissions ||
            permissionMode !== appliedPermissionMode
        )
            return;
        const commandName = text.match(/^\/([^\s]+)/)?.[1];
        if (commandName && state.commands.length > 0 && !state.commands.some((command) => command.name === commandName)) {
            setComposerError(`/${commandName} is not available in this session`);
            return;
        }
        const paths = [...attachments];
        setDraft("");
        setAttachments([]);
        setComposerError(null);
        setSlashDismissed(false);
        dispatch({ type: "local_prompt", text, paths });
        try {
            await acpApi.prompt(agent.id, text, paths);
        } catch (error) {
            dispatch({ type: "error", message: error instanceof Error ? error.message : String(error) });
        }
    };

    const chooseFiles = async () => {
        try {
            const selected = await open({ multiple: true, directory: false });
            if (!selected) return;
            setAttachments((current) => mergePaths(current, Array.isArray(selected) ? selected : [selected]));
            window.requestAnimationFrame(() => editorRef.current?.focus());
        } catch (error) {
            setComposerError(error instanceof Error ? error.message : String(error));
        }
    };

    const replyPermission = async (requestId: string, optionId?: string) => {
        setReplyingPermission(requestId);
        try {
            await acpApi.permissionReply(agent.id, requestId, optionId);
            dispatch({ type: "permission_cleared", requestId });
        } catch (error) {
            setComposerError(error instanceof Error ? error.message : String(error));
        } finally {
            setReplyingPermission(null);
        }
    };

    const permission = permissionCopyForType(agent.type, permissionMode);
    const changeConfig = async (config: SessionConfig, value: string) => {
        if (configPending.current || state.running || changingPermissions) return;
        configPending.current = true;
        setChangingConfig(true);
        setComposerError(null);
        const sessionId = sessionIdRef.current;
        try {
            const response = await acpApi.setConfig(agent.id, config.id, value);
            if (sessionIdRef.current !== sessionId) return;
            dispatch({ type: "config", options: response.configOptions });
            const options = sessionConfigs({ configOptions: response.configOptions });
            const model = options.find((option) => option.id === "model")?.currentValue ?? agent.model;
            const effort =
                options.find((option) => option.id === (agent.type === "claude" ? "effort" : "reasoning_effort"))?.currentValue ?? agent.effort;
            const knownEffort = ["off", "none", "minimal", "low", "medium", "high", "xhigh", "max", "ultra"].includes(effort ?? "")
                ? (effort as Agent["effort"])
                : undefined;
            cmd.setAgentModelPreferences(agent.id, model, knownEffort);
        } catch (error) {
            if (sessionIdRef.current === sessionId) setComposerError(error instanceof Error ? error.message : String(error));
        } finally {
            configPending.current = false;
            setChangingConfig(false);
        }
    };
    const composerPlaceholder =
        state.connection === "ready"
            ? "Ask about this project, or type / for commands"
            : state.connection === "error" || state.connection === "stopped"
              ? "Reconnect to continue this conversation"
              : state.connection === "installing"
                ? "Installing structured-session adapter…"
                : state.connection === "starting"
                  ? "Starting agent adapter…"
                  : "Connecting to agent session…";

    return (
        <div className="agent-chat-pane" ref={paneRef}>
            <div
                className="chat-scroll"
                ref={scrollRef}
                onScroll={(event) => {
                    const element = event.currentTarget;
                    const next = element.scrollHeight - element.scrollTop - element.clientHeight < 72;
                    if (next !== atBottom) setAtBottom(next);
                }}>
                {state.messages.length === 0 && (
                    <div className={`chat-connection-state ${state.connection}`} role="status">
                        <span>
                            {state.connection === "ready"
                                ? "Start a session with this project."
                                : state.connection === "installing"
                                  ? "Installing structured-session adapter…"
                                  : state.connection === "starting"
                                    ? "Starting agent adapter…"
                                    : state.connection === "initializing"
                                      ? "Connecting to agent session…"
                                      : state.connection === "error"
                                        ? "Structured session unavailable."
                                        : state.connection === "stopped"
                                          ? "Agent session stopped."
                                          : "Preparing agent session…"}
                        </span>
                        {(state.connection === "error" || state.connection === "stopped") && (
                            <button type="button" onClick={() => setRestartKey((value) => value + 1)}>
                                Retry
                            </button>
                        )}
                    </div>
                )}
                {state.connection === "error" && agent.resumeId && (
                    <button
                        type="button"
                        onClick={() =>
                            cmd.addAgent(agent.type, undefined, undefined, {
                                permissionMode: agent.permissionMode,
                                profileId: agent.profileId,
                                detectedExecutablePath: profile?.executablePath || agent.executablePath,
                                cwd,
                            })
                        }>
                        Start new chat
                    </button>
                )}
                <div className="chat-virtual-space" style={{ height: `${virtualizer.getTotalSize()}px` }}>
                    {virtualizer.getVirtualItems().map((item) => {
                        const message = state.messages[item.index];
                        return (
                            <div
                                key={message.id}
                                data-index={item.index}
                                ref={virtualizer.measureElement}
                                className="chat-virtual-row"
                                style={{ transform: `translateY(${item.start}px)` }}>
                                <ChatMessageRow message={message} />
                            </div>
                        );
                    })}
                </div>
                {state.plan !== null && (
                    <details className="chat-plan">
                        <summary>Plan</summary>
                        <pre>{formatDetail(state.plan)}</pre>
                    </details>
                )}
                {state.permissions.map((request) => (
                    <PermissionRequest
                        key={request.requestId}
                        request={request}
                        busy={replyingPermission === request.requestId}
                        onReply={(optionId) => void replyPermission(request.requestId, optionId)}
                    />
                ))}
                {state.error && (
                    <div className="chat-error" role="alert">
                        <IconWarning size={14} />
                        <span>{state.error}</span>
                    </div>
                )}
                {state.messages.length > 0 && (state.connection === "error" || state.connection === "stopped") && (
                    <button type="button" onClick={() => setRestartKey((value) => value + 1)}>
                        Reconnect
                    </button>
                )}
            </div>

            {!atBottom && state.messages.length > 0 && (
                <button
                    type="button"
                    className="chat-jump-bottom"
                    aria-label="Jump to latest message"
                    onClick={() => {
                        setAtBottom(true);
                        virtualizer.scrollToIndex(state.messages.length - 1, { align: "end" });
                    }}>
                    <IconArrowDown size={14} />
                </button>
            )}

            <div className="chat-composer-wrap">
                <div className="chat-composer">
                    {slashCommands.length > 0 && <SlashCommands commands={slashCommands} selected={slashSelection} onSelect={selectCommand} />}
                    {attachments.length > 0 && (
                        <div className="chat-attachments">
                            {attachments.map((path) => (
                                <span key={path} title={path}>
                                    <IconFile size={14} />
                                    <span>{basename(path)}</span>
                                    <button
                                        type="button"
                                        aria-label={`Remove ${basename(path)}`}
                                        onClick={() => setAttachments((current) => current.filter((candidate) => candidate !== path))}>
                                        <IconClose size={11} />
                                    </button>
                                </span>
                            ))}
                        </div>
                    )}
                    <textarea
                        ref={editorRef}
                        value={draft}
                        disabled={state.connection !== "ready"}
                        aria-label="Message agent"
                        placeholder={composerPlaceholder}
                        rows={3}
                        onChange={(event) => {
                            setDraft(event.target.value);
                            setComposerError(null);
                            setSlashDismissed(false);
                        }}
                        onKeyDown={(event) => {
                            if (slashCommands.length > 0) {
                                if (event.key === "ArrowDown") {
                                    event.preventDefault();
                                    setSlashSelection((current) => (current + 1) % slashCommands.length);
                                    return;
                                }
                                if (event.key === "ArrowUp") {
                                    event.preventDefault();
                                    setSlashSelection((current) => (current - 1 + slashCommands.length) % slashCommands.length);
                                    return;
                                }
                                if (event.key === "Tab" || event.key === "Enter") {
                                    event.preventDefault();
                                    selectCommand(slashCommands[slashSelection]);
                                    return;
                                }
                                if (event.key === "Escape") {
                                    event.preventDefault();
                                    setSlashDismissed(true);
                                    return;
                                }
                            }
                            if (event.key === "Enter" && !event.shiftKey && !event.nativeEvent.isComposing) {
                                event.preventDefault();
                                void send();
                            }
                        }}
                    />
                    {composerError && <div className="chat-composer-error">{composerError}</div>}
                    <div className="chat-composer-bar">
                        <button type="button" className="chat-composer-icon" aria-label="Add files" onClick={() => void chooseFiles()}>
                            <IconPlus size={17} />
                        </button>
                        <button
                            type="button"
                            className={`chat-permission-mode tone-${permission.tone}`}
                            disabled={
                                state.connection !== "ready" ||
                                changingConfig ||
                                state.running ||
                                state.permissions.length > 0 ||
                                changingPermissions ||
                                permissionMode !== appliedPermissionMode
                            }
                            title={permission.detail}
                            onClick={() => cmd.toggleAgentSkipPermissions(agent.id)}>
                            <IconShieldBolt size={14} />
                            <span>{permission.label}</span>
                        </button>
                        <ComposerPickers
                            agent={agent}
                            profile={profile}
                            setup={state.setup}
                            agentLocked={agentLockedRef.current}
                            disabled={
                                state.connection !== "ready" || state.running || changingPermissions || changingConfig || state.permissions.length > 0
                            }
                            onAgent={(type, profileId) => {
                                if (agentLockedRef.current || state.running || state.permissions.length > 0) return;
                                cmd.configureEmptyAgent(agent.id, type, profileId);
                            }}
                            onConfig={(config, value) => void changeConfig(config, value)}
                        />
                        <span className="chat-composer-spacer" />
                        {state.running ? (
                            <button
                                type="button"
                                className="chat-send stop"
                                aria-label="Stop agent"
                                onClick={() =>
                                    void acpApi
                                        .cancel(agent.id)
                                        .catch((error: unknown) => setComposerError(error instanceof Error ? error.message : String(error)))
                                }>
                                <span />
                            </button>
                        ) : (
                            <button
                                type="button"
                                className="chat-send"
                                aria-label="Send message"
                                disabled={
                                    state.connection !== "ready" ||
                                    changingConfig ||
                                    changingPermissions ||
                                    permissionMode !== appliedPermissionMode ||
                                    (!draft.trim() && attachments.length === 0)
                                }
                                onClick={() => void send()}>
                                <IconArrowUp size={18} />
                            </button>
                        )}
                    </div>
                </div>
            </div>
            <div className="chat-drop-target" aria-hidden="true">
                <IconFile size={22} />
                <span>Drop files or folders into this session</span>
            </div>
        </div>
    );
}
