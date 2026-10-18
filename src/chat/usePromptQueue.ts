import { useCallback, useEffect, useRef, useState, type Dispatch, type RefObject } from "react";
import { errorText } from "../lib/format";
import { acpApi, type PromptContext } from "../api/acp";
import type { Agent } from "../state/types";
import * as cmd from "../state/commands";
import { unknownCommand } from "./composerInput";
import { combineQueued, nextBatch, type OutgoingMessage, type QueuedMessage } from "./queuedMessages";
import type { AcpAvailableCommand, ChatAction, ChatState } from "./types";

/* Left off when there is none, so a plain message goes out exactly as before. */
const contextArgs = (context: PromptContext[]): [] | [PromptContext[]] => (context.length ? [context] : []);

/** How long a queue waits after a turn ends, for word that it failed. */
const SETTLE_MS = 150;

export function usePromptQueue({
    agentRef,
    agentId,
    cwd,
    connection,
    running,
    replaying,
    commands,
    steerable,
    held = false,
    dispatch,
    onError,
}: {
    agentRef: RefObject<Agent>;
    agentId: string;
    cwd: string;
    connection: ChatState["connection"];
    running: boolean;
    replaying: boolean;
    commands: AcpAvailableCommand[];
    steerable: boolean;
    /** The last turn failed for want of the network: what is queued waits for it to go again. */
    held?: boolean;
    dispatch: Dispatch<ChatAction>;
    onError: (message: string | null) => void;
}) {
    const [queued, setQueued] = useState<QueuedMessage[]>([]);
    const queuedCount = useRef(0);
    const [early, setEarly] = useState<{ messageId: string; message: OutgoingMessage } | null>(null);

    useEffect(() => {
        setQueued([]);
        setEarly(null);
    }, [agentId, cwd]);

    const showPrompt = useCallback(
        ({ text, paths, context }: OutgoingMessage) => {
            const messageId = crypto.randomUUID();
            dispatch({ type: "local_prompt", text, paths, context, messageId });
            cmd.titleAgentFromPrompt(agentRef.current.id, text || context[0]?.title || "");
            return messageId;
        },
        [agentRef, dispatch],
    );

    const deliver = useCallback(
        async ({ text, paths, context }: OutgoingMessage, messageId: string) => {
            try {
                await acpApi.prompt(agentRef.current.id, text, paths, context, messageId);
            } catch (error) {
                dispatch({ type: "error", message: errorText(error) });
            }
        },
        [agentRef, dispatch],
    );

    const promptNow = useCallback((message: OutgoingMessage) => deliver(message, showPrompt(message)), [deliver, showPrompt]);

    /* A message sent while the agent is still starting is already on screen
       with the turn shown running; it goes out the moment the agent can take it. */
    useEffect(() => {
        if (connection !== "ready" || early === null) return;
        setEarly(null);
        void deliver(early.message, early.messageId);
    }, [connection, early, deliver]);

    /* Messages written mid-turn wait, then go out together as one prompt once
       the running turn ends, so nothing in flight is cut short. They wait a
       moment more, since a turn the network cut short says so just after it
       ends, and then they wait for that turn to go again. */
    useEffect(() => {
        if (connection !== "ready" || running || held || queued.length === 0) return;
        const timer = window.setTimeout(() => {
            const batch = nextBatch(queued);
            const sent = new Set(batch.map((message) => message.id));
            setQueued((current) => current.filter((message) => !sent.has(message.id)));
            void promptNow(combineQueued(batch));
        }, SETTLE_MS);
        return () => window.clearTimeout(timer);
    }, [promptNow, queued, connection, running, held]);

    /* Steering stops whatever the agent has in flight so it reads this message
       now, so a message only goes this way when it is asked to. */
    const steer = async (messages: QueuedMessage[]) => {
        const steered = new Set(messages.map((message) => message.id));
        setQueued((current) => current.filter((candidate) => !steered.has(candidate.id)));
        const message = combineQueued(messages);
        dispatch({ type: "local_prompt", text: message.text, paths: message.paths, context: message.context });
        try {
            if ((await acpApi.steer(agentId, message.text, message.paths, ...contextArgs(message.context))) !== "promptRequired") return;
            await acpApi.prompt(agentId, message.text, message.paths, ...contextArgs(message.context));
        } catch (error) {
            dispatch({ type: "error", message: errorText(error) });
        }
    };

    /** Says whether the composer may clear what it just handed over. */
    const send = (outgoing: OutgoingMessage, steerNow: boolean): boolean => {
        const unknown = unknownCommand(outgoing.text, commands);
        if (unknown) {
            onError(`/${unknown} is not available in this session`);
            return false;
        }
        onError(null);
        if (connection === "ready" && !running) {
            void promptNow(outgoing);
            return true;
        }
        const coming = connection === "connecting" || connection === "starting" || connection === "initializing";
        if (coming && !running && !replaying && early === null) {
            setEarly({ messageId: showPrompt(outgoing), message: outgoing });
            return true;
        }

        /* Written mid-turn, or while the session is still coming up: it waits
           in the queue and goes out with the rest of it once the session is free. */
        queuedCount.current += 1;
        const message: QueuedMessage = { id: `queued-${queuedCount.current}`, ...outgoing };
        if (steerNow && steerable && running) {
            void steer([...queued, message]);
            return true;
        }
        setQueued((current) => [...current, message]);
        return true;
    };

    const drop = (id: string) => setQueued((current) => current.filter((message) => message.id !== id));

    return { queued, send, steer, drop };
}
