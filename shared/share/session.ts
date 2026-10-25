import type { ShareClientMessage, ShareServerMessage } from "../../server/protocol/generated/types";
import { ShareCloseCode } from "../../server/protocol/generated/types";
import { recordOf } from "../lib/guards";
import { applyChanges, emptyChat, fromView, withPage } from "../chat/rows";
import type { ChatChange, ChatPage, ChatRows, ChatView, ToolDetail } from "../chat/types";
import type { ShareCipher } from "./cipher";
import type { ShareLink } from "./link";

/** What a share says of its chat, from its latest snapshot. */
export interface SharedChat {
    readonly agentId: string;
    readonly provider: string | null;
    readonly title: string | null;
    readonly model: string | null;
    readonly cwd: string | null;
    readonly home: string | null;
}

export type ShareStatus =
    /** Reaching the server, or waiting for the chat's first snapshot. */
    | "connecting"
    | "live"
    /** The chat's agent is not running on the sharer's computer now. */
    | "missing"
    | "ended"
    | "notFound"
    | "signedOut"
    /** The link's key does not read what the host sent. */
    | "unreadable";

export interface ShareSnapshot {
    readonly status: ShareStatus;
    readonly endedBecause: "stopped" | "expired" | null;
    readonly hostOnline: boolean;
    /** Milliseconds since the Unix epoch. */
    readonly expiresAt: number | null;
    readonly chat: SharedChat | null;
    readonly agent: ChatRows;
    readonly hasEarlier: boolean;
    readonly earlier: "idle" | "loading" | "failed";
}

/** What a shared chat is called: the app's name for it, else its first message. */
export function sharedChatTitle(snapshot: ShareSnapshot): string | null {
    if (snapshot.chat?.title) return snapshot.chat.title;
    if (snapshot.agent.title) return snapshot.agent.title;
    const first = snapshot.agent.messages.find((message) => message.role === "user");
    const part = first?.parts.find((candidate) => candidate.kind === "text");
    const text = first?.letters?.[0]?.text ?? (part?.kind === "text" ? part.text : "");
    const words = text.split(/\s+/).filter(Boolean).join(" ");
    return words ? (words.length > 80 ? `${words.slice(0, 79)}…` : words) : null;
}

/** The one thing a socket must do for a share session; the browser's WebSocket does it. */
export interface ShareSocket {
    onopen: (() => void) | null;
    onmessage: ((event: { data: unknown }) => void) | null;
    onclose: ((event: { code: number }) => void) | null;
    onerror: (() => void) | null;
    send(text: string): void;
    close(): void;
}

export interface ShareSessionOptions {
    link: ShareLink;
    /** The share socket, such as `wss://api.sikemux.com/v1/share`. */
    url: string;
    cipher: ShareCipher;
    /** A sign-in token, or null when signed out. `fresh` asks past any cache for a newer one. */
    token?(fresh: boolean): Promise<string | null>;
    /** A Mac watches with its host's key instead of a token: this signs the server's challenge with it. */
    signHello?(nonce: string): Promise<{ key: string; signature: string }>;
    connect?: (url: string) => ShareSocket;
    /** Runs once before the next frame is drawn; returns how to call that off. */
    frame?: (run: () => void) => () => void;
    now?: () => number;
}

type Answer = { page?: ChatPage; toolDetail?: ToolDetail; file?: { data: string }; error?: string };

const BACKOFF_BASE_MS = 1_000;
const BACKOFF_CAP_MS = 60_000;
/** A question the host has not answered in this long gets no answer. */
const ANSWER_WAIT_MS = 30_000;
/** A fresh token goes out this long before the last one expires. */
const AUTH_AHEAD_MS = 15_000;
const IMAGE_TYPES: Record<string, string> = {
    png: "image/png",
    jpg: "image/jpeg",
    jpeg: "image/jpeg",
    gif: "image/gif",
    webp: "image/webp",
    heic: "image/heic",
    svg: "image/svg+xml",
};

function animationFrame(run: () => void): () => void {
    if (typeof requestAnimationFrame !== "function") {
        const timer = setTimeout(run, 16);
        return () => clearTimeout(timer);
    }
    const frame = requestAnimationFrame(run);
    return () => cancelAnimationFrame(frame);
}

