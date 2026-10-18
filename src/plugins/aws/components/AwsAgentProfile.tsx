import { reportError, useNearestProject } from "../../../plugin-api/host";
import { invalidate, useResource } from "../../../plugin-api/resources";
import { Dropdown, type DropdownOption } from "../../../plugin-api/ui";
import { awsApi, type AgentProfilePicks } from "../api";
import { awsAgentProfileR, awsProfilesR } from "../resources";

const UNSET = "";
const LATER = "Applies to agents started after a change.";

function pick(project: string | null, profile: string): void {
    awsApi
        .pickAgentProfile(project, profile || null)
        .then(() => invalidate((kind) => kind === awsAgentProfileR.kind))
        .catch(reportError("pick the AWS profile for agents"));
}

function unsetLabel(picks: AgentProfilePicks | undefined, forProject: boolean): string {
    const effective = picks?.effective;
    if (forProject) return picks?.default ? `Default (${picks.default})` : "Default";
    return effective?.source === "automatic" && effective.profile ? `Automatic (${effective.profile})` : "None";
}

/** The profile agents start on as AWS_PROFILE, for the nearest project and for every other one. */
export function AwsAgentProfile() {
    const project = useNearestProject();
    const picks = useResource(awsAgentProfileR, project?.cwd ?? null).data;
    const names = (useResource(awsProfilesR).data ?? []).map((profile) => profile.name);
    const own = picks?.effective?.source === "environment" ? picks.effective : null;
    const options = (forProject: boolean): DropdownOption[] => [
        { value: UNSET, label: unsetLabel(picks, forProject) },
        ...names.map((name) => ({ value: name, label: name })),
    ];

    return (
        <div className="aws-agent-profile">
            <div className="aws-nav-label">Agents use</div>
            {own ? (
                <p className="aws-agent-profile-note">{own.profile ? `${own.profile}, from your shell` : "The AWS keys your shell sets"}</p>
            ) : (
                <>
                    {project && (
                        <>
                            <span className="aws-agent-profile-for">{project.name}</span>
                            <Dropdown
                                value={picks?.project ?? UNSET}
                                options={options(true)}
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
                        options={options(false)}
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
