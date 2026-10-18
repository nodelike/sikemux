import { IconCheck, IconGit, IconMerge, IconPullRequest } from "../../plugin-api/ui";
import type { Label } from "../api";
import { PULL_STATE_LABEL, pullState } from "../project";
import { CommentIcon, NotPlannedIcon, SectionIcon } from "./ActionsIcon";
import { Face } from "./CommentThread";
import { useGitShell } from "../shellContext";
import { useHost } from "../registry";
import { sectionLabel } from "../sections";
import type { Section } from "../state";

export function Labels({ labels }: { labels: readonly Label[] }) {
    if (labels.length === 0) return null;
    return (
        <span className="gha-labels">
            {labels.map((label) => (
                <span key={label.name} className="gha-label" title={label.name}>
                    {label.name}
                </span>
            ))}
        </span>
    );
}

export function Branch({ name }: { name: string }) {
    return (
        <span className="gha-branch" title={name}>
            <IconGit size={11} />
            <span>{name}</span>
        </span>
    );
}

export function Who({ login, avatarUrl }: { login: string | null; avatarUrl: string | null }) {
    return (
        <span className="gha-who">
            <Face login={login} url={avatarUrl} />
            <span className="gha-who-name">{login ?? "someone"}</span>
        </span>
    );
}

export function Comments({ count }: { count: number }) {
    if (count === 0) return null;
    return (
        <span className="gha-item-comments" title={`${count} comment${count === 1 ? "" : "s"}`}>
            <CommentIcon />
            {count}
        </span>
    );
}

const ISSUE_LABEL: Record<string, string> = { open: "Open", closed: "Closed", not_planned: "Closed as not planned" };

function toneOf(kind: "pull" | "issue", state: string, draft: boolean, reason: string | null): string {
    if (kind === "pull") return pullState({ state, draft, mergedAt: null });
    return state === "closed" && reason === "not_planned" ? "not_planned" : state;
}

export function stateLabel(kind: "pull" | "issue", state: string, draft = false, reason: string | null = null): string {
    const tone = toneOf(kind, state, draft, reason);
    return (kind === "pull" ? PULL_STATE_LABEL : ISSUE_LABEL)[tone] ?? tone;
}

export function StateMark({
    kind,
    state,
    draft = false,
    reason = null,
    size = 12,
}: {
    kind: "pull" | "issue";
    state: string;
    draft?: boolean;
    reason?: string | null;
    size?: number;
}) {
    const tone = toneOf(kind, state, draft, reason);
    const label = stateLabel(kind, state, draft, reason);
    return (
        <span className="gha-state-mark" data-kind={kind} data-state={tone} title={label} aria-label={label} role="img">
            <StateGlyph kind={kind} tone={tone} size={size} />
        </span>
    );
}

function StateGlyph({ kind, tone, size }: { kind: "pull" | "issue"; tone: string; size: number }) {
    if (kind === "pull") return tone === "merged" ? <IconMerge size={size} /> : <IconPullRequest size={size} />;
    if (tone === "not_planned") return <NotPlannedIcon size={size} />;
    return tone === "closed" ? <IconCheck size={size} /> : <SectionIcon section="issues" size={size} />;
}

/** Who is signed in to the host, lent by the git pane's shell to whichever head a screen draws first. */
export function ShellAccount() {
    return <>{useGitShell().account}</>;
}

/** A full list's name with its tree icon, at the start of its head, since the screen has no title bar of its own. */
export function ListTitle({ section }: { section: Section }) {
    const host = useHost();
    return (
        <h2 className="gha-list-title">
            <SectionIcon section={section} size={14} />
            {sectionLabel(host, section)}
        </h2>
    );
}
