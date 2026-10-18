import type { ChatMessage } from "./types";

/** A message another agent sent with agent_send, as the core labels it for the agent that gets it. */
export interface AgentMessageLetter {
    kind: "message";
    name: string;
    agentId: string;
    cli: string;
    text: string;
}

/** The person's answer to a question an agent asked, as the core hands it to the agent. */
export interface AnswerLetter {
    kind: "answer";
    /** The name of the agent that asked. */
    asker: string;
    cli: string;
    question: string;
    text: string;
    /** The person's answer is a secret the agent reads from a file, so `text` only says where. */
    secret: boolean;
}

export type AgentLetter = AgentMessageLetter | AnswerLetter;

const LETTER = /<(agent-message|question-answer) ([^>]*)>\n([\s\S]*?)\n<\/\1>/g;
const ATTRIBUTE = /([a-z-]+)="([^"]*)"/g;

const unescaped = (value: string) =>
    value
        .replace(/&quot;/g, '"')
        .replace(/&lt;/g, "<")
        .replace(/&gt;/g, ">")
        .replace(/&amp;/g, "&");

export const isAgentLetter = (text: string): boolean => /^<(agent-message|question-answer) /.test(text.trimStart());

function attributes(source: string): Record<string, string> {
    const found: Record<string, string> = {};
    for (const [, key, value] of source.matchAll(ATTRIBUTE)) found[key] = unescaped(value);
    return found;
}

/** The messages from other agents and answers a text is made of, or null when it holds anything else. */
export function agentLetters(text: string): AgentLetter[] | null {
    if (!isAgentLetter(text)) return null;
    const letters: AgentLetter[] = [];
    const rest = text.replace(LETTER, (_whole, tag: string, source: string, body: string) => {
        const found = attributes(source);
        const unclosed = body.replace(new RegExp(`&lt;/${tag}`, "g"), `</${tag}`);
        letters.push(
            tag === "question-answer"
                ? {
                      kind: "answer",
                      asker: found.asker ?? "",
                      cli: found.cli ?? "",
                      question: found.question ?? "",
                      text: unclosed,
                      secret: found.secret === "true",
                  }
                : { kind: "message", name: found.from ?? "", agentId: found["agent-id"] ?? "", cli: found.cli ?? "", text: unclosed },
        );
        return "";
    });
    return letters.length > 0 && rest.trim() === "" ? letters : null;
}

/** What a sent message holds from other agents, when that is all it holds. */
export function messageLetters(message: ChatMessage): AgentLetter[] | null {
    if (message.role !== "user" || message.parts.some((part) => part.kind !== "text")) return null;
    return agentLetters(message.parts.map((part) => (part.kind === "text" ? part.text : "")).join(""));
}
