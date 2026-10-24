import { reportError, useNearestProject } from "../../../plugin-api/host";
import { useResourceEnabled } from "../../../plugin-api/resources";
import { Dropdown, type DropdownOption } from "../../../plugin-api/ui";
import { awsApi } from "../api";
import { agentProfileV } from "../resources";

const UNSET = "";
const LATER = "Applies to agents started after a change.";

function pick(project: string | null, profile: string): void {
    awsApi.pickAgentProfile(project, profile || null).catch(reportError("pick the AWS profile for agents"));
}

/** The profile agents start on as AWS_PROFILE, for the nearest project and for every other one. */
export function AwsAgentProfile({ paneId, active }: { paneId: string; active: boolean }) {
    const project = useNearestProject();
    const picks = useResourceEnabled(active, agentProfileV, paneId, project?.cwd ?? null).data;
    const names = picks?.profiles ?? [];
    const options = (unset: string): DropdownOption[] => [{ value: UNSET, label: unset }, ...names.map((name) => ({ value: name, label: name }))];

    return (
        <div className="aws-agent-profile">
            <div className="aws-nav-label">Agents use</div>
            {picks?.fromShell ? (
                <p className="aws-agent-profile-note">{picks.fromShell}</p>
            ) : (
                <>
                    {project && (
                        <>
                            <span className="aws-agent-profile-for">{project.name}</span>
                            <Dropdown
                                value={picks?.project ?? UNSET}
                                options={options(picks?.projectUnset ?? "Default")}
                                onChange={(profile) => pick(project.cwd, profile)}
                                label={`AWS profile for agents in ${project.name}`}
                                title={LATER}
                                search={names.length > 8 ? "Find a profile" : undefined}
                            />
                        </>
                    )}
                    <span className="aws-agent-profile-for">{project ? "Other projects" : "Every project"}</span>
                    <Dropdown
                        value={picks?.default ?? UNSET}
                        options={options(picks?.defaultUnset ?? "None")}
                        onChange={(profile) => pick(null, profile)}
                        label="AWS profile for agents in other projects"
                        title={LATER}
                        search={names.length > 8 ? "Find a profile" : undefined}
                    />
                </>
            )}
        </div>
    );
}
