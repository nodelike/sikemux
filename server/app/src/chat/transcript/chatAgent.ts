import { createContext } from "react";
import type { AgentType } from "@shared/agents/catalog";
import type { ChatMessage, SubagentCard, ToolDetail } from "@shared/chat/types";

export const ChatAgentContext = createContext<AgentType>("agent");

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

const OPENABLE = new Set(["http:", "https:", "mailto:"]);

/** Opens a web or mail link in a new tab; anything else names something on the sharer's computer. */
export function openLink(href: string) {
  const url = URL.parse(href);
  if (url && OPENABLE.has(url.protocol))
    window.open(url.href, "_blank", "noopener,noreferrer");
}
