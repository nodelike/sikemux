import { useAuth } from "@clerk/react";
import {
  LiveCloseCode,
  type AccountEvent,
  type LiveDeviceMessage,
  type LiveServerMessage,
} from "@sikemux/protocol";
import { useEffect, useEffectEvent } from "react";

import { config } from "./config.ts";

export type LiveHandlers = {
  /** Something changed on the account; events arrive oldest first. */
  onEvents: (events: AccountEvent[]) => void;
  /** Connected again after a gap, so changes may have been missed. */
  onResync: () => void;
  onAccountDeleted: () => void;
};

type TokenSource = (fresh: boolean) => Promise<string | null>;

const OVER_LIMIT_WAIT_MS = 30_000;
const REFRESH_AHEAD_MS = 15_000;
const MAX_BACKOFF_MS = 60_000;

/** Seconds since 1970 when a Clerk session token stops being accepted. */
function expiresAt(token: string): number {
  try {
    const payload = (token.split(".")[1] ?? "")
      .replace(/-/g, "+")
      .replace(/_/g, "/");
    return (JSON.parse(atob(payload)) as { exp: number }).exp * 1000;
  } catch {
    return 0;
  }
}

function jittered(ms: number): number {
  return ms / 2 + Math.random() * (ms / 2);
}

/** The web app's connection to /v1/live, open only while the tab is visible. */
class LiveConnection {
  private socket: WebSocket | undefined;
  private stopped = false;
  private failures = 0;
  private connectedBefore = false;
  private authExpiry = 0;
  private heartbeatMs = 25_000;
  private retryTimer: ReturnType<typeof setTimeout> | undefined;
  private refreshTimer: ReturnType<typeof setTimeout> | undefined;
  private silenceTimer: ReturnType<typeof setTimeout> | undefined;
  private nextDelay: number | undefined;

  private readonly token: TokenSource;
  private readonly handlers: LiveHandlers;

  constructor(token: TokenSource, handlers: LiveHandlers) {
    this.token = token;
    this.handlers = handlers;
  }

  start() {
    document.addEventListener("visibilitychange", this.onVisibility);
    if (document.visibilityState === "visible") this.open();
  }

  stop() {
    this.stopped = true;
    document.removeEventListener("visibilitychange", this.onVisibility);
    this.disconnect();
  }

  private onVisibility = () => {
    if (document.visibilityState === "visible") {
      if (!this.socket && this.retryTimer === undefined) this.open();
    } else {
      this.disconnect();
    }
  };

  private disconnect() {
    clearTimeout(this.retryTimer);
    this.retryTimer = undefined;
    this.clearTimers();
    const socket = this.socket;
    this.socket = undefined;
    socket?.close(1000);
  }

  private clearTimers() {
    clearTimeout(this.refreshTimer);
    clearTimeout(this.silenceTimer);
  }

  private open() {
    if (this.stopped) return;
    const socket = new WebSocket(
      `${config.apiUrl.replace(/^http/, "ws")}/v1/live`,
    );
    this.socket = socket;
    socket.onmessage = (message) => {
      if (this.socket !== socket) return;
      let parsed: LiveServerMessage;
      try {
        parsed = JSON.parse(String(message.data)) as LiveServerMessage;
      } catch {
        return;
      }
      this.receive(socket, parsed).catch(() => socket.close());
    };
    socket.onclose = (closed) => {
      if (this.socket !== socket) return;
      this.socket = undefined;
      this.clearTimers();
      if (this.stopped || closed.code === LiveCloseCode.revoked) return;
      this.reconnect(
        closed.code === LiveCloseCode.overLimit
          ? OVER_LIMIT_WAIT_MS
          : undefined,
      );
    };
  }

  private reconnect(atLeastMs?: number) {
    if (this.stopped || document.visibilityState !== "visible") return;
    const backoff = jittered(
      Math.min(MAX_BACKOFF_MS, 1000 * 2 ** this.failures),
    );
    const delay = Math.max(this.nextDelay ?? backoff, atLeastMs ?? 0);
    this.nextDelay = undefined;
    this.failures += 1;
    this.retryTimer = setTimeout(() => {
      this.retryTimer = undefined;
      this.open();
    }, delay);
  }

  private send(socket: WebSocket, message: LiveDeviceMessage) {
    if (socket.readyState === WebSocket.OPEN)
      socket.send(JSON.stringify(message));
  }

  /** Two heartbeats without a word from the server means the connection is gone. */
  private heard(socket: WebSocket) {
    clearTimeout(this.silenceTimer);
    this.silenceTimer = setTimeout(() => socket.close(), this.heartbeatMs * 2);
  }

  private async receive(socket: WebSocket, message: LiveServerMessage) {
    this.heard(socket);
    switch (message.type) {
      case "challenge": {
        const token = await this.token(false);
        if (!token) return this.stop();
        this.send(socket, { type: "hello", role: "web", token });
        return;
      }
      case "ready":
        this.failures = 0;
        this.heartbeatMs = message.heartbeatMs;
        this.heard(socket);
        if (message.authExpiresAt)
          this.scheduleRefresh(socket, Date.parse(message.authExpiresAt));
        if (this.connectedBefore) this.handlers.onResync();
        this.connectedBefore = true;
        return;
      case "events": {
        const last = message.events.at(-1);
        this.handlers.onEvents(message.events);
        if (last) this.send(socket, { type: "ack", id: last.id });
        if (message.events.some((event) => event.type === "account.deleted"))
          this.handlers.onAccountDeleted();
        return;
      }
      case "reset":
        this.handlers.onResync();
        return;
      case "ping":
        this.send(socket, { type: "pong" });
        return;
      case "bye":
        this.nextDelay = jittered(message.reconnectAfterMs * 2);
        return;
      case "revoked":
        this.stop();
        this.handlers.onAccountDeleted();
        return;
    }
  }

  private scheduleRefresh(socket: WebSocket, expiry: number) {
    this.authExpiry = expiry;
    clearTimeout(this.refreshTimer);
    this.refreshTimer = setTimeout(
      () => void this.refresh(socket),
      Math.max(1000, expiry - Date.now() - REFRESH_AHEAD_MS),
    );
  }

  /** Clerk may still hand out the token the server already has; ask again past its cache if so. */
  private async refresh(socket: WebSocket) {
    let token = await this.token(false);
    if (token && expiresAt(token) <= this.authExpiry)
      token = await this.token(true);
    if (!token || this.socket !== socket) return;
    this.send(socket, { type: "auth", token });
    this.scheduleRefresh(socket, expiresAt(token));
  }
}

/** Keeps the account's live connection open while signed in, and hands what it hears to the page. */
export function useLive(enabled: boolean, handlers: LiveHandlers) {
  const { getToken } = useAuth();
  const token = useEffectEvent((fresh: boolean) =>
    getToken(fresh ? { skipCache: true } : undefined),
  );
  const onEvents = useEffectEvent(handlers.onEvents);
  const onResync = useEffectEvent(handlers.onResync);
  const onAccountDeleted = useEffectEvent(handlers.onAccountDeleted);

  useEffect(() => {
    if (!enabled) return;
    const connection = new LiveConnection((fresh) => token(fresh), {
      onEvents: (events) => onEvents(events),
      onResync: () => onResync(),
      onAccountDeleted: () => onAccountDeleted(),
    });
    connection.start();
    return () => connection.stop();
  }, [enabled]);
}
