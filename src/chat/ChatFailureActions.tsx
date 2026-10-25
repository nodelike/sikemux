import { agentApi } from "../api/agents";
import { AccountSignIn } from "../agents/AccountSignIn";
import { signInAgent } from "../agents/AgentInstall";
import { agentLabel, loginArgs } from "@shared/agents/catalog";
import { accountsOf, moveChat } from "../agents/accounts";
import { useResource } from "../state/resources";
import { agentAccountR } from "../state/resources.defs";
import { useStore } from "../state/store";
import { reportError } from "../state/toast";
import type { Agent, ProviderProfile } from "../state/types";
import type { ChatFailure } from "@shared/chat/types";
import { useState } from "react";

/** A way the agent said, as it started, that it can be signed in. */
export type SignInMethod =
    | { kind: "terminal"; id: string; name: string; description: string | null; args: string[]; env: [string, string][] }
    | { kind: "agent"; id: string; name: string; description: string | null };

/** What the person can do about a turn that failed because of its account. */
export function ChatFailureActions({
    agent,
    profile,
    failure,
    signInMethods = [],
}: {
    agent: Agent;
    profile: ProviderProfile | undefined;
    failure: ChatFailure;
    signInMethods?: readonly SignInMethod[];
}) {
    if (!profile) {
        if (failure.kind !== "signIn") return null;
        const own = { type: agent.type, command: agent.executablePath ?? agent.type, configPath: null };
        if (signInMethods.length > 0) {
            return (
                <span className="chat-error-actions">
                    {signInMethods.map((method) => (
                        <button
                            key={method.id}
                            type="button"
                            title={method.description ?? undefined}
                            onClick={() =>
                                method.kind === "terminal"
                                    ? signInAgent(own, { method: { args: method.args, env: method.env }, retry: agent.id })
                                    : void agentApi
                                          .authenticateChat(agent.id, method.id)
                                          .then(() => agentApi.retryFailed(agent.id))
                                          .catch(reportError(`Sign in to ${agentLabel(agent.type)}`))
                            }>
                            {method.name}
                        </button>
                    ))}
                </span>
            );
        }
        if (!loginArgs(agent.type)) return null;
        return (
            <span className="chat-error-actions">
                <button type="button" onClick={() => signInAgent(own, { retry: agent.id })}>
                    Sign in to {agentLabel(agent.type)}
                </button>
            </span>
        );
    }
    if (failure.kind === "signIn") {
        return (
            <span className="chat-error-actions">
                <AccountSignIn profile={profile} label={`Sign in to ${profile.name}`} onSignedIn={() => moveChat(agent, profile)} />
            </span>
        );
    }
    if (failure.kind === "limit")
        return (
            <>
                {failure.resumesAt !== undefined && failure.resumesAt > Date.now() && (
                    <span className="chat-error-actions">
                        <button type="button" onClick={() => void agentApi.cancelResume(agent.id).catch(reportError("Cancel the resume"))}>
                            Don't resume
                        </button>
                    </span>
                )}
                <ContinueElsewhere agent={agent} profile={profile} />
            </>
        );
    return null;
}

function ContinueElsewhere({ agent, profile }: { agent: Agent; profile: ProviderProfile }) {
    const profiles = useStore((s) => s.providerProfiles);
    const own = useResource(agentAccountR, profile.provider, profile.configPath);
    const others = accountsOf(profile.provider, profiles).filter((other) => other.id !== profile.id);
    if (others.length === 0 || !own.data?.sessions) return null;
    return (
        <span className="chat-error-actions">
            {others.map((other) => (
                <ContinueOn key={other.id} agent={agent} account={other} sessions={own.data!.sessions!} />
            ))}
        </span>
    );
}

/* Only an account that is signed in and keeps chats where this one does can
   take the chat over. */
function ContinueOn({ agent, account, sessions }: { agent: Agent; account: ProviderProfile; sessions: string }) {
    const status = useResource(agentAccountR, account.provider, account.configPath);
    if (!status.data?.signedIn || status.data.sessions !== sessions) return null;
    return (
        <button type="button" onClick={() => moveChat(agent, account)}>
            Continue on {account.name}
        </button>
    );
}

/** What the person can do about a turn the network cut short: send it now, let it go, or read what the agent said. */
export function NetworkFailureActions({ agent, failure, onDismiss }: { agent: Agent; failure: ChatFailure; onDismiss: () => void }) {
    const [open, setOpen] = useState(false);
    return (
        <>
            <span className="chat-error-actions">
                <button type="button" onClick={() => void agentApi.retryFailed(agent.id).catch(reportError("Retry"))}>
                    Retry now
                </button>
                <button type="button" onClick={() => void agentApi.discardFailed(agent.id).then(onDismiss).catch(reportError("Discard"))}>
                    Discard
                </button>
                {failure.details && (
                    <button type="button" aria-expanded={open} onClick={() => setOpen(!open)}>
                        Details
                    </button>
                )}
            </span>
            {open && failure.details && <pre className="chat-error-details">{failure.details}</pre>}
        </>
    );
}
