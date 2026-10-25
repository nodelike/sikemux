import { createContext } from "react";
import { fsapi } from "./page/files";
import * as cmd from "./page/commands";
import { swallow } from "./page/toast";
import type { Agent } from "./page/agent";
import type { ChatMessage, SubagentCard, ToolDetail } from "@shared/chat/types";
import { safeWebUrl } from "@shared/lib/urls";
import { localPath } from "@shared/lib/imagePaths";
import { openUrl } from "./page/links";

export const ChatAgentContext = createContext<{
  id: string;
  type: Agent["type"];
}>({ id: "", type: "claude" });

/** Every subagent card the chat holds, and each subagent's own rows, by its session. */
export const ChatSubagentsContext = createContext<{
  cards: ReadonlyMap<string, SubagentCard>;
  rows: Readonly<Record<string, ChatMessage[]>>;
}>({ cards: new Map(), rows: {} });

/** Fetches what a tool call did, line by line, for a row opened to show it; none where nothing can be asked. */
export const ToolDetailContext = createContext<
  ((toolCallId: string) => Promise<ToolDetail>) | null
>(null);

/** Scrolls the transcript as the reader asked, for a wheel turned over a page, which the transcript never sees. */
export const ReaderScrollContext = createContext<(deltaY: number) => void>(
  () => {},
);

export function openLink(href: string, agentId: string, external: boolean) {
  const path = localPath(href);
  const webUrl = safeWebUrl(href);
  if (path) void fsapi.revealInFinder(path).catch(swallow("reveal chat file"));
  else if (webUrl && agentId && !external) cmd.openUrlOnDesk(agentId, webUrl);
  else void openUrl(href).catch(swallow("open chat link"));
}
