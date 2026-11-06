import { createContext } from "react";
import type { AgentType } from "@shared/agents/catalog";
import type { Subagents } from "@shared/chat/subagents";
import type { ToolDetail } from "@shared/wire.generated";

export const ChatAgentContext = createContext<AgentType>("agent");

export const ChatSubagentsContext = createContext<Subagents>({
  card: () => undefined,
  rows: {},
});

/** Fetches what a tool call did, line by line, for a row opened to show it. */
export const ToolDetailContext = createContext<
  (toolCallId: string) => Promise<ToolDetail>
>(() => Promise.resolve({}));

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
