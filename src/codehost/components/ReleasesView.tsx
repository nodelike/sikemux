import { Segmented } from "../../ui/Controls";
import { useState } from "react";
import { GitColumns } from "../../git/GitColumns";
import { notify, openUrl, reportError, swallow } from "../../plugin-api/host";
import { useResourceEnabled } from "../../plugin-api/resources";
import { EmptyState, IconChevron, IconDownload, IconExternal, IconFile, IconRefresh, SkeletonRows, StaleNotice, Tooltip } from "../../plugin-api/ui";
import { hostApi, type Release, type RepoRef } from "../api";
import { errorText } from "../../lib/format";
import { useHost } from "../registry";
import { releasesR } from "../resources";
import { SectionIcon } from "./ActionsIcon";
import { ShellAccount } from "./Bits";
import { Face } from "./CommentThread";
import { useNow } from "../../plugin-api/hooks";
import { Prose } from "./Pictures";
import { formatAgo, formatBytes } from "../../lib/format";

/** How many releases the backend reads, newest first. */
const RELEASES_READ = 100;

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

/** A release as the side panel lists it: its name and tags, then its tag, who and when. */
function ReleaseRow({ release, latest, now, on, onOpen }: { release: Release; latest: boolean; now: number; on: boolean; onOpen: () => void }) {
    return (
        <button
            type="button"
            className="side-row"
            data-on={on ? "1" : "0"}
            ref={(node) => {
                if (node && on) node.scrollIntoView({ block: "nearest" });
            }}
            onClick={onOpen}>
            <SectionIcon section="releases" size={13} />
            <span className="side-row-title">{release.name}</span>
            <span className="release-row-tags">
                <Tags release={release} latest={latest} />
            </span>
            <span className="side-row-sub">
                {release.tag !== release.name ? `${release.tag} · ` : ""}
                {release.author ?? "someone"} · {formatAgo(release.publishedAt, now)}
            </span>
        </button>
    );
}

/** A release's files as GitHub lists them under its notes: name, size, downloads and when, each saved with one click. */
function Assets({ repo, release, now }: { repo: RepoRef; release: Release; now: number }) {
    const [saving, setSaving] = useState<ReadonlySet<number>>(() => new Set());
    const [open, setOpen] = useState(true);
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
    if (release.assets.length === 0) return null;
    return (
        <section className="release-assets">
            <button type="button" className="release-assets-head" aria-expanded={open} onClick={() => setOpen((was) => !was)}>
                <IconChevron size={11} className="release-assets-chev" />
                Assets
                <span className="git-group-count">{release.assets.length}</span>
            </button>
            {open && (
                <div className="release-asset-list">
                    {release.assets.map((asset) => (
                        <div className="release-asset" key={asset.id}>
                            <IconFile size={13} />
                            <button
                                type="button"
                                className="release-asset-name"
                                disabled={saving.has(asset.id)}
                                title={`Download ${asset.name}`}
                                onClick={() => save(asset.id, asset.name)}>
                                {asset.name}
                            </button>
                            {asset.downloads > 0 && (
                                <span className="release-asset-meta" title={`${asset.downloads} downloads`}>
                                    <IconDownload size={11} />
                                    {asset.downloads}
                                </span>
                            )}
                            <span className="release-asset-meta">{formatBytes(asset.sizeBytes)}</span>
                            <span className="release-asset-meta release-asset-when">
                                {saving.has(asset.id) ? "Saving…" : formatAgo(release.publishedAt, now)}
                            </span>
                        </div>
                    ))}
                </div>
            )}
        </section>
    );
}

