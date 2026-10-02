import { useEffect, useMemo, useRef } from "react";
import { invalidate, useResourceEnabled } from "../../plugin-api/resources";
import { EmptyState, SkeletonRows } from "../../plugin-api/ui";
import { failureMessage } from "../api";
import { LocalRepoProvider } from "../localRepo";
import { useHost } from "../registry";
import { hostStatusR, workflowsR } from "../resources";
import { hostSettings, resetView, setProjectAccount, slugOf, updateView, useHostView, type Section } from "../state";
import type { RepoRef } from "../types";
import { DispatchDialog } from "./DispatchDialog";
import { InboxView } from "./InboxView";
import { IssuesView } from "./IssuesView";
import { PullsView } from "./PullsView";
import { RateLimitBanner } from "./RateLimitBanner";
import { ReleasesView } from "./ReleasesView";
import { RunsList } from "./RunsList";
import { RunView } from "./RunView";
import "../codehost.css";

interface Props {
    paneId: string;
    section: Section;
    repo: RepoRef;
    /** The branch checked out in the git pane. */
    branch: string | null;
    /** The project folder, when the repository shown is its own. */
    cwd: string | null;
    active: boolean;
}

/** One of the code host's sections, drawn in the git pane in place of the local workbench. */
export function HostArea({ paneId, section, repo, branch, cwd, active }: Props) {
    const host = useHost();
    const view = useHostView(paneId);
    const status = useResourceEnabled(active, hostStatusR, host.id, repo.account ?? null);
    const followBranch = hostSettings(host.id).useSelect((settings) => settings.followBranch);
    const signedIn = !!status.data?.ok;

    // Whatever was open belongs to the repository it was opened in.
    const slug = `${repo.provider}:${slugOf(repo)}`;
    const shown = useRef(slug);
    useEffect(() => {
        if (shown.current === slug) return;
        shown.current = slug;
        resetView(paneId);
    }, [paneId, slug]);

    const workflows = useResourceEnabled(active && signedIn && view.dispatching !== null, workflowsR, repo);
    const dispatching = useMemo(
        () => (view.dispatching === null ? null : (workflows.data ?? []).find((workflow) => workflow.id === view.dispatching)),
        [view.dispatching, workflows.data],
    );

    if (status.status === "loading" && !status.data) {
        return (
            <div className="gha-pane" data-active={active ? "1" : "0"}>
                <SkeletonRows rows={8} label={`Connecting to ${host.name}`} />
            </div>
        );
    }
    if (status.error && !status.data) {
        return (
            <div className="gha-pane" data-active={active ? "1" : "0"}>
                <EmptyState
                    title={`Could not reach ${host.name}`}
                    message={failureMessage(status.error)}
                    tone="error"
                    action={{ label: "Try again", onClick: () => void status.refresh() }}
                />
            </div>
        );
    }
    if (!signedIn) {
        return (
            <div className="gha-pane" data-active={active ? "1" : "0"}>
                <host.SignIn
                    onSignedIn={(account) => {
                        if (account && cwd) setProjectAccount(host.id, cwd, account);
                        invalidate((kind) => kind.startsWith("host."));
                    }}
                />
            </div>
        );
    }

    const canWrite = !!status.data?.canWriteCi;
    const runBranch = followBranch && !view.branch ? branch : view.branch;
    return (
        <LocalRepoProvider value={cwd}>
            <div className="gha-pane" data-active={active ? "1" : "0"}>
                <RateLimitBanner active={active} />
                <div className="gha-body">
                    {section === "inbox" ? (
                        <InboxView paneId={paneId} login={status.data?.login ?? null} active={active} />
                    ) : section === "pulls" ? (
                        <PullsView
                            paneId={paneId}
                            repo={repo}
                            listState={view.pullState}
                            item={view.item}
                            composing={view.composing === "pull"}
                            projectBranch={branch}
                            cwd={cwd}
                            login={status.data?.login ?? null}
                            active={active}
                        />
                    ) : section === "issues" ? (
                        <IssuesView
                            paneId={paneId}
                            repo={repo}
                            listState={view.issueState}
                            item={view.item}
                            composing={view.composing === "issue"}
                            page={view.page}
                            cwd={cwd}
                            active={active}
                        />
                    ) : section === "releases" ? (
                        <ReleasesView paneId={paneId} repo={repo} active={active} />
                    ) : view.run === null ? (
                        <RunsList
                            paneId={paneId}
                            repo={repo}
                            view={view}
                            branch={runBranch}
                            projectBranch={branch}
                            active={active}
                            canWrite={canWrite}
                            onDispatch={(workflowId) => updateView(paneId, { dispatching: workflowId })}
                        />
                    ) : (
                        <RunView paneId={paneId} repo={repo} runId={view.run} openJob={view.job} active={active} canWrite={canWrite} />
                    )}
                </div>
                {dispatching && (
                    <DispatchDialog
                        repo={repo}
                        workflow={dispatching}
                        defaultBranch={runBranch ?? branch}
                        onClose={() => updateView(paneId, { dispatching: null })}
                    />
                )}
            </div>
        </LocalRepoProvider>
    );
}
