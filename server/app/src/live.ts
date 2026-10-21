import { useAuth } from "@clerk/react";
import type { AccountEvent } from "@sikemux/protocol";
import { LiveClient, liveUrl } from "@sikemux/protocol/live";
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

/** The web app's connection to /v1/live, open only while the tab is visible. */
function liveConnection(token: TokenSource, handlers: LiveHandlers) {
  let connectedBefore = false;
  const client = new LiveClient({
    url: liveUrl(config.apiUrl),
    connect: (url, on) => {
      const socket = new WebSocket(url);
      socket.onmessage = (message) => on.message(message.data);
      socket.onclose = (closed) => on.closed(closed.code);
      return socket;
    },
    token,
    hello: async () => {
      const signedIn = await token(false);
      return signedIn ? { type: "hello", role: "web", token: signedIn } : null;
    },
    events: async (events) => {
      handlers.onEvents(events);
      return {
        ack: events.at(-1)?.id ?? 0,
        gone: events.some((event) => event.type === "account.deleted")
          ? "account_deleted"
          : undefined,
      };
    },
    ready: () => {
      if (connectedBefore) handlers.onResync();
      connectedBefore = true;
    },
    reset: handlers.onResync,
    gone: handlers.onAccountDeleted,
  });
  const onVisibility = () => {
    if (document.visibilityState === "visible") client.start();
    else client.stop();
  };
  return {
    start() {
      document.addEventListener("visibilitychange", onVisibility);
      onVisibility();
    },
    stop() {
      document.removeEventListener("visibilitychange", onVisibility);
      client.stop();
    },
  };
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
    const connection = liveConnection((fresh) => token(fresh), {
      onEvents: (events) => onEvents(events),
      onResync: () => onResync(),
      onAccountDeleted: () => onAccountDeleted(),
    });
    connection.start();
    return () => connection.stop();
  }, [enabled]);
}
