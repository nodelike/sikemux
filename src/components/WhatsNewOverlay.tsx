import { useEffect, useState } from "react";
import Markdown, { type Components } from "react-markdown";
import { getVersion } from "@tauri-apps/api/app";
import { installPendingUpdate, isUpdateBusy, updateStatusLabel } from "../api/updater";
import { openInBrowser, releasesApi, type ReleaseContributor, type ReleaseNotes } from "../api/releases";
import * as cmd from "../state/commands";
import { useStore } from "../state/store";
import { errMessage, swallow } from "../state/toast";
import { useOccludeNativeViews } from "../state/nativeViews";
import { ExperienceBackdrop } from "./ExperienceOverlays";
import { ShaderField } from "./ShaderField";

const FEATURED = 3;
const WALL = 13;

const fetchedNotes = new Map<string, Promise<ReleaseNotes>>();
const fetchedAvatars = new Map<string, string>();

export function resetReleaseCachesForTests(): void {
    fetchedNotes.clear();
    fetchedAvatars.clear();
}

function notesFor(version: string): Promise<ReleaseNotes> {
    let notes = fetchedNotes.get(version);
    if (!notes) {
        notes = releasesApi.notes(version);
        notes.catch(() => fetchedNotes.delete(version));
        fetchedNotes.set(version, notes);
    }
    return notes;
}

/** The update waiting to install, else the build that is running. Notes the app already holds show until GitHub answers. */
function useShownRelease(open: boolean) {
    const pending = useStore((s) => s.pendingUpdate);
    const installed = useStore((s) => s.lastReleaseNotes);
    const [running, setRunning] = useState("");
    const [fetched, setFetched] = useState<ReleaseNotes | null>(null);
    const [error, setError] = useState("");
    useEffect(() => {
        if (open) void getVersion().then(setRunning);
    }, [open]);
    const version = pending?.version ?? running;
    useEffect(() => {
        if (!open || !version) return;
        let live = true;
        setError("");
        notesFor(version)
            .then((notes) => live && setFetched(notes))
            .catch((value) => live && setError(errMessage(value)));
        return () => {
            live = false;
        };
    }, [open, version]);
    const held = pending ?? (installed?.version === version ? installed : null);
    const release: ReleaseNotes | null =
        fetched?.version === version
            ? fetched
            : held && { version, notes: held.notes, date: held.date, commits: null, compare: null, contributors: [] };
    return { release, version, error };
}

function useAvatars(contributors: readonly ReleaseContributor[]): ReadonlyMap<string, string> {
    const [, setLoaded] = useState(0);
    const key = contributors.map((person) => person.avatar).join(" ");
    useEffect(() => {
        const missing = contributors.map((person) => person.avatar).filter((url) => !fetchedAvatars.has(url));
        if (missing.length === 0) return;
        let live = true;
        releasesApi
            .avatars(missing)
            .then((found) => {
                for (const [url, data] of Object.entries(found)) fetchedAvatars.set(url, data);
                if (live) setLoaded((count) => count + 1);
            })
            .catch(swallow("load contributor avatars"));
        return () => {
            live = false;
        };
        // eslint-disable-next-line react-hooks/exhaustive-deps -- the joined addresses stand in for the list
    }, [key]);
    return fetchedAvatars;
}

function releaseDate(date: string | null): string | null {
    if (!date) return null;
    const parsed = new Date(date);
    if (Number.isNaN(parsed.getTime())) return null;
    return parsed.toLocaleDateString(undefined, { day: "numeric", month: "short", year: "numeric" });
}

/** `0.4.0-nightly.10` is too long to set large, so the build rides under the release. */
function splitVersion(version: string): [string, string] {
    const dash = version.indexOf("-");
    return dash < 0 ? [version, ""] : [version.slice(0, dash), version.slice(dash + 1)];
}

function highlightCount(notes: string | null): number {
    return notes?.match(/^[-*] /gm)?.length ?? 0;
}

const plural = (count: number, word: string) => `${count} ${word}${count === 1 ? "" : "s"}`;
const profile = (login: string) => `https://github.com/${login}`;
const openLink = (url: string) => void openInBrowser(url).catch(swallow("open a release link"));

function Avatar({ person, src }: { person: ReleaseContributor; src: string | undefined }) {
    return src ? (
        <img className="wn-avatar" src={src} alt="" />
    ) : (
        <span className="wn-avatar" aria-hidden="true">
            {(person.name || person.login).charAt(0).toUpperCase()}
        </span>
    );
}