/** When a sign-in token expires, in milliseconds since the epoch, or 0 when it cannot be read. */
function expiresAt(token: string): number {
    try {
        const payload = (token.split(".")[1] ?? "").replace(/-/g, "+").replace(/_/g, "/");
        const exp = (JSON.parse(atob(payload)) as { exp?: unknown }).exp;
        return typeof exp === "number" ? exp * 1000 : 0;
    } catch {
        return 0;
    }
}

function text(value: unknown): string | null {
    return typeof value === "string" ? value : null;
}

/**
 * One shared chat, watched read-only through the accounts server. It holds the rows the host's
 * snapshot sends and makes the changes that follow, and asks the host, sealed, for the turns
 * before them, for what a tool call changed and printed, and for the pictures pasted into them.
 */
export class ShareSession {
    private listeners = new Set<() => void>();
    private status: ShareStatus = "connecting";
    private endedBecause: ShareSnapshot["endedBecause"] = null;
    private hostOnline = false;
    private expiresAt: number | null = null;
    private chat: SharedChat | null = null;
    private agent: ChatRows = emptyChat;
    /** The run of the chat's agent the rows come from, which earlier pages are asked of. */
    private feed: string | null = null;
    /** The last of the chat's changes made, so one the snapshot already holds is not made again. */
    private chatSeq = 0;
    private earlier: ShareSnapshot["earlier"] = "idle";
    private snap: ShareSnapshot;

    private socket: ShareSocket | null = null;
    private stopped = true;
    private failures = 0;
    private retryTimer: ReturnType<typeof setTimeout> | null = null;
    private authTimer: ReturnType<typeof setTimeout> | null = null;
    /** The last frame taken, so a reconnect asks only for what came after it. */
    private seq: number | undefined;
    /** Frames are unsealed one after another, so they apply in the order they came. */
    private unsealing: Promise<void> = Promise.resolve();
    private pending: ChatChange[] = [];
    private cancelFrame: (() => void) | undefined;
    private asked = 0;
    private answers = new Map<number, (answer: Answer | null) => void>();
    private images = new Map<string, Promise<string | null>>();

    constructor(private readonly options: ShareSessionOptions) {
        this.snap = this.build();
    }

    subscribe = (listener: () => void) => {
        this.listeners.add(listener);
        return () => {
            this.listeners.delete(listener);
        };
    };

    snapshot = () => this.snap;

    /** Watches until `stop`. */
    start = () => {
        if (!this.stopped) return;
        this.stopped = false;
        this.open();
    };

    stop = () => {
        this.stopped = true;
        if (this.retryTimer) clearTimeout(this.retryTimer);
        if (this.authTimer) clearTimeout(this.authTimer);
        this.retryTimer = null;
        this.authTimer = null;
        const socket = this.socket;
        this.socket = null;
        socket?.close();
        for (const settle of this.answers.values()) settle(null);
        this.answers.clear();
    };

    /** Fetches the turns before the first message shown, unless that is already under way. */
    loadEarlier = () => {
        const { feed } = this;
        const before = this.agent.olderBefore;
        if (feed === null || before === null || this.earlier === "loading") return;
        this.earlier = "loading";
        this.emit();
        void this.ask({ type: "page", feed, before }).then((answer) => {
            if (this.feed !== feed || this.agent.olderBefore !== before) return;
            const page = answer?.page;
            if (page) this.agent = withPage(this.agent, page);
            this.earlier = page ? "idle" : "failed";
            this.emit();
        });
    };

    /** What a tool call changed and printed, or null when the host will not or cannot say. */
    readToolDetail = (toolCallId: string): Promise<ToolDetail | null> =>
        this.ask({ type: "toolDetail", toolCallId }).then((answer) => answer?.toolDetail ?? null);

    /** A picture pasted into the chat, as a data URL, or null when the host will not or cannot send it. */
    readImage = (path: string): Promise<string | null> => {
        const held = this.images.get(path);
        if (held) return held;
        const type = IMAGE_TYPES[path.split(".").pop()?.toLowerCase() ?? ""];
        const read = type
            ? this.ask({ type: "file", path }).then((answer) => (answer?.file ? `data:${type};base64,${answer.file.data}` : null))
            : Promise.resolve(null);
        this.images.set(path, read);
        void read.then((source) => {
            if (source === null) this.images.delete(path);
        });
        return read;
    };

