import { useState } from "react";
import { GitColumns } from "../../git/GitColumns";
import { notify, openUrl, reportError, swallow } from "../../plugin-api/host";
import { useResourceEnabled } from "../../plugin-api/resources";
import { EmptyState, IconExternal, IconGit, IconRefresh, SkeletonRows, Tooltip } from "../../plugin-api/ui";
import { hostApi, failureMessage, type Release, type RepoRef } from "../api";
import { useHost } from "../registry";
import { releasesR } from "../resources";
import { formatAgo } from "../runStatus";
import { SectionIcon } from "./ActionsIcon";
import { formatBytes } from "./Artifacts";
import { Who } from "./Bits";
import { useNow } from "./hooks";
import { Prose } from "./Pictures";

/** How many releases the backend reads, newest first. */
const RELEASES_READ = 100;

type Tab = "notes" | "assets";

/** The newest release that is neither a draft nor a pre-release, which is the one GitHub marks as latest. */
export function latestOf(releases: readonly Release[]): number | null {
    return releases.find((release) => !release.draft && !release.prerelease)?.id ?? null;
}

function Tags({ release, latest }: { release: Release; latest: boolean }) {
    return (
        <>
            {latest && (
                <span className="gha-tag" data-tone="latest">
                    Latest
                </span>
            )}
            {release.draft && (
                <span className="gha-tag" data-tone="quiet">
                    Draft
                </span>
            )}
            {release.prerelease && (
                <span className="gha-tag" data-tone="quiet">
                    Pre-release
                </span>
            )}
        </>
    );
}

function ReleaseRow({ release, latest, now, on, onOpen }: { release: Release; latest: boolean; now: number; on: boolean; onOpen: () => void }) {
    return (
        <button type="button" className="gha-item-row" data-on={on ? "1" : "0"} onClick={onOpen}>
            <SectionIcon section="releases" size={13} />
            <span className="gha-item-head">
                <span className="gha-item-title">{release.name}</span>
                <Tags release={release} latest={latest} />
            </span>
            <span className="gha-item-sub">
                {release.tag !== release.name && <span className="gha-mono">{release.tag}</span>}
                {release.author && <Who login={release.author} avatarUrl={null} />}
            </span>
            <span className="gha-item-when">{formatAgo(release.publishedAt, now)}</span>
        </button>
    );
}

function Assets({ repo, release }: { repo: RepoRef; release: Release }) {
    const [saving, setSaving] = useState<ReadonlySet<number>>(() => new Set());
    const save = (id: number, name: string) => {
        if (saving.has(id)) return;
        setSaving((was) => new Set(was).add(id));
        void hostApi(repo.provider)
            .downloadAsset(repo, id, name)
            .then((saved) => notify("success", `Saved ${name} to ${saved.path}`))
            .catch(reportError(`Could not download ${name}`))
            .finally(() =>
                setSaving((was) => {
                    const next = new Set(was);
                    next.delete(id);
                    return next;
                }),
            );
    };
    if (release.assets.length === 0) return <EmptyState message="This release has no files." />;
    return (
        <div className="gha-artifacts release-assets">
            {release.assets.map((asset) => (
                <div className="gha-artifact" key={asset.id}>
                    <span className="gha-artifact-name">{asset.name}</span>
                    <span className="gha-dim">{formatBytes(asset.sizeBytes)}</span>
                    <span className="gha-dim">{asset.downloads} downloads</span>
                    <button type="button" className="gha-link" disabled={saving.has(asset.id)} onClick={() => save(asset.id, asset.name)}>
                        {saving.has(asset.id) ? "Saving…" : "Download"}
                    </button>
                </div>
            ))}
        </div>
    );
}