function Contributors({ people }: { people: readonly ReleaseContributor[] }) {
    const avatars = useAvatars(people);
    const [everyone, setEveryone] = useState(false);
    const featured = people.slice(0, FEATURED);
    const rest = people.slice(FEATURED);
    const shown = everyone ? rest : rest.slice(0, WALL);
    return (
        <section className="wn-people" aria-label="Contributors">
            <span className="wn-label">Contributors</span>
            {featured.map((person) => (
                <button key={person.login} type="button" className="wn-person" onClick={() => openLink(profile(person.login))}>
                    <Avatar person={person} src={avatars.get(person.avatar)} />
                    <span>
                        <b>{person.name}</b>
                        <small>
                            @{person.login} · {plural(person.commits, "commit")}
                        </small>
                    </span>
                </button>
            ))}
            {rest.length > 0 && (
                <>
                    <span className="wn-more-label">and {rest.length} more</span>
                    <div className="wn-wall">
                        {shown.map((person) => (
                            <button
                                key={person.login}
                                type="button"
                                title={`@${person.login} · ${plural(person.commits, "commit")}`}
                                aria-label={`@${person.login}`}
                                onClick={() => openLink(profile(person.login))}>
                                <Avatar person={person} src={avatars.get(person.avatar)} />
                            </button>
                        ))}
                        {shown.length < rest.length && (
                            <button type="button" className="wn-rest" onClick={() => setEveryone(true)}>
                                +{rest.length - shown.length}
                            </button>
                        )}
                    </div>
                </>
            )}
        </section>
    );
}

const markdown: Components = {
    h1: () => null,
    a: ({ href, children }) => (
        <a
            href={href}
            onClick={(event) => {
                event.preventDefault();
                if (href) openLink(href);
            }}>
            {children}
        </a>
    ),
};

export function WhatsNewOverlay() {
    const open = useStore((s) => s.whatsNewOpen);
    useOccludeNativeViews(open);
    const pending = useStore((s) => s.pendingUpdate);
    const { release, version, error } = useShownRelease(open);
    if (!open) return null;
    const [core, build] = splitVersion(version);
    const date = releaseDate(release?.date ?? null);
    const highlights = highlightCount(release?.notes ?? null);
    const people = release?.contributors ?? [];
    const updateBusy = pending ? isUpdateBusy(pending.state) : false;
    return (
        <ExperienceBackdrop label="What’s new" className="whats-new" onClose={cmd.closeWhatsNew}>
            <aside className="wn-side">
                <ShaderField preset="release" className="wn-sky" />
                <span className="wn-channel">{pending ? "update ready" : version.includes("-") ? "nightly" : "stable"}</span>
                <h1 className="wn-version">
                    <span>v</span>
                    {core || "…"}
                    {build && <small>{build}</small>}
                </h1>
                {date && <span className="wn-date">{date}</span>}
                <dl className="wn-stats">
                    {release?.commits != null && (
                        <div>
                            <dt>commits</dt>
                            <dd>{release.commits}</dd>
                        </div>
                    )}
                    {people.length > 0 && (
                        <div>
                            <dt>people</dt>
                            <dd>{people.length}</dd>
                        </div>
                    )}
                    {highlights > 0 && (
                        <div>
                            <dt>highlights</dt>
                            <dd>{highlights}</dd>
                        </div>
                    )}
                </dl>
                {people.length > 0 && <Contributors people={people} />}
            </aside>
            <div className="wn-main">
                <header>
                    <h2>What’s new</h2>
                    <button type="button" className="wn-esc" onClick={cmd.closeWhatsNew} aria-label="Close What’s new">
                        esc
                    </button>
                </header>
                <div className="wn-notes">
                    {release?.notes ? (
                        <Markdown skipHtml components={markdown}>
                            {release.notes}
                        </Markdown>
                    ) : error ? (
                        <p className="wn-status">
                            The notes for v{version} could not be loaded. {error}
                        </p>
                    ) : (
                        <p className="wn-status">{release ? `v${version} shipped without notes.` : `Loading the notes for v${version}…`}</p>
                    )}
                </div>
                <footer>
                    {release?.compare ? (
                        <a
                            className="wn-compare"
                            href={release.compare}
                            onClick={(event) => {
                                event.preventDefault();
                                openLink(release.compare!);
                            }}>
                            {release.compare.split("/compare/")[1]?.replace("...", "…") ?? "Compare"} ↗
                        </a>
                    ) : (
                        <span />
                    )}
                    {pending && (
                        <button type="button" className="primary" disabled={updateBusy} onClick={() => void installPendingUpdate()}>
                            {updateBusy
                                ? updateStatusLabel(pending)
                                : pending.state === "error"
                                  ? `Retry v${pending.version}`
                                  : `Install v${pending.version}`}
                        </button>
                    )}
                </footer>
            </div>
        </ExperienceBackdrop>
    );
}