    private now() {
        return (this.options.now ?? Date.now)();
    }

    private emit() {
        this.snap = this.build();
        this.listeners.forEach((listener) => listener());
    }

    private build(): ShareSnapshot {
        return {
            status: this.status,
            endedBecause: this.endedBecause,
            hostOnline: this.hostOnline,
            expiresAt: this.expiresAt,
            chat: this.chat,
            agent: this.agent,
            hasEarlier: this.feed !== null && this.agent.olderBefore !== null,
            earlier: this.earlier,
        };
    }

    private finished(status: ShareStatus) {
        this.status = status;
        this.stop();
        this.emit();
    }

    private open() {
        const connect = this.options.connect ?? ((url: string) => new WebSocket(url) as unknown as ShareSocket);
        const socket = connect(this.options.url);
        this.socket = socket;
        socket.onmessage = (event) => {
            if (this.socket !== socket || typeof event.data !== "string") return;
            let message: ShareServerMessage;
            try {
                message = JSON.parse(event.data) as ShareServerMessage;
            } catch {
                return;
            }
            void this.handle(socket, message);
        };
        socket.onclose = (event) => {
            if (this.socket !== socket) return;
            this.socket = null;
            this.closed(event.code);
        };
        socket.onerror = () => undefined;
    }

    private send(socket: ShareSocket, message: ShareClientMessage) {
        if (this.socket === socket) socket.send(JSON.stringify(message));
    }

    private closed(code: number) {
        if (this.authTimer) clearTimeout(this.authTimer);
        this.authTimer = null;
        for (const settle of this.answers.values()) settle(null);
        this.answers.clear();
        if (this.stopped) return;
        if (code === ShareCloseCode.notFound) return this.finished("notFound");
        if (code === ShareCloseCode.ended) return this.finished("ended");
        this.hostOnline = false;
        this.emit();
        this.failures += 1;
        const ceiling = Math.min(BACKOFF_CAP_MS, BACKOFF_BASE_MS * 2 ** Math.min(this.failures - 1, 16));
        const wait = code === ShareCloseCode.overLimit ? Math.max(30_000, ceiling) : Math.random() * ceiling;
        this.retryTimer = setTimeout(() => {
            this.retryTimer = null;
            if (!this.stopped) this.open();
        }, wait);
    }

    private async handle(socket: ShareSocket, message: ShareServerMessage) {
        switch (message.type) {
            case "challenge": {
                const [signIn, proof] = await Promise.all([this.signIn(message.nonce), this.options.cipher.viewerProof()]);
                if (this.socket !== socket) return;
                if (!signIn) return this.finished("signedOut");
                this.send(socket, {
                    type: "hello",
                    role: "viewer",
                    ...signIn,
                    share: this.options.link.id,
                    proof,
                    ...(this.seq === undefined ? {} : { since: this.seq }),
                });
                return;
            }
            case "ready":
                this.failures = 0;
                this.hostOnline = message.hostOnline ?? false;
                this.expiresAt = message.expiresAt ? Date.parse(message.expiresAt) : null;
                if (message.authExpiresAt) this.refreshAuth(socket, Date.parse(message.authExpiresAt));
                this.emit();
                return;
            case "frames":
                for (const frame of message.frames) {
                    this.unsealing = this.unsealing.then(async () => {
                        const plain = await this.options.cipher.open(frame.sealed);
                        if (this.socket !== socket) return;
                        if (plain === null) return this.finished("unreadable");
                        this.seq = frame.seq;
                        this.take(plain);
                    });
                }
                return;
            case "answer": {
                const plain = await this.options.cipher.open(message.sealed);
                const answer = plain === null ? null : (recordOf(JSON.parse(plain)) as (Answer & { id?: number }) | null);
                if (typeof answer?.id !== "number") return;
                this.answers.get(answer.id)?.(answer);
                return;
            }
            case "host":
                this.hostOnline = message.online;
                this.emit();
                return;
            case "ended":
                this.endedBecause = message.reason === "stopped" || message.reason === "expired" ? message.reason : null;
                return this.finished("ended");
            case "ping":
                this.send(socket, { type: "pong" });
                return;
            default:
                return;
        }
    }

