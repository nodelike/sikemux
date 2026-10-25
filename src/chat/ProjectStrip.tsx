import type { ReactNode } from "react";
import { useProjectRepo } from "../git/projectRepo";
import { basename, prettyPath } from "@shared/lib/paths";
import { ProjectPicker } from "../projects/ProjectPicker";
import * as cmd from "../state/commands";
import { useResource } from "../state/resources";
import { gitOverviewR } from "../state/resources.defs";
import { useStore } from "../state/store";
import { IconChevron, IconGit } from "@shared/ui/Icons";
import { THIS_HOST, hostName } from "../hosts/hosts";
import { hostMark, projectMark } from "../hosts/placeMarks";
import { WorktreeToggle } from "./ChatWorktree";
import type { WorktreeSwitchState } from "./worktreeSwitch";
import "../styles/chat/project-strip.css";

/**
 * Where the chat runs, docked on its composer: the host, the project and the branch or worktree on the left, the
 * account on the right. Until the first message the project opens a switcher that moves the chat elsewhere.
 */
export function ProjectStrip({
    agentId,
    host,
    project,
    cwd,
    started,
    worktree,
    pull,
    account,
}: {
    agentId: string;
    host: string | undefined;
    /** The project's own folder, which a chat in a worktree does not run in. */
    project: string;
    cwd: string;
    started: boolean;
    worktree: { state: WorktreeSwitchState; toggle: () => void };
    pull: ReactNode;
    account: ReactNode;
}) {
    const home = useStore((s) => s.home);
    const hostLabel = useStore((s) => hostName(s, host) ?? "This host");
    const repo = useProjectRepo(cwd).repo ?? cwd;
    const branch = useResource(gitOverviewR, repo).data?.status?.branch;
    const name = basename(project) || project;
    const where = worktree.state.kind === "hidden" || worktree.state.kind === "choosing" ? null : worktree.state;
    return (
        <div className="chat-project-strip">
            <span className="chat-project-host" title={hostLabel}>
                {hostMark(host ? { kind: "core", host } : THIS_HOST, 12)}
                <span>{hostLabel}</span>
            </span>
            <IconChevron size={10} className="chat-project-sep" />
            {started ? (
                <span className="chat-project-trigger static" title={prettyPath(project, home)}>
                    {projectMark(12)}
                    <span>{name}</span>
                </span>
            ) : (
                <ProjectPicker
                    className="chat-project-trigger"
                    host={host}
                    value={project}
                    title={prettyPath(project, home)}
                    opens="above"
                    onPick={(choice) => {
                        if (choice.path !== project) void cmd.moveAgentToProject(agentId, choice.path);
                    }}
                />
            )}
            {(where || branch) && <span className="chat-project-divider" aria-hidden="true" />}
            {where ? (
                <WorktreeToggle state={where} onToggle={worktree.toggle} />
            ) : (
                branch && (
                    <span className="chat-project-branch" title={branch}>
                        <IconGit size={11} />
                        <span>{branch}</span>
                    </span>
                )
            )}
            {worktree.state.kind === "choosing" && <WorktreeToggle state={worktree.state} onToggle={worktree.toggle} />}
            {pull}
            <span className="chat-project-spacer" />
            {account}
        </div>
    );
}
