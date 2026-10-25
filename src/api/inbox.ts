import type { AgentMessageRows } from "@shared/agents/agentMessages";
import type { Inbox } from "@shared/wire.generated";
import { coreCall } from "./core";

export type { Inbox, InboxItem, RemoteAgent } from "@shared/wire.generated";

/** Sent by the window whenever the core's inbox changes. */
export const INBOX_EVENT = "inbox_changed";

export const inboxApi = {
    read: () => coreCall<Inbox>("inbox"),
    agentMessageRows: (project: string) => coreCall<AgentMessageRows>("agentMessageRows", { project }),
};
