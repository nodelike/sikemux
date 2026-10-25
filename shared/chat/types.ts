/* A chat as the core folds it. Clients draw these; none reads an agent's own messages. */
export type {
    AccountMove,
    AgentLetter,
    AgentNotice,
    AsyncTask,
    ChatChange,
    ChatCommandInfo,
    ChatFailure,
    ChatMessage,
    ChatPage,
    ChatPart,
    ChatView,
    ContextUsage,
    DiffLine,
    PartTree,
    PermissionAsk,
    QueuedMessage,
    SentContext,
    SubagentCard,
    TaskNotice,
    ToolAction,
    ToolCall,
    ToolDetail,
    ToolDiff,
    ToolOutput,
    WorkflowAgent,
} from "../wire.generated";
export type { ChatRows } from "./rows";

/** A picture, file or resource an agent put in a message, as its protocol gives it and the core passes it on. */
export interface ContentBlock {
    type: string;
    text?: string;
    data?: string;
    mimeType?: string;
    name?: string;
    title?: string;
    uri?: string;
    resource?: unknown;
}

/* One run of a code fence that reads as one colour, and the lines they make up.
   A run with nothing set is plain text, which keeps the colour the stylesheet
   already gives the fence. */
export interface CodeToken {
    readonly text: string;
    readonly color?: string;
    readonly italic?: boolean;
    readonly bold?: boolean;
    readonly underline?: boolean;
}

export type CodeLine = readonly CodeToken[];
