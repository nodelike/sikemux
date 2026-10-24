import { lazy, Suspense, useEffect, useMemo, useState, type ReactNode } from "react";
import { AuthorPicturesProvider, type AuthorPictures } from "../../git/AuthorAvatar";
import { invalidate, useCachedResourceEnabled } from "../../plugin-api/resources";
import { subscribe } from "../../state/bus";
import { SkeletonRows } from "../../plugin-api/ui";
import type { GitArea } from "../../state/types";
import type { RepoRef } from "../types";
import { useHostRepo } from "../project";
import { commitAuthorsR, hostStatusR } from "../resources";
import { AccountProvider, codeHost, HostProvider, type CodeHost } from "../registry";
import { isTreeSection, sameRepo, setProjectAccount, setProjectRepo, slugOf } from "../state";
import { HostAccountButton } from "./HostRail";
import { sectionsOf } from "../sections";
import { useStore } from "../../state/store";
import { ShellFrameContext } from "../../git/GitColumns";
import { PaneField } from "../../ui/ShaderField";
import { GitShellContext } from "../shellContext";
import { preferencesOf } from "../../state/preferences";

const HostArea = lazy(() => import("./HostArea").then((module) => ({ default: module.HostArea })));
const RepoPicker = lazy(() => import("./RepoPicker").then((module) => ({ default: module.RepoPicker })));

interface Props {
    paneId: string;
    /** The repository folder the git pane is on. */
    cwd: string;
    area: GitArea;
    active: boolean;
    /** Whether the pane is on screen and settled, so its host data may be read. */
    fetching: boolean;
    onArea: (area: GitArea) => void;
    /** The local workbench, shown unless the project tree opened one of the host's screens. */
    children: ReactNode;
}

/**
 * The git pane's local workbench, or the host screen the project tree opened in its place under a header of its own.
 * The workbench borrows the host's account, inbox and repository picker for its side panel.
 */
export function GitHostShell({ paneId, cwd, area, active, fetching, onArea, children }: Props) {
    const found = useHostRepo(cwd, fetching);
    const host = found.repo ? codeHost(found.repo.provider) : undefined;
    const [picking, setPicking] = useState(false);
    const [adding, setAdding] = useState(false);
    const paneShader = useStore((s) => preferencesOf(s).paneShader);
    const hosted = !!host;
    const status = useCachedResourceEnabled(fetching && hosted, hostStatusR, found.repo?.provider ?? "", found.repo?.account ?? null);
    const signedIn = !!status.data?.ok;
    const pictures = useAuthorPictures(host, found.repo, fetching && signedIn);

    // A push, pull or commit changes what the host has to say about this repository's branches and pull requests.
    useEffect(() => {
        if (!hosted) return;
        return subscribe("git-refresh", (event) => {
            if (event.repo !== cwd) return;
            invalidate((kind) => kind === "host.timeline" || kind === "host.pullCommits" || kind === "host.pullFiles");
        });
    }, [cwd, hosted]);

    const repo = host ? found.repo : null;
    const added = (account: string | null) => {
        if (host && account) setProjectAccount(host.id, cwd, account);
        setAdding(false);
        invalidate((kind) => kind.startsWith("host."));
    };

    // The project tree opens these as screens of their own, so the git screens stay out of their header.
    const standalone = host && repo && isTreeSection(area) ? area : null;
    // A host screen in two columns frames its right column itself, so the whole screen is framed only while none is.
    const [columns, setColumns] = useState(0);
    const shellFrame = useMemo(
        () => ({
            claim: () => {
                setColumns((count) => count + 1);
                return () => setColumns((count) => count - 1);
            },
        }),
        [],
    );
    const framed = !!standalone && columns === 0;
    const account =
        host && repo ? (
            <HostAccountButton
                slug={slugOf(repo)}
                cwd={cwd}
                active={active}
                onSignIn={standalone ? null : () => onArea("pulls")}
                onPickRepo={() => setPicking(true)}
                onAddAccount={() => setAdding(true)}
            />
        ) : null;
    const lent = {
        pickRepo: host && repo ? () => setPicking(true) : null,
        account,
        inbox: !!host && !!repo && signedIn && sectionsOf(host).includes("inbox"),
    };

    return (
        <HostProvider value={host && repo ? host : null}>
            <AccountProvider value={repo?.account ?? null}>
                <div className="git-shell">
                    <div className={`git-shell-main${framed ? " git-framed" : ""}`}>
                        {framed && <PaneField enabled={paneShader} />}
                        {host && repo && adding ? (
                            <div className="gha-pane" data-active={active ? "1" : "0"}>
                                <div className="gha-callout gha-add-account">
                                    <span>Add another {host.name} account. This project switches to it once it signs in.</span>
                                    <button type="button" className="gha-link" onClick={() => setAdding(false)}>
                                        Cancel
                                    </button>
                                </div>
                                <host.SignIn onSignedIn={added} />
                            </div>
                        ) : standalone && host && repo ? (
                            <ShellFrameContext.Provider value={shellFrame}>
                                <GitShellContext.Provider value={lent}>
                                    <Suspense fallback={<SkeletonRows rows={8} label={`Loading ${host.name}`} />}>
                                        <HostArea
                                            paneId={paneId}
                                            section={standalone}
                                            repo={repo}
                                            branch={found.branch}
                                            project={cwd}
                                            cwd={sameRepo(found.remote, repo) ? cwd : null}
                                            active={active}
                                        />
                                    </Suspense>
                                </GitShellContext.Provider>
                            </ShellFrameContext.Provider>
                        ) : (
                            <GitShellContext.Provider value={lent}>
                                <AuthorPicturesProvider value={pictures}>{children}</AuthorPicturesProvider>
                            </GitShellContext.Provider>
                        )}
                    </div>
                </div>
                {host && repo && picking && (
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

/** The host's pictures for the emails in local commits, read while `signedIn` says someone is signed in to it. */
function useAuthorPictures(host: CodeHost | undefined, repo: RepoRef | null, signedIn: boolean): AuthorPictures | null {
    const provider = repo?.provider ?? "";
    const authors = useCachedResourceEnabled(signedIn && !!host?.api.commitAuthors, commitAuthorsR, repo ?? { provider, owner: "", name: "" }, null);
    return useMemo(() => {
        if (!host || !signedIn) return null;
        const byEmail = new Map((authors.data ?? []).map((author) => [author.email.toLowerCase(), author.avatarUrl]));
        return {
            pictureFor: (email) => host.avatarForEmail?.(email) ?? byEmail.get(email.toLowerCase()) ?? null,
            load: (url) => host.api.image(url),
        };
    }, [host, signedIn, authors.data]);
}
