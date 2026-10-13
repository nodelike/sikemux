import type { PluginTopBarProps } from "../../../plugin-api";
import { currentBranch, gitOverviewR } from "../../../plugin-api/host";
import { useResourceEnabled } from "../../../plugin-api/resources";
import { jiraKeysR, jiraSearchR, jiraStatusR } from "../resources";
import { openJiraIssue } from "../state";
import { JiraMark } from "./JiraMark";
import { StatusChip } from "./StatusChip";

/** The issue the project's branch is named after, e.g. `abc-123-fix-x`, beside the branch in the top bar. */
export function JiraTopBarItem({ projectCwd }: PluginTopBarProps) {
    const status = useResourceEnabled(true, jiraStatusR);
    const signedIn = !!status.data?.configured && !status.data.authFailed;
    const repo = useResourceEnabled(signedIn && !!projectCwd, gitOverviewR, projectCwd ?? "");
    const branch = currentBranch(repo.data?.status);
    const keys = useResourceEnabled(signedIn && !!branch, jiraKeysR, branch ?? "");
    const key = keys.data?.[0] ?? "";
    const issue = useResourceEnabled(!!key, jiraSearchR, `key = ${key}`, "");
    const found = issue.data?.issues[0];
    if (!key || !found) return null;
    return (
        <>
            <span className="tb-sep" />
            <button
                type="button"
                className="jira-topbar"
                data-no-window-drag
                title={`${found.key}: ${found.summary}`}
                onClick={() => openJiraIssue(found.key)}>
                <JiraMark size={12} />
                <span className="jira-key">{found.key}</span>
                <StatusChip status={found.status} category={found.statusCategory} />
            </button>
        </>
    );
}
