import { createContext } from "react";
import { fsapi } from "../api/fs";
import { invokeCommand as invoke } from "../api/invoke";
import * as cmd from "../state/commands";
import { swallow } from "../state/toast";
import type { Agent } from "../state/types";
import { safeWebUrl } from "../terminal/interactions";
import { localPath } from "./imagePreview";

export const ChatAgentContext = createContext<{ id: string; type: Agent["type"] }>({ id: "", type: "claude" });

/** Scrolls the transcript as the reader asked, for a wheel turned over a page, which the transcript never sees. */
export const ReaderScrollContext = createContext<(deltaY: number) => void>(() => {});

export function openLink(href: string, agentId: string, external: boolean) {
    const path = localPath(href);
    const webUrl = safeWebUrl(href);
    if (path) void fsapi.revealInFinder(path).catch(swallow("reveal chat file"));
    else if (webUrl && agentId && !external) cmd.openUrlOnDesk(agentId, webUrl);
    else void invoke("open_url", { url: href, app: null, shortcut: null }).catch(swallow("open chat link"));
}