function ReleaseRight({ repo, release, latest, now }: { repo: RepoRef; release: Release; latest: boolean; now: number }) {
    const host = useHost();
    const [tab, setTab] = useState<Tab>("notes");
    const tabs: { id: Tab; label: string; count: number }[] = [
        { id: "notes", label: "Notes", count: 0 },
        { id: "assets", label: "Assets", count: release.assets.length },
    ];
    return (
        <div className="pr-right">
            <div className="git-detail">
                <div className="pr-title-row">
                    <h2 className="git-detail-title">{release.name}</h2>
                    <Tooltip label={`Open on ${host.name}`}>
                        <button
                            type="button"
                            className="gha-icon-btn"
                            aria-label={`Open on ${host.name}`}
                            onClick={() => void openUrl(release.url).catch(swallow(`open ${host.name}`))}>
                            <IconExternal size={12} />
                        </button>
                    </Tooltip>
                </div>
                <div className="git-detail-meta">
                    {release.author && <Who login={release.author} avatarUrl={null} />}
                    <span>released {formatAgo(release.publishedAt, now)}</span>
                    <span className="gha-branch">
                        <IconGit size={11} />
                        <span>{release.tag}</span>
                    </span>
                    <Tags release={release} latest={latest} />
                </div>
                <div className="git-detail-actions" role="tablist" aria-label="Release">
                    {tabs.map((each) => (
                        <button
                            key={each.id}
                            type="button"
                            role="tab"
                            aria-selected={tab === each.id}
                            className="gha-chip pr-tab"
                            data-on={tab === each.id ? "1" : "0"}
                            onClick={() => setTab(each.id)}>
                            {each.label}
                            {each.count > 0 && <span className="gha-tab-count">{each.count}</span>}
                        </button>
                    ))}
                </div>
            </div>
            <div className="pr-conversation">
                {tab === "assets" ? (
                    <Assets repo={repo} release={release} />
                ) : release.body.trim() ? (
                    <Prose className="prose release-notes">{release.body}</Prose>
                ) : (
                    <EmptyState message="This release has no notes." />
                )}
            </div>
        </div>
    );
}

interface Props {
    paneId: string;
    repo: RepoRef;
    active: boolean;
}

/** Releases down the left, newest first; the one picked, the latest to begin with, reads on the right. */
export function ReleasesView({ paneId, repo, active }: Props) {
    const host = useHost();
    const releases = useResourceEnabled(active, releasesR, repo);
    const [picked, setPicked] = useState<number | null>(null);
    const now = useNow(false);

    const rows = releases.data ?? [];
    const latest = latestOf(rows);
    const shown = rows.find((release) => release.id === picked) ?? rows.find((release) => release.id === latest) ?? rows[0] ?? null;

    const left =
        releases.status === "loading" && !releases.data ? (
            <SkeletonRows rows={6} label="Loading releases" />
        ) : releases.error ? (
            <EmptyState
                title="Could not read releases"
                message={failureMessage(releases.error)}
                tone="error"
                action={{ label: "Try again", onClick: () => void releases.refresh() }}
            />
        ) : (
            <div className="gha-list pr-list">
                <div className="gha-list-head">
                    <span className="gha-dim">
                        {rows.length === RELEASES_READ ? `Newest ${RELEASES_READ}` : `${rows.length} release${rows.length === 1 ? "" : "s"}`}
                    </span>
                    <span className="gha-page-spacer" />
                    <Tooltip label="Refresh">
                        <button type="button" className="gha-icon-btn" onClick={() => void releases.refresh()} aria-label="Refresh releases">
                            <IconRefresh size={13} />
                        </button>
                    </Tooltip>
                </div>
                {rows.length === 0 ? (
                    <EmptyState icon={<SectionIcon section="releases" size={20} />} message="This repository has no releases." />
                ) : (
                    rows.map((release) => (
                        <ReleaseRow
                            key={release.id}
                            release={release}
                            latest={release.id === latest}
                            now={now}
                            on={release.id === shown?.id}
                            onOpen={() => setPicked(release.id)}
                        />
                    ))
                )}
                {rows.length >= RELEASES_READ && (
                    <div className="gha-pager">
                        <button
                            type="button"
                            className="gha-link"
                            onClick={() => void openUrl(rows[0].url.replace(/\/tag\/.*$/u, "")).catch(swallow(`open ${host.name}`))}>
                            Older releases on {host.name}
                        </button>
                    </div>
                )}
            </div>
        );

    const right = shown ? (
        <ReleaseRight key={shown.id} repo={repo} release={shown} latest={shown.id === latest} now={now} />
    ) : (
        <EmptyState icon={<SectionIcon section="releases" size={20} />} message="Nothing released yet." />
    );

    return <GitColumns paneId={paneId} left={left} right={<div className="git-right-review">{right}</div>} />;
}
