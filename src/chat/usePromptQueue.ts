import { useCallback, useEffect, useState, type RefObject } from "react";
import { errorText } from "../lib/format";
import { acpApi } from "../api/acp";
import type { Agent } from "../state/types";
import { unknownCommand } from "./composerInput";
import type { OutgoingMessage } from "./queuedMessages";
import type { QueuedMessage } from "./types";
import type { ChatCommandInfo, ChatRows } from "./types";

/**
 * Sends what the person writes. Written mid-turn, a message waits in the core's queue, which every app shows and the
 * core sends on once the turn ends; written while the agent is still starting, they go out in order once it can take them.
 */
export function usePromptQueue({
    agentRef,
    agentId,
    cwd,
    connection,
    running,
    queued,
    commands,
    steerable,
    show,
    unshow,
    onError,
    onSent,
}: {
    agentRef: RefObject<Agent>;
    agentId: string;
    cwd: string;
    connection: ChatRows["connection"];
    running: boolean;
    queued: QueuedMessage[];
    commands: ChatCommandInfo[];
    steerable: boolean;
    /** Shows a message until the core shows it back, answering the id it goes out with. */
    show: (text: string, paths: string[], context: OutgoingMessage["context"]) => string;
    unshow: (messageId: string) => void;
    onError: (message: string | null) => void;
    onSent: () => void;
}) {
    /* Only the first shows before the agent can take it. */
    const [early, setEarly] = useState<{ message: OutgoingMessage; messageId?: string }[]>([]);

    useEffect(() => setEarly([]), [agentId, cwd]);

    const showPrompt = useCallback(({ text, paths, context }: OutgoingMessage) => show(text, paths, context), [show]);

    const deliver = useCallback(
        async ({ text, paths, context }: OutgoingMessage, messageId?: string) => {
            try {
                await acpApi.prompt(agentRef.current.id, text, paths, context, messageId);
                onSent();
            } catch (error) {
                if (messageId) unshow(messageId);
                onError(errorText(error));
            }
        },
        [agentRef, unshow, onError, onSent],
    );

    useEffect(() => {
        if (connection !== "ready" || early.length === 0) return;
        setEarly([]);
        const [first, ...rest] = early;
        const sent = deliver(first.message, first.messageId ?? showPrompt(first.message));
        void rest.reduce((before, { message }) => before.then(() => deliver(message)), sent);
    }, [connection, early, deliver, showPrompt]);

    const failed = (error: unknown) => onError(errorText(error));

    /* Steering puts messages into what the agent has in flight, so a message only goes this way when it is asked to. */
    const steer = (messages: QueuedMessage[], message?: OutgoingMessage) =>
        acpApi
            .steerQueued(
                agentId,
                messages.map((queued) => queued.id),
                message,
            )
            .catch(failed);

    /** Says whether the composer may clear what it just handed over. */
    const send = (outgoing: OutgoingMessage, steerNow: boolean): boolean => {
        const unknown = unknownCommand(outgoing.text, commands);
        if (unknown) {
            onError(`/${unknown} is not available in this session`);
            return false;
        }
        onError(null);
        if (running && steerNow && steerable) {
            void steer(queued, outgoing);
            return true;
        }
        if (running) {
            void deliver(outgoing);
            return true;
        }
        if (connection === "ready") {
            void deliver(outgoing, showPrompt(outgoing));
            return true;
        }
        const shown = early.length === 0;
        setEarly([...early, { message: outgoing, ...(shown && { messageId: showPrompt(outgoing) }) }]);
        return true;
    };

    const drop = (id: string) => void acpApi.dropQueued(agentId, [id]).catch(failed);

    return { queued, send, steer, drop };
}
