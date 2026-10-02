import { Fragment, useEffect, useMemo, useRef, useState } from "react";
import { useModalFocus, usePluginOverlay } from "../../plugin-api/host";
import { useResource } from "../../plugin-api/resources";
import { IconPin, IconSearch, rankBy, useMouseActive } from "../../plugin-api/ui";
import type { RepoListing, RepoRef } from "../api";
import { myReposR } from "../resources";
import { formatAgo } from "../runStatus";
import { useAccount, useHost } from "../registry";
import { hostSettings, refOf, slugOf, togglePinned } from "../state";

interface Entry {
    slug: string;
    repo: RepoRef;
    /** The heading this belongs under, or nothing when it is the typed one. */
    group: string | null;
    sub: string;
}

export function entriesFor(provider: string, pinned: readonly string[], mine: readonly RepoListing[], now: number): Entry[] {
    const seen = new Set<string>();
    const entries: Entry[] = [];
    const add = (slug: string, group: string, sub: string) => {
        const repo = refOf(provider, slug);
        if (!repo || seen.has(slug)) return;
        seen.add(slug);
        entries.push({ slug, repo, group, sub });
    };
    for (const slug of pinned) add(slug, "Pinned", "pinned");
    for (const repo of mine) {
        const age = repo.pushedAt ? formatAgo(repo.pushedAt, now) : "";
        add(repo.slug, "Your repositories", [repo.private ? "private" : "", repo.archived ? "archived" : "", age].filter(Boolean).join(" · "));
    }
    return entries;
}

interface Props {
    current: RepoRef | null;
    onPick: (repo: RepoRef) => void;
    onClose: () => void;
}

export function RepoPicker({ current, onPick, onClose }: Props) {
    const modalRef = useRef<HTMLDivElement>(null);
    const inputRef = useRef<HTMLInputElement>(null);
    const listRef = useRef<HTMLDivElement>(null);
    useModalFocus(modalRef);
    usePluginOverlay(true);

    const host = useHost();
    const mine = useResource(myReposR, host.id, useAccount());
    const pinned = hostSettings(host.id).useSelect((settings) => settings.pinned);
    const [query, setQuery] = useState("");
    const [selected, setSelected] = useState(0);
    const mouseActive = useMouseActive();

    useEffect(() => {
        inputRef.current?.focus();
    }, []);

    const items = useMemo(() => {
        const typed = query.trim();
        const all = entriesFor(host.id, pinned, mine.data ?? [], Date.now());
        const ranked = rankBy(typed, all, (entry) => entry.slug, 60);
        const asSlug = refOf(host.id, typed);
        if (!asSlug || all.some((entry) => entry.slug === typed)) return ranked;
        return [{ slug: typed, repo: asSlug, group: null, sub: "open it" }, ...ranked];
    }, [host.id, pinned, mine.data, query]);

    useEffect(() => {
        listRef.current
            ?.querySelector<HTMLElement>(`.picker-item-wrap:nth-child(${selected + 1}) .picker-item`)
            ?.scrollIntoView({ block: "nearest" });
    }, [selected]);

    const activate = (entry: Entry | undefined) => {
        if (!entry) return;
        onPick(entry.repo);
        onClose();
    };

    const onKeyDown = (event: React.KeyboardEvent) => {
        if (event.key === "Escape") onClose();
        else if (event.key === "ArrowDown" || (event.key === "Tab" && !event.shiftKey)) {
            event.preventDefault();
            setSelected((at) => (items.length ? (at + 1) % items.length : 0));
        } else if (event.key === "ArrowUp" || (event.key === "Tab" && event.shiftKey)) {
            event.preventDefault();
            setSelected((at) => (items.length ? (at - 1 + items.length) % items.length : 0));
        } else if (event.key === "Enter") {
            event.preventDefault();
            activate(items[selected]);
        }
    };

    return (
        <div className="picker-backdrop" onMouseDown={onClose}>
            <div
                ref={modalRef}
                tabIndex={-1}
                className="picker gha-picker"
                role="dialog"
                aria-modal="true"
                aria-label="Choose a repository"
                onMouseDown={(event) => event.stopPropagation()}>
                <div className="picker-input-wrap">
                    <IconSearch size={15} className="picker-search-icon" />
                    <input
                        ref={inputRef}
                        className="picker-input"
                        placeholder="Search your repositories, or type owner/repo"
                        value={query}
                        onChange={(event) => {
                            setQuery(event.target.value);
                            setSelected(0);
                        }}
                        onKeyDown={onKeyDown}
                        spellCheck={false}
                        autoCapitalize="off"
                        autoCorrect="off"
                    />
                </div>

                <div className="picker-list" ref={listRef}>
                    {items.length === 0 && <div className="picker-empty">{mine.status === "loading" ? "loading…" : "no matches"}</div>}
                    {items.map((entry, index) => {
                        const heading = entry.group && entry.group !== items[index - 1]?.group ? entry.group : null;
                        return (
                            <Fragment key={entry.slug}>
                                {heading && <div className="picker-group">{heading}</div>}
                                <div
                                    className="picker-item-wrap"
                                    onMouseEnter={() => {
                                        if (mouseActive.current) setSelected(index);
                                    }}>
                                    <button
                                        type="button"
                                        className={`picker-item${index === selected ? " sel" : ""}`}
                                        onClick={() => activate(entry)}>
                                        <span className="picker-icon plugin">{host.icon(14)}</span>
                                        <span className="picker-name">{entry.slug}</span>
                                        <span className="picker-sub">{current && slugOf(current) === entry.slug ? "open" : entry.sub}</span>
                                    </button>
                                    {entry.group !== null && index === selected && (
                                        <button
                                            type="button"
                                            className="gha-pick-pin"
                                            aria-label={pinned.includes(entry.slug) ? `Unpin ${entry.slug}` : `Pin ${entry.slug}`}
                                            aria-pressed={pinned.includes(entry.slug)}
                                            title={
                                                pinned.includes(entry.slug)
                                                    ? "Stop keeping it at the top of this list"
                                                    : "Keep it at the top of this list"
                                            }
                                            onClick={() => togglePinned(host.id, entry.slug)}>
                                            <IconPin size={14} filled={pinned.includes(entry.slug)} />
                                        </button>
                                    )}
                                </div>
                            </Fragment>
                        );
                    })}
                </div>
            </div>
        </div>
    );
}