    private async signIn(nonce: string): Promise<{ token: string } | { key: string; signature: string } | null> {
        const { signHello, token } = this.options;
        if (signHello) return signHello(nonce).catch(() => null);
        const current = await token?.(false);
        return current ? { token: current } : null;
    }

    private refreshAuth(socket: ShareSocket, expiry: number) {
        if (this.authTimer) clearTimeout(this.authTimer);
        this.authTimer = setTimeout(
            () => {
                this.authTimer = null;
                void this.refresh(socket, expiry);
            },
            Math.max(1_000, expiry - this.now() - AUTH_AHEAD_MS),
        );
    }

    /** The sign-in may still hand out the token the server already has; it is asked past its cache then. */
    private async refresh(socket: ShareSocket, expiry: number) {
        let token = (await this.options.token?.(false)) ?? null;
        if (token && expiresAt(token) <= expiry) token = (await this.options.token?.(true)) ?? null;
        if (!token || this.socket !== socket) return;
        this.send(socket, { type: "auth", token });
        this.refreshAuth(socket, expiresAt(token));
    }

    /** One unsealed frame: a snapshot starts the chat over, the rest change it. */
    private take(plain: string) {
        const frame = recordOf(JSON.parse(plain));
        if (frame?.type === "snapshot") return this.replace(frame);
        // The core's own message, as its clients hear it: the chat's changes are inside.
        const event = frame?.type === "event" ? recordOf(frame.event) : null;
        if (event?.kind !== "chat" || event.agentId !== this.chat?.agentId) return;
        const seq = typeof event.seq === "number" ? event.seq : 0;
        if (seq <= this.chatSeq || !Array.isArray(event.changes)) return;
        this.chatSeq = seq;
        this.pending.push(...(event.changes as ChatChange[]));
        this.cancelFrame ??= (this.options.frame ?? animationFrame)(this.flush);
    }

    private flush = () => {
        this.cancelFrame = undefined;
        if (!this.pending.length) return;
        const batch = this.pending;
        this.pending = [];
        this.agent = applyChanges(this.agent, batch);
        this.emit();
    };

    private replace(frame: Record<string, unknown>) {
        this.cancelFrame?.();
        this.cancelFrame = undefined;
        this.pending = [];
        const chat = recordOf(frame.chat);
        this.chat = {
            agentId: text(chat?.agentId) ?? "",
            provider: text(chat?.provider),
            title: text(chat?.title),
            model: text(chat?.model),
            cwd: text(chat?.cwd),
            home: text(chat?.home),
        };
        const attachment = recordOf(frame.attachment);
        const view = recordOf(attachment?.view);
        const mark = recordOf(attachment?.mark);
        if (attachment?.status !== "live" || !view) {
            this.status = "missing";
            this.feed = null;
            this.agent = this.agent.messages.length ? { ...this.agent, running: false } : emptyChat;
            this.emit();
            return;
        }
        this.agent = fromView(view as unknown as ChatView);
        this.feed = text(mark?.feed);
        this.chatSeq = typeof mark?.seq === "number" ? mark.seq : 0;
        this.earlier = "idle";
        this.status = "live";
        this.emit();
    }

    /** Asks the host something, sealed. Answers null when the host is away or will not say. */
    private async ask(question: Record<string, unknown>): Promise<Answer | null> {
        const socket = this.socket;
        if (!socket || !this.hostOnline) return null;
        const id = (this.asked += 1);
        const sealed = await this.options.cipher.seal(JSON.stringify({ ...question, id }));
        return new Promise((resolve) => {
            const timer = setTimeout(() => this.answers.get(id)?.(null), ANSWER_WAIT_MS);
            this.answers.set(id, (answer) => {
                clearTimeout(timer);
                this.answers.delete(id);
                resolve(answer?.error ? null : answer);
            });
            this.send(socket, { type: "ask", sealed });
        });
    }
}
