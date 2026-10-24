import type { AgentMessageRows } from "../agents/agentMessages";
import type { Inbox } from "./generated/wire";
import { coreCall } from "./core";

export type { Inbox, InboxItem, RemoteAgent } from "./generated/wire";

/** Sent by the window whenever the core's inbox changes. */
export const INBOX_EVENT = "inbox_changed";

export const inboxApi = {
    read: () => coreCall<Inbox>("inbox"),
    agentMessageRows: (project: string) => coreCall<AgentMessageRows>("agentMessageRows", { project }),
};
