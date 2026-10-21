import { reportError, useNearestProject } from "../../../plugin-api/host";
import { invalidate, useResource } from "../../../plugin-api/resources";
import { Dropdown, type DropdownOption } from "../../../plugin-api/ui";
import { gcpApi, type AgentPick, type AgentPicks } from "../api";
import { agentPicksR, gcpProjectsR } from "../resources";
import { gcpSettings } from "../state";

const UNSET = "";
const LATER = "Applies to agents and terminals started after a change.";

function save(folder: string | null, pick: AgentPick): void {
    gcpApi
        .pickForAgents(folder, pick)
        .then(() => invalidate((kind) => kind === agentPicksR.kind))
        .catch(reportError("pick the Google Cloud project for agents"));
}

/** What a pick comes to, in a few words. */
export function describePick(picks: AgentPicks | undefined): string {
    const effective = picks?.effective;
    if (!effective) return "gcloud's own configuration";
    const { account, project } = effective.pick;
    const what = [project, account && `as ${account}`].filter(Boolean).join(" ") || "gcloud's own configuration";
    return effective.source === "environment" ? `${what}, from your shell` : what;
}

/**
 * The Google account and project agents and terminals start on, for the nearest Sikemux project and for every
 * other one, set as CLOUDSDK_CORE_PROJECT and CLOUDSDK_CORE_ACCOUNT.
 */
export function AgentSetup() {
    const folder = useNearestProject();
    const picks = useResource(agentPicksR, folder?.cwd ?? null).data;
    const account = gcpSettings.useSelect((s) => s.account);
    const projects = (useResource(gcpProjectsR, account ?? "").data ?? []).map((project) => project.id);
    const own = picks?.effective?.source === "environment";
    const options = (forFolder: boolean): DropdownOption[] => [
        { value: UNSET, label: forFolder ? (picks?.default?.project ? `Default (${picks.default.project})` : "Default") : "gcloud's configuration" },
        ...projects.map((id) => ({ value: id, label: id })),
    ];
    const pickFor = (forFolder: string | null, project: string) => save(forFolder, { account: project ? account : null, project: project || null });

    return (
        <div className="gcp-agents">
            <div className="gcp-nav-group">Agents use</div>
            {own ? (
                <p className="gcp-agents-note">{describePick(picks)}</p>
            ) : (
                <>
                    {folder && (
                        <>
                            <span className="gcp-agents-for">{folder.name}</span>
                            <Dropdown
                                value={picks?.project?.project ?? UNSET}
                                options={options(true)}
                                onChange={(project) => pickFor(folder.cwd, project)}
                                label={`Google Cloud project for agents in ${folder.name}`}
                                title={LATER}
                                search={projects.length > 8 ? "Find a project" : undefined}
                            />
                        </>
                    )}
                    <span className="gcp-agents-for">{folder ? "Other projects" : "Every project"}</span>
                    <Dropdown
                        value={picks?.default?.project ?? UNSET}
                        options={options(false)}
                        onChange={(project) => pickFor(null, project)}
                        label="Google Cloud project for agents in other projects"
                        title={LATER}
                        search={projects.length > 8 ? "Find a project" : undefined}
                    />
                </>
            )}
        </div>
    );
}
