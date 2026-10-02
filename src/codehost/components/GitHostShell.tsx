import { lazy, Suspense, useEffect, useMemo, useState, type ReactNode } from "react";
import { AuthorPicturesProvider, type AuthorPictures } from "../../git/AuthorAvatar";
import { invalidate, useResourceEnabled } from "../../plugin-api/resources";
import { subscribe } from "../../state/bus";
import { SkeletonRows } from "../../plugin-api/ui";
import type { GitArea } from "../../state/types";
import type { RepoRef } from "../types";
import { useHostRepo } from "../project";
import { commitAuthorsR, hostStatusR } from "../resources";
import { AccountProvider, codeHost, HostProvider, type CodeHost } from "../registry";
import { sameRepo, setProjectAccount, setProjectRepo, slugOf } from "../state";
import { GitRail, HostRailItems, type RailItem } from "./HostRail";

const HostArea = lazy(() => import("./HostArea").then((module) => ({ default: module.HostArea })));
const RepoPicker = lazy(() => import("./RepoPicker").then((module) => ({ default: module.RepoPicker })));

interface Props {
    paneId: string;
    /** The repository folder the git pane is on. */
    cwd: string;
    area: GitArea;
    active: boolean;
    onArea: (area: GitArea) => void;
    /** The local workbench's screens, which head the rail. */
    local: readonly RailItem[];
    /** The local workbench, shown while the area is `local` or the folder is on no known host. */
    children: ReactNode;
}

/**
 * The git pane's rail and whatever it has open. The code host the folder's remote lives on adds its sections to the
 * rail; a folder on no known host, or on one whose plugin is switched off, gets the local screens alone.
 */
export function GitHostShell({ paneId, cwd, area, active, onArea, local, children }: Props) {
    const found = useHostRepo(cwd, active);
    const host = found.repo ? codeHost(found.repo.provider) : undefined;
    const [picking, setPicking] = useState(false);
    const [adding, setAdding] = useState(false);
    const hosted = !!host;
    const pictures = useAuthorPictures(host, found.repo, active);

    // A push, pull or commit changes what the host has to say about this repository's branches and pull requests.
    useEffect(() => {
        if (!hosted) return;
        return subscribe("git-refresh", (event) => {
            if (event.repo !== cwd) return;
            invalidate(
                (kind) =>
                    kind === "host.runs" ||
                    kind === "host.pulls" ||
                    kind === "host.pull" ||
                    kind === "host.timeline" ||
                    kind === "host.pullCommits" ||
                    kind === "host.pullFiles",
            );
        });
    }, [cwd, hosted]);

    if (!host || !found.repo) {
        return (
            <div className="git-shell">
                <GitRail local={local} host={null} />
                <div className="git-shell-main">{children}</div>
            </div>
        );
    }
    const repo = found.repo;
    const added = (account: string | null) => {
        if (account) setProjectAccount(host.id, cwd, account);
        setAdding(false);
        invalidate((kind) => kind.startsWith("host."));
    };

    return (
        <HostProvider value={host}>
            <AccountProvider value={repo.account ?? null}>
                <div className="git-shell">
                    <GitRail
                        local={local}
                        host={
                            <HostRailItems
                                area={area}
                                slug={slugOf(repo)}
                                cwd={cwd}
                                active={active}
                                onArea={onArea}
                                onPickRepo={() => setPicking(true)}
                                onAddAccount={() => setAdding(true)}
                            />
                        }
                    />
                    <div className="git-shell-main">
                        {adding ? (
                            <div className="gha-pane" data-active={active ? "1" : "0"}>
                                <div className="gha-callout gha-add-account">
                                    <span>Add another {host.name} account. This project switches to it once it signs in.</span>
                                    <button type="button" className="gha-link" onClick={() => setAdding(false)}>
                                        Cancel
                                    </button>
                                </div>
                                <host.SignIn onSignedIn={added} />
                            </div>
                        ) : area === "local" ? (
                            <AuthorPicturesProvider value={pictures}>{children}</AuthorPicturesProvider>
                        ) : (
                            <Suspense fallback={<SkeletonRows rows={8} label={`Loading ${host.name}`} />}>
                                <HostArea
                                    paneId={paneId}
                                    section={area}
                                    repo={repo}
                                    branch={found.branch}
                                    cwd={sameRepo(found.remote, repo) ? cwd : null}
                                    active={active}
                                />
                            </Suspense>
                        )}
                    </div>
                </div>
                {picking && (
                    <Suspense fallback={null}>
                        <RepoPicker
                            current={repo}
                            onClose={() => setPicking(false)}
                            onPick={(picked) =>
                                setProjectRepo(host.id, cwd, found.remote && slugOf(found.remote) === slugOf(picked) ? null : slugOf(picked))
                            }
                        />
                    </Suspense>
                )}
            </AccountProvider>
        </HostProvider>
    );
}

/** The host's pictures for the emails in local commits, once someone is signed in to it. */
function useAuthorPictures(host: CodeHost | undefined, repo: RepoRef | null, active: boolean): AuthorPictures | null {
    const provider = repo?.provider ?? "";
    const status = useResourceEnabled(active && !!host, hostStatusR, provider, repo?.account ?? null);
    const signedIn = !!status.data?.ok;
    const authors = useResourceEnabled(
        active && signedIn && !!host?.api.commitAuthors,
        commitAuthorsR,
        repo ?? { provider, owner: "", name: "" },
        null,
    );
    return useMemo(() => {
        if (!host || !signedIn) return null;
        const byEmail = new Map((authors.data ?? []).map((author) => [author.email.toLowerCase(), author.avatarUrl]));
        return {
            pictureFor: (email) => host.avatarForEmail?.(email) ?? byEmail.get(email.toLowerCase()) ?? null,
            load: (url) => host.api.image(url),
        };
    }, [host, signedIn, authors.data]);
}
