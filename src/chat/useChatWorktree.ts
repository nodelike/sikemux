import { useCallback, useEffect, useRef, useState } from "react";
import { coreCall } from "../api/core";
import { useProjectRepo } from "../git/projectRepo";
import { errorText } from "../lib/format";
import { deliverToAgent } from "../agents/agentInbox";
import { worktreeSwitchState } from "./worktreeSwitch";
import * as cmd from "../state/commands";
import { useResourceEnabled } from "../state/resources";
import { gitWorktreesR } from "../state/resources.defs";
import { usePreference } from "../state/preferences";
import type { Agent, AgentWorktree } from "../state/types";
import type { OutgoingMessage } from "./queuedMessages";
import type { ChatRows } from "./types";

/** The checkout the core cut for an agent and moved it into. */
export interface PreparedWorktree {
    worktree: AgentWorktree;
    /** Where the agent works inside the new checkout. */
    cwd: string;
    /** Why the project's setup commands did not all finish, when they did not. */
    setupError: string | null;
}

/** Waits for the session to come back up in the worktree before the message that started it goes out. */
interface PendingSend {
    cwd: string;
    message: OutgoingMessage;
    /** The first render in the new folder still shows the old session's connection. */
    seen: boolean;
}

/**
 * The composer's Worktree switch. With it on, a fresh chat's first message
 * first moves the agent into a new worktree, and only then goes out.
 */
export function useChatWorktree({
    agent,
    cwd,
    visible,
    started,
    connection,
    running,
    send,
    onError,
}: {
    agent: Agent;
    cwd: string;
    visible: boolean;
    started: boolean;
    connection: ChatRows["connection"];
    running: boolean;
    send: (message: OutgoingMessage, steerNow: boolean) => boolean;
    onError: (message: string | null) => void;
}) {
    const projectDefault = usePreference("agentWorktreeDefaults")?.[cwd] === true;
    const [choice, setChoice] = useState<{ cwd: string; on: boolean } | null>(null);
    const on = choice?.cwd === cwd ? choice.on : projectDefault;
    const [step, setStep] = useState<string | null>(null);
    const pending = useRef<PendingSend | null>(null);
    const project = useProjectRepo(cwd, visible && !agent.worktree && !started);
    const checkouts = useResourceEnabled(visible && !agent.worktree && !started && project.repo !== null, gitWorktreesR, project.repo ?? cwd);
    const noRepo = project.repos !== null && project.repo === null;
    const isRepo = noRepo ? false : checkouts.data ? true : checkouts.status === "error" ? false : null;
    const state = worktreeSwitchState({ worktree: agent.worktree, isRepo, started, preparing: step, on });

    const toggle = () => {
        if (state.kind !== "choosing") return;
        setChoice({ cwd, on: !on });
        cmd.setAgentWorktreeDefault(cwd, !on);
    };

    const agentId = agent.id;
    const giveBack = useCallback(
        (message: OutgoingMessage, error: string) => {
            setStep(null);
            onError(error);
            deliverToAgent(agentId, message);
        },
        [agentId, onError],
    );

    const sendMessage = (message: OutgoingMessage, steerNow: boolean): boolean => {
        if (state.kind === "preparing") return false;
        if (state.kind !== "choosing" || !state.on) return send(message, steerNow);
        onError(null);
        setStep("Creating worktree");
        void coreCall<PreparedWorktree>("createAgentWorktree", {
            agentId,
            message: message.text || message.context[0]?.title || "",
            repo: project.repo !== cwd ? project.repo : null,
        }).then(
            (prepared) => {
                if (!prepared.setupError) pending.current = { cwd: prepared.cwd, message, seen: false };
                cmd.setAgentWorktree(agentId, prepared.cwd, prepared.worktree);
                if (prepared.setupError) {
                    giveBack(message, `${prepared.setupError}. The worktree is kept at ${prepared.worktree.path}; send again to start there.`);
                    return;
                }
                setStep("Starting the agent in the worktree");
            },
            (failure: unknown) => giveBack(message, `Could not create a worktree: ${errorText(failure)}`),
        );
        return true;
    };

    useEffect(() => {
        const waiting = pending.current;
        if (!waiting || waiting.cwd !== cwd) return;
        if (!waiting.seen) {
            waiting.seen = true;
            return;
        }
        if (connection === "error" || connection === "stopped") {
            pending.current = null;
            giveBack(waiting.message, "The agent did not start in the worktree");
            return;
        }
        if (connection !== "ready" || running) return;
        pending.current = null;
        setStep(null);
        send(waiting.message, false);
    }, [cwd, connection, running, send, giveBack]);

    useEffect(
        () => () => {
            if (pending.current) deliverToAgent(agentId, pending.current.message);
        },
        [agentId],
    );

    return { state, step, toggle, sendMessage };
}
