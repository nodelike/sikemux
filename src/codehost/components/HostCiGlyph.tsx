import type { ComponentType } from "react";
import type { PluginTopBarProps } from "../../plugin-api";
import { useResourceEnabled } from "../../plugin-api/resources";
import { Tooltip } from "../../plugin-api/ui";
import { openGitArea } from "../../state/commands";
import { useHostRepo } from "../project";
import { hostStatusR, runsR } from "../resources";
import { formatAgo, isUnfinished, outcomeOf, OUTCOME_LABEL } from "../runStatus";
import { OutcomeIcon } from "./ActionsIcon";
import { useEvery } from "./hooks";
import "../topbar.css";

const LIVE_REFRESH_MS = 10_000;
const IDLE_REFRESH_MS = 20_000;

function HostCiGlyph({ hostId, projectCwd }: PluginTopBarProps & { hostId: string }) {
    const found = useHostRepo(projectCwd, !!projectCwd);
    const repo = found.repo?.provider === hostId ? found.repo : null;
    const status = useResourceEnabled(!!repo, hostStatusR, hostId, repo?.account ?? null);
    const signedIn = !!status.data?.ok;
    const runs = useResourceEnabled(signedIn && !!repo, runsR, {
        ...(repo ?? { provider: hostId, owner: "", name: "" }),
        branch: found.branch ?? undefined,
        perPage: 1,
    });
    const latest = runs.data?.runs[0] ?? null;
    const live = !!latest && isUnfinished(latest);

    useEvery(signedIn && !!repo, live ? LIVE_REFRESH_MS : IDLE_REFRESH_MS, () => void runs.refresh());

    if (!repo || !latest) return null;

    const outcome = outcomeOf(latest);
    const label = `${OUTCOME_LABEL[outcome]} · ${latest.name} #${latest.runNumber} · ${formatAgo(latest.createdAt, Date.now())}`;
    return (
        <Tooltip label={label}>
            <button type="button" className="gha-topbar" data-outcome={outcome} onClick={() => openGitArea("actions")} aria-label={label}>
                <OutcomeIcon outcome={outcome} size={12} />
                <span className="gha-topbar-number">#{latest.runNumber}</span>
            </button>
        </Tooltip>
    );
}

/** How the branch in front is doing on this host's CI, in the space of one glyph, for the host plugin's top bar slot. */
export function hostCiGlyph(hostId: string): ComponentType<PluginTopBarProps> {
    return function HostCiGlyphFor(props: PluginTopBarProps) {
        return <HostCiGlyph {...props} hostId={hostId} />;
    };
}
