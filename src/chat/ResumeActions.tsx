import * as cmd from "../state/commands";
import type { Agent, ProviderProfile } from "../state/types";

/** Trying a dropped session again, or leaving it for a new chat with the same agent. */
export function ResumeActions({
    agent,
    profile,
    cwd,
    retry,
    onRetry,
}: {
    agent: Agent;
    profile?: ProviderProfile;
    cwd?: string;
    retry: string;
    onRetry: () => void;
}) {
    const startNewChat = () =>
        cmd.addAgent(agent.type, undefined, undefined, {
            permissionMode: agent.permissionMode,
            profileId: agent.profileId,
            detectedExecutablePath: profile?.executablePath || agent.executablePath,
            cwd,
        });
    return (
        <div className="chat-connection-actions">
            <button type="button" onClick={onRetry}>
                {retry}
            </button>
            {agent.resumeId && (
                <button type="button" onClick={startNewChat}>
                    Start new chat
                </button>
            )}
        </div>
    );
}
