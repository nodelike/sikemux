import type { ShareClientMessage, ShareServerMessage } from "../../server/protocol/generated/types";
import { ShareCloseCode } from "../../server/protocol/generated/types";
import { OVER_LIMIT_WAIT_MS, reconnectDelay, SignInRenewal } from "../account/renewal";
import { agentLabel, setAgentCatalog } from "../agents/catalog";
import { imageType } from "../lib/imagePaths";
import { applyChanges, emptyChat, fromView, withPage } from "../chat/rows";
import type { ChatChange, ChatRows, ToolDetail } from "../chat/types";
import type { ShareAsk, SharedChat, ShareFrame, ShareReply } from "../wire.generated";
import type { ShareCipher } from "./cipher";
import type { ShareLink } from "./link";

export type { SharedChat };

/** What a viewer asks the host, before it is numbered. */
type Question = { [Type in ShareAsk["type"]]: Omit<Extract<ShareAsk, { type: Type }>, "id"> }[ShareAsk["type"]];

type ShareStatus =
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

/** What a shared chat is called: the app's name for it, else its first message, else its agent's. */
export function sharedChatTitle(snapshot: ShareSnapshot): string {
    if (snapshot.chat?.title) return snapshot.chat.title;
    if (snapshot.agent.title) return snapshot.agent.title;
    const first = snapshot.agent.messages.find((message) => message.role === "user");
    const part = first?.parts.find((candidate) => candidate.kind === "text");
    const text = first?.letters?.[0]?.text ?? (part?.kind === "text" ? part.text : "");
    const words = text.split(/\s+/).filter(Boolean).join(" ");
    if (words) return words.length > 80 ? `${words.slice(0, 79)}…` : words;
    return `${agentLabel(snapshot.chat?.provider ?? "agent")} chat`;
}

interface ShareSessionOptions {
    link: ShareLink;
    /** The share socket, such as `wss://api.sikemux.com/v1/share`. */
    url: string;
    cipher: ShareCipher;
    /** A sign-in token, or null when signed out. `fresh` asks past any cache for a newer one. */
    token(fresh: boolean): Promise<string | null>;
}

/** Whose catalog a share's agents are named from: the host that shared it, not one this client is on. */
const SHARING_CORE = "share";
/** A question the host has not answered in this long gets no answer. */
const ANSWER_WAIT_MS = 30_000;

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

    private socket: WebSocket | null = null;
    private stopped = true;
    private failures = 0;
    private retryTimer: ReturnType<typeof setTimeout> | null = null;
    private readonly renewal: SignInRenewal;
    /** The last frame taken, so a reconnect asks only for what came after it. */
    private seq: number | undefined;
    /** Frames are unsealed one after another, so they apply in the order they came. */
    private unsealing: Promise<void> = Promise.resolve();
    private pending: ChatChange[] = [];
    private drawing: number | undefined;
    private asked = 0;
    private answers = new Map<number, (answer: ShareReply | null) => void>();
    private images = new Map<string, Promise<string | null>>();

    private readonly options: ShareSessionOptions;

    constructor(options: ShareSessionOptions) {
        this.options = options;
        this.renewal = new SignInRenewal(options.token);
        this.snap = this.build();
    }

    subscribe = (listener: () => void) => {
        this.listeners.add(listener);
        return () => {
            this.listeners.delete(listener);
        };
    };

    snapshot = () => this.snap;

    start = () => {
        if (!this.stopped) return;
        this.stopped = false;
        this.open();
    };

    stop = () => {
        this.stopped = true;
        if (this.retryTimer) clearTimeout(this.retryTimer);
        this.renewal.stop();
        this.retryTimer = null;
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
        const type = imageType(path);
        const read = type
            ? this.ask({ type: "file", path }).then((answer) => (answer?.file ? `data:${type};base64,${answer.file.data}` : null))
            : Promise.resolve(null);
        this.images.set(path, read);
        void read.then((source) => {
            if (source === null) this.images.delete(path);
        });
        return read;
    };

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
        const socket = new WebSocket(this.options.url);
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
    }

    private send(socket: WebSocket, message: ShareClientMessage) {
        if (this.socket === socket) socket.send(JSON.stringify(message));
    }

    private closed(code: number) {
        this.renewal.stop();
        for (const settle of this.answers.values()) settle(null);
        this.answers.clear();
        if (this.stopped) return;
        if (code === ShareCloseCode.notFound) return this.finished("notFound");
        if (code === ShareCloseCode.ended) return this.finished("ended");
        this.hostOnline = false;
        this.emit();
        this.failures += 1;
        const delay = reconnectDelay(this.failures - 1, Math.random());
        const wait = code === ShareCloseCode.overLimit ? Math.max(OVER_LIMIT_WAIT_MS, delay) : delay;
        this.retryTimer = setTimeout(() => {
            this.retryTimer = null;
            if (!this.stopped) this.open();
        }, wait);
    }

    private async handle(socket: WebSocket, message: ShareServerMessage) {
        switch (message.type) {
            case "challenge": {
                const [token, proof] = await Promise.all([this.options.token(false), this.options.cipher.viewerProof()]);
                if (this.socket !== socket) return;
                if (!token) return this.finished("signedOut");
                this.send(socket, {
                    type: "hello",
                    role: "viewer",
                    token,
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
                const answer = plain === null ? null : (JSON.parse(plain) as ShareReply);
                if (answer) this.answers.get(answer.id)?.(answer);
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

    private refreshAuth(socket: WebSocket, expiry: number) {
        this.renewal.before(expiry, (token) => {
            if (this.socket !== socket) return false;
            this.send(socket, { type: "auth", token });
            return true;
        });
    }

    /** One unsealed frame: a snapshot starts the chat over, the rest change it. */
    private take(plain: string) {
        const frame = JSON.parse(plain) as ShareFrame;
        if (frame.type === "snapshot") return this.replace(frame);
        if (frame.type !== "event") return;
        const { event } = frame;
        if (event.kind !== "chat" || event.agentId !== this.chat?.agentId || event.seq <= this.chatSeq) return;
        this.chatSeq = event.seq;
        this.pending.push(...event.changes);
        this.drawing ??= requestAnimationFrame(this.flush);
    }

    private flush = () => {
        this.drawing = undefined;
        if (!this.pending.length) return;
        const batch = this.pending;
        this.pending = [];
        this.agent = applyChanges(this.agent, batch);
        this.emit();
    };

    private replace({ chat, attachment }: Extract<ShareFrame, { type: "snapshot" }>) {
        if (this.drawing !== undefined) cancelAnimationFrame(this.drawing);
        this.drawing = undefined;
        this.pending = [];
        this.chat = chat;
        setAgentCatalog(SHARING_CORE, chat.agents ?? []);
        if (attachment.status !== "live") {
            this.status = "missing";
            this.feed = null;
            this.agent = this.agent.messages.length ? { ...this.agent, running: false } : emptyChat;
            this.emit();
            return;
        }
        this.agent = fromView(attachment.view);
        this.feed = attachment.mark.feed;
        this.chatSeq = attachment.mark.seq;
        this.earlier = "idle";
        this.status = "live";
        this.emit();
    }

    /** Asks the host something, sealed. Answers null when the host is away or will not say. */
    private async ask(question: Question): Promise<ShareReply | null> {
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