/** One release read on the right as GitHub shows it: its header, its notes, then its files under them. */
function ReleaseRight({ repo, release, latest, now }: { repo: RepoRef; release: Release; latest: boolean; now: number }) {
    const host = useHost();
    return (
        <div className="pr-right">
            <div className="git-detail">
                <div className="pr-headline">
                    <span className="pr-face">
                        <Face login={release.author} url={release.avatarUrl ?? null} />
                    </span>
                    <span className="pr-headline-top">
                        <h2 className="git-detail-title pr-headline-title">{release.name}</h2>
                        <Tags release={release} latest={latest} />
                        <Tooltip label={`Open on ${host.name}`}>
                            <button
                                type="button"
                                className="gha-icon-btn"
                                aria-label={`Open on ${host.name}`}
                                onClick={() => void openUrl(release.url).catch(swallow(`open ${host.name}`))}>
                                <IconExternal size={12} />
                            </button>
                        </Tooltip>
                    </span>
                    <span className="pr-headline-sub">
                        <span className="pr-headline-who">
                            {release.author ?? "someone"} released this {formatAgo(release.publishedAt, now)}
                        </span>
                        <span className="pr-ref">{release.tag}</span>
                    </span>
                </div>
            </div>
            <div className="pr-conversation">
                <div className="release-page gha-reading">
                    {release.body.trim() ? <Prose>{release.body}</Prose> : <p className="gha-dim release-empty">This release has no notes.</p>}
                    <Assets repo={repo} release={release} now={now} />
                </div>
            </div>
        </div>
    );
}

type Kind = "all" | "stable" | "pre";
const KINDS: { id: Kind; label: string }[] = [
    { id: "all", label: "All" },
    { id: "stable", label: "Releases" },
    { id: "pre", label: "Pre-releases" },
];

interface Props {
    paneId: string;
    repo: RepoRef;
    active: boolean;
}

/** Releases in the side panel, newest first; the one picked, the latest to begin with, reads on the right. */
export function ReleasesView({ paneId, repo, active }: Props) {
    const host = useHost();
    const releases = useResourceEnabled(active, releasesR, repo);
    const [picked, setPicked] = useState<number | null>(null);
    const [kind, setKind] = useState<Kind>("all");
    const now = useNow(false);

    const all = releases.data ?? [];
    const latest = latestOf(all);
    const rows = all.filter((release) => kind === "all" || (kind === "pre") === (release.prerelease || release.draft));
    const shown = all.find((release) => release.id === picked) ?? all.find((release) => release.id === latest) ?? all[0] ?? null;

    const left = (
        <div className="git-side">
            <div className="git-side-head">
                <div className="git-side-title">
                    <SectionIcon section="releases" size={14} />
                    <span className="pr-side-number">Releases</span>
                    <span className="gha-page-spacer" />
                    <Tooltip label="Refresh">
                        <button type="button" className="git-foot-btn icon" onClick={() => void releases.refresh()} aria-label="Refresh releases">
                            <IconRefresh size={13} />
                        </button>
                    </Tooltip>
                    <ShellAccount />
                </div>
                <Segmented
                    fill
                    small
                    label="Which releases to list"
                    value={kind}
                    options={KINDS.map((each) => ({ value: each.id, label: each.label }))}
                    onChange={setKind}
                />
            </div>
            {releases.status === "loading" && !releases.data ? (
                <SkeletonRows rows={6} label="Loading releases" />
            ) : releases.error && !releases.data ? (
                <EmptyState
                    title="Could not read releases"
                    message={errorText(releases.error)}
                    tone="error"
                    action={{ label: "Try again", onClick: () => void releases.refresh() }}
                />
            ) : (
                <div className="git-list side-list">
                    <StaleNotice handle={releases} />
                    {rows.length === 0 ? (
                        <EmptyState icon={<SectionIcon section="releases" size={20} />} message="No releases here." />
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
                    {all.length >= RELEASES_READ && (
                        <div className="gha-pager">
                            <button
                                type="button"
                                className="gha-link"
                                onClick={() => void openUrl(all[0].url.replace(/\/tag\/.*$/u, "")).catch(swallow(`open ${host.name}`))}>
                                Older releases on {host.name}
                            </button>
                        </div>
                    )}
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
