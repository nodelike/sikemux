import type { AgentType } from "@shared/agents/catalog";
import type { AgentBuddy } from "@shared/buddies/shared";

export type { AgentType };

/** What the chat view knows of the agent whose chat is shared. */
export interface Agent {
  id: string;
  type: AgentType;
  buddy?: AgentBuddy;
}
