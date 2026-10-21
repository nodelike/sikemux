import { useCallback, useRef, useState, type ComponentType, type MouseEvent, type ReactNode } from "react";
import { gitOverviewR, notify, reportError, sendToBoard, swallow, useActiveProjectCwd } from "../../../plugin-api/host";
import { useResourceEnabled } from "../../../plugin-api/resources";
import {
    ContextMenu,
    Dropdown,
    EmptyState,
    IconAgent,
    IconBoard,
    IconChevron,
    IconClock,
    IconClose,
    IconCopy,
    IconExternal,
    IconEye,
    IconInbox,
    IconLogOut,
    IconPencil,
    IconSearch,
    IconStack,
    IconStar,
    IconTimer,
    IconUser,
    IconUserMinus,
    Loaded,
    Markdown,
    Pending,
    ResizeHandle,
    SignedIn,
    StaleNotice,
    type ContextMenuItem,
} from "../../../plugin-api/ui";
import { errorText } from "../../../plugin-api/format";
import { jiraApi, type JiraBoard as JiraBoardInfo, type JiraIssue, type JiraIssueSummary, type JiraPerson, type JiraProject } from "../api";
import { jira, jiraBoardsR, jiraFiltersR, jiraIssueR, jiraProjectsR, jiraSearchR } from "../resources";
import { copyIssue, issueMenu, openIssue } from "../issueLinks";
import { jiraSettings, jqlOf, mentions, updateJiraView, useJiraView, type JiraList } from "../state";
import { statusCounts, withStatuses } from "../statusFilter";
import { JiraBoard } from "./JiraBoard";
import { JiraActivity } from "./JiraActivity";
import { Avatar, ProjectAvatar } from "./JiraFace";
import { DueChip, EpicChip, IssueKey, IssueTypeIcon, PriorityGlyph } from "./JiraGlyphs";
import { JiraMark } from "./JiraMark";
import { JiraSignIn } from "./JiraSignIn";
import { StatusChip } from "./StatusChip";
import { TaskBox, TaskToggleContext, type ToggleTask } from "./TaskBox";
import "../jira.css";

const sameList = (a: JiraList, b: JiraList) => JSON.stringify(a) === JSON.stringify(b);

const ticketOf = (issue: JiraIssueSummary) => ({ key: issue.key, url: issue.url, source: issue.issueType ?? undefined, provider: "jira" as const });

type Icon = ComponentType<{ size?: number; className?: string }>;

const MY_WORK: [JiraList, string, Icon][] = [
    [{ kind: "mine" }, "Assigned to me", IconUser],
    [{ kind: "reported" }, "Reported by me", IconPencil],
    [{ kind: "watching" }, "Watching", IconEye],
    [{ kind: "recent" }, "Recently viewed", IconClock],
    [{ kind: "sprint" }, "Current sprint", IconTimer],
];

const EMPTY: Partial<Record<JiraList["kind"], string>> = {
    mine: "Nothing is assigned to you that is open or was done in the last two weeks.",
    reported: "Nothing you reported is open or was done in the last two weeks.",
    watching: "You are not watching any open issue.",
    recent: "No issue viewed lately.",
    unassigned: "Every open issue here has someone on it.",
};

export function JiraPane({ paneId, active }: { paneId: string; active: boolean }) {
    return (
        <SignedIn service={jira} active={active} signIn={(status) => <JiraSignIn status={status} onSignedIn={() => jira.refresh()} />}>
            {(status) => <JiraWorkspace paneId={paneId} active={active} sites={status.sites.map((site) => site.host)} />}
        </SignedIn>
    );
}

function JiraWorkspace({ paneId, active, sites }: { paneId: string; active: boolean; sites: string[] }) {
    const view = useJiraView(paneId);
    const [jql, setJql] = useState(view.list.kind === "jql" ? view.list.jql : "");
    const site = sites.includes(view.site) ? view.site : (sites[0] ?? "");
    const filters = useResourceEnabled(active, jiraFiltersR, site);
    const projects = useResourceEnabled(active, jiraProjectsR, site);
    const boards = useResourceEnabled(active, jiraBoardsR, site);
    const show = (list: JiraList) => updateJiraView(paneId, { list });
    const listButton: ListButton = (list, label, Mark, title, key = label) => (
        <button key={key} type="button" className={`jira-list${sameList(view.list, list) ? " active" : ""}`} title={title} onClick={() => show(list)}>
            <Mark size={14} className="jira-list-mark" />
            <span className="jira-list-label">{label}</span>
        </button>
    );

    return (
        <div className={`jira-pane${view.list.kind === "board" ? ` board${view.issue ? " with-issue" : ""}` : ""}`}>
            <nav className="jira-sidebar" aria-label="Jira lists">
                {sites.length > 1 ? (
                    <Dropdown
                        label="Site"
                        value={site}
                        options={sites.map((host) => ({ value: host, label: host }))}
                        onChange={(host) =>
                            updateJiraView(paneId, { site: host, issue: null, ...(view.list.kind === "filter" && { list: { kind: "mine" } }) })
                        }
                    />
                ) : (
                    <div className="jira-site" title={site}>
                        <JiraMark size={16} />
                        <span>{site.replace(/\.atlassian\.net$/, "")}</span>
                    </div>
                )}
                <label className="jira-jql">
                    <IconSearch size={13} className="jira-jql-mark" />
                    <input
                        value={jql}
                        onChange={(event) => setJql(event.target.value)}
                        onKeyDown={(event) => {
                            if (event.key === "Enter" && jql.trim()) show({ kind: "jql", jql: jql.trim() });
                        }}
                        placeholder="Search"
                        title="Words to find, or JQL such as project = ABC"
                        aria-label="Search issues by words or JQL"
                        spellCheck={false}
                    />
                </label>
                <div className="jira-lists">
                    <div className="jira-heading">Your work</div>
                    {MY_WORK.map(([list, label, Mark]) => listButton(list, label, Mark))}
                </div>
                <Spaces
                    site={site}
                    projects={projects.data ?? []}
                    boards={boards.data ?? []}
                    boardsError={boards.status === "error" ? (boards.error ?? "Jira would not list the boards.") : null}
                    current={view.list}
                    listButton={listButton}
                />
                {(filters.data?.length ?? 0) > 0 && (
                    <div className="jira-lists">
                        <div className="jira-heading">Starred filters</div>
                        {filters.data?.map((filter) => listButton({ kind: "filter", ...filter }, filter.name, IconStar, filter.jql, filter.id))}
                    </div>
                )}
                <button
                    type="button"
                    className="jira-signout"
                    title={`Sign out of ${site}`}
                    onClick={() =>
                        void jiraApi
                            .signOut(site)
                            .then(() => jira.refresh())
                            .catch(reportError("sign out of Jira"))
                    }>
                    <IconLogOut size={14} className="jira-list-mark" />
                    <span className="jira-list-label">Sign out</span>
                </button>
            </nav>
            {view.list.kind === "board" ? (
                <JiraBoard
                    key={view.list.id}
                    active={active}
                    boardId={view.list.id}
                    site={site}
                    selected={view.issue}
                    onSelect={(key) => updateJiraView(paneId, { issue: key === view.issue ? null : key })}
                />
            ) : (
                <IssueList
                    key={jqlOf(view.list)}
                    active={active}
                    empty={EMPTY[view.list.kind] ?? "No issues here."}
                    jql={jqlOf(view.list)}
                    site={site}
                    selected={view.issue}
                    onSelect={(key) => updateJiraView(paneId, { issue: key })}
                />
            )}
            {view.list.kind === "board" && !view.issue ? null : (
                <DetailSlot>
                    {view.issue ? (
                        <IssueDetail active={active} issueKey={view.issue} site={site} onClose={() => updateJiraView(paneId, { issue: null })} />
                    ) : (
                        <div className="jira-detail">
                            <EmptyState message="Pick an issue to read it, comment, move it along or assign it." />
                        </div>
                    )}
                </DetailSlot>
            )}
        </div>
    );
}

const DETAIL_MIN = 360;
const LIST_MIN = 260;

function DetailSlot({ children }: { children: ReactNode }) {
    const slot = useRef<HTMLDivElement>(null);
    const width = jiraSettings.useSelect((settings) => settings.detailWidth);
    const room = () => {
        const pane = slot.current?.parentElement;
        const sidebar = pane?.querySelector<HTMLElement>(":scope > .jira-sidebar");
        return (pane?.clientWidth ?? 0) - (sidebar?.offsetWidth ?? 0) - LIST_MIN - 16;
    };
    return (
        <div className="jira-detail-slot" ref={slot} style={width ? { width } : undefined}>
            <ResizeHandle
                targetRef={slot}
                axis="x"
                grows={-1}
                min={DETAIL_MIN}
                max={room}
                size={width}
                label="Resize the list and the issue"
                className="jira-split"
                onResize={(detailWidth) => jiraSettings.update((settings) => ({ ...settings, detailWidth }))}
            />
            {children}
        </div>
    );
}

type ListButton = (list: JiraList, label: string, Mark: Icon, title?: string, key?: string) => ReactNode;

const SPACES_SHOWN = 8;

/** Jira's spaces: each project with its picture, opening onto its boards and its lists. */
function Spaces({
    site,
    projects,
    boards,
    boardsError,
    current,
    listButton,
}: {
    site: string;
    projects: JiraProject[];
    boards: JiraBoardInfo[];
    boardsError: string | null;
    current: JiraList;
    listButton: ListButton;
}) {
    const [toggled, setToggled] = useState<ReadonlySet<string>>(new Set());
    const [find, setFind] = useState("");
    const [everyone, setEveryone] = useState(false);
    const known = new Set(projects.map((project) => project.key));
    const homeOf = (board: JiraBoardInfo) => board.projectKey ?? board.project ?? "";
    const boardsOf = (key: string) => boards.filter((board) => homeOf(board) === key);
    const loose = boards.filter((board) => !known.has(homeOf(board)));
    const activeKey =
        current.kind === "project" || current.kind === "unassigned"
            ? current.key
            : current.kind === "board"
              ? (boards.find((board) => board.id === current.id) ?? null)?.projectKey
              : undefined;
    const openByDefault = activeKey ?? projects.find((project) => boardsOf(project.key).length > 0)?.key ?? projects[0]?.key;
    const isOpen = (key: string) => (key === openByDefault) !== toggled.has(key);
    const toggle = (key: string) =>
        setToggled((now) => {
            const next = new Set(now);
            if (!next.delete(key)) next.add(key);
            return next;
        });
    const words = find.trim().toLowerCase();
    const matching = projects.filter(
        (project) =>
            !words ||
            project.name.toLowerCase().includes(words) ||
            project.key.toLowerCase().includes(words) ||
            boardsOf(project.key).some((board) => board.name.toLowerCase().includes(words)),
    );
    const shown = words || everyone ? matching : matching.filter((project, index) => index < SPACES_SHOWN || project.key === activeKey);
    if (projects.length === 0 && boards.length === 0 && !boardsError) return null;
    return (
        <div className="jira-lists">
            <div className="jira-heading">Spaces</div>
            {projects.length > SPACES_SHOWN && (
                <input
                    className="jira-board-filter"
                    value={find}
                    onChange={(event) => setFind(event.target.value)}
                    placeholder="Find a space or board"
                    aria-label="Find a space or board"
                    spellCheck={false}
                />
            )}
            {shown.map((project) => {
                const open = isOpen(project.key) || !!words;
                return (
                    <div key={project.key} className="jira-space">
                        <button
                            type="button"
                            className="jira-list jira-space-row"
                            aria-expanded={open}
                            title={`${project.name} · ${project.key}`}
                            onClick={() => toggle(project.key)}>
                            <IconChevron size={11} className="jira-space-chevron" />
                            <ProjectAvatar project={project} site={site} />
                            <span className="jira-list-label">{project.name}</span>
                        </button>
                        {open && (
                            <div className="jira-space-children">
                                {boardsOf(project.key).map((board) =>
                                    listButton(
                                        { kind: "board", id: board.id, name: board.name },
                                        board.name,
                                        IconBoard,
                                        board.name,
                                        String(board.id),
                                    ),
                                )}
                                {listButton(
                                    { kind: "project", key: project.key, name: project.name },
                                    "All open",
                                    IconStack,
                                    `Open issues in ${project.name}`,
                                    `${project.key}-open`,
                                )}
                                {listButton(
                                    { kind: "unassigned", key: project.key, name: project.name },
                                    "Unassigned",
                                    IconInbox,
                                    `Open issues in ${project.name} with nobody on them`,
                                    `${project.key}-unassigned`,
                                )}
                            </div>
                        )}
                    </div>
                );
            })}
            {!words && !everyone && matching.length > shown.length && (
                <button type="button" className="jira-list jira-more" onClick={() => setEveryone(true)}>
                    <span className="jira-list-label">Show all {matching.length} spaces</span>
                </button>
            )}
            {loose.length > 0 && (
                <>
                    <div className="jira-heading jira-subheading">Other boards</div>
                    {loose.map((board) =>
                        listButton(
                            { kind: "board", id: board.id, name: board.name },
                            board.name,
                            IconBoard,
                            board.project ? `${board.name} · ${board.project}` : board.name,
                            String(board.id),
                        ),
                    )}
                </>
            )}
            {boardsError && <p className="jira-lists-error">{boardsError} Sign out and in again to retry.</p>}
        </div>
    );
}

function IssueList({
    active,
    empty,
    jql,
    site,
    selected,
    onSelect,
}: {
    active: boolean;
    empty: string;
    jql: string;
    site: string;
    selected: string | null;
    onSelect: (key: string) => void;
}) {
    const found = useResourceEnabled(active, jiraSearchR, jql, site);
    const [picked, setPicked] = useState<ReadonlySet<string>>(new Set());
    return (
        <div className="jira-list-pane">
            <Loaded handle={found} loading="Loading issues" rows={8} empty={(page) => (page.issues.length === 0 ? empty : null)}>
                {(page) => {
                    const counts = statusCounts(page.issues);
                    const shown = new Set([...picked].filter((status) => counts.some((each) => each.status === status)));
                    const toggle = (status: string) => {
                        const next = new Set(shown);
                        if (!next.delete(status)) next.add(status);
                        setPicked(next);
                    };
                    return (
                        <>
                            <StaleNotice handle={found} />
                            {counts.length > 1 && (
                                <div className="jira-status-filter" role="group" aria-label="Filter by status">
                                    <button
                                        type="button"
                                        className="jira-status-pick"
                                        aria-pressed={shown.size === 0}
                                        onClick={() => setPicked(new Set())}>
                                        All <span className="jira-status-count">{page.issues.length}</span>
                                    </button>
                                    {counts.map(({ status, category, count }) => (
                                        <button
                                            key={status}
                                            type="button"
                                            className="jira-status-pick"
                                            aria-pressed={shown.has(status)}
                                            onClick={() => toggle(status)}>
                                            <span className={`jira-dot jira-dot-${category || "new"}`} />
                                            {status} <span className="jira-status-count">{count}</span>
                                        </button>
                                    ))}
                                </div>
                            )}
                            <div className="jira-issues" role="list">
                                {withStatuses(page.issues, shown).map((issue) => (
                                    <IssueRow key={issue.key} issue={issue} selected={issue.key === selected} onSelect={() => onSelect(issue.key)} />
                                ))}
                            </div>
                        </>
                    );
                }}
            </Loaded>
        </div>
    );
}

function IssueRow({ issue, selected, onSelect }: { issue: JiraIssueSummary; selected: boolean; onSelect: () => void }) {
    const [menu, setMenu] = useMenu();
    return (
        <>
            <button
                type="button"
                role="listitem"
                className={`jira-row${selected ? " active" : ""}`}
                onClick={onSelect}
                onContextMenu={(event) => setMenu(event, issueMenu(issue))}>
                <span className="jira-row-top">
                    <IssueTypeIcon type={issue.issueType} />
                    <IssueKey issue={issue} />
                    <StatusChip status={issue.status} category={issue.statusCategory} />
                </span>
                <span className="jira-row-summary">{issue.summary}</span>
                <span className="jira-row-foot">
                    <PriorityGlyph priority={issue.priority} />
                    {issue.sprint && <span className="jira-meta jira-ellipsis">{issue.sprint}</span>}
                    <span className="jira-grow" />
                    <Assignee person={issue.assignee} />
                </span>
            </button>
            {menu}
        </>
    );
}

function Assignee({ person }: { person: JiraPerson | null }) {
    return (
        <span className="jira-assignee">
            <span className="jira-ellipsis">{person?.name ?? "Unassigned"}</span>
            <Avatar person={person} />
        </span>
    );
}

/** A right-click menu: the element to render, and the handler that opens it at the pointer. */
function useMenu(): [ReactNode, (event: MouseEvent, items: ContextMenuItem[]) => void] {
    const [open, setOpen] = useState<{ x: number; y: number; items: ContextMenuItem[] } | null>(null);
    const element = open && <ContextMenu x={open.x} y={open.y} items={open.items} onClose={() => setOpen(null)} />;
    return [
        element,
        (event, items) => {
            event.preventDefault();
            setOpen({ x: event.clientX, y: event.clientY, items });
        },
    ];
}

function IssueDetail({ active, issueKey, site, onClose }: { active: boolean; issueKey: string; site: string; onClose: () => void }) {
    const found = useResourceEnabled(active, jiraIssueR, issueKey, site);
    if (found.data === undefined)
        return (
            <div className="jira-detail">
                <Pending handle={found} loading={`Loading ${issueKey}`} />
            </div>
        );
    return (
        <IssueBody
            key={found.data.key}
            issue={found.data}
            site={site}
            refresh={() => void found.refresh()}
            onClose={onClose}
            notice={<StaleNotice handle={found} />}
        />
    );
}

function IssueBody({
    issue,
    site,
    refresh,
    onClose,
    notice,
}: {
    issue: JiraIssue;
    site: string;
    refresh: () => void;
    onClose: () => void;
    notice: ReactNode;
}) {
    const [menu, setMenu] = useMenu();
    const [busy, setBusy] = useState(false);
    const [assigning, setAssigning] = useState(false);
    const [people, setPeople] = useState<JiraPerson[]>([]);
    const changed = () => {
        refresh();
        jira.refresh();
    };
    const toggleTask = useCallback<ToggleTask>(
        async (index, text, done) => {
            try {
                await jiraApi.setTask(issue.key, index, text, done, site || undefined);
                refresh();
            } catch (error) {
                notify("error", `Update the task: ${errorText(error)}`);
                throw error;
            }
        },
        [issue.key, site, refresh],
    );
    const run = async (label: string, work: () => Promise<unknown>) => {
        setBusy(true);
        try {
            await work();
            changed();
            return true;
        } catch (error) {
            notify("error", `${label}: ${errorText(error)}`);
            return false;
        } finally {
            setBusy(false);
        }
    };

    return (
        <article className="jira-detail" aria-label={`${issue.key} ${issue.summary}`}>
            {notice}
            <div className="jira-detail-body">
                <header className="jira-detail-head" onContextMenu={(event) => setMenu(event, issueMenu(issue))}>
                    <span className="jira-detail-key">
                        <IssueTypeIcon type={issue.issueType} />
                        <button type="button" className="jira-key link" title="Open in browser" onClick={() => openIssue(issue)}>
                            {issue.key}
                        </button>
                        <button
                            type="button"
                            className="jira-icon-button"
                            title="Copy link"
                            aria-label="Copy link"
                            onClick={() => copyIssue(issue.url, "link")}>
                            <IconCopy size={12} />
                        </button>
                        <button
                            type="button"
                            className="jira-icon-button"
                            title="Open in browser"
                            aria-label="Open in browser"
                            onClick={() => openIssue(issue)}>
                            <IconExternal size={12} />
                        </button>
                        <span className="jira-grow" />
                        <button
                            type="button"
                            className="jira-chip jira-send"
                            title="Put this issue on the open project's board as a card for agents"
                            onClick={() =>
                                void sendToBoard({
                                    title: issue.summary,
                                    details: issue.description,
                                    ticket: ticketOf(issue),
                                    subtasks: issue.subtasks
                                        .filter((sub) => sub.statusCategory !== "done")
                                        .map((sub) => ({ title: sub.summary, details: "", ticket: ticketOf(sub) })),
                                    blockedBy: issue.blockedBy,
                                    blocks: issue.blocks,
                                })
                            }>
                            <IconAgent size={12} />
                            Send to agent board
                        </button>
                        <button type="button" className="jira-icon-button" title="Close" aria-label="Close the issue" onClick={onClose}>
                            <IconClose size={12} />
                        </button>
                    </span>
                    <h2>{issue.summary}</h2>
                    {menu}
                </header>
                <aside className="jira-detail-side">
                    <div className="jira-status-row">
                        <Dropdown
                            label={`Status: ${issue.status}`}
                            value={issue.status}
                            title="Move this issue along its workflow"
                            className={`jira-status-button jira-status-${issue.statusCategory || "new"}`}
                            options={issue.transitions.map((transition) => ({
                                value: transition.id,
                                label: transition.to,
                                meta: transition.name.toLowerCase() === transition.to.toLowerCase() ? undefined : transition.name,
                                className: `jira-status jira-status-${transition.toCategory || "new"}`,
                            }))}
                            onChange={(id) => void run("Move issue", () => jiraApi.transition(issue.key, id, site || undefined))}
                            disabled={busy || issue.transitions.length === 0}
                        />
                        <button
                            type="button"
                            className="jira-chip"
                            disabled={busy}
                            onClick={() => void run("Assign", () => jiraApi.assign(issue.key, { me: true }, site || undefined))}>
                            <IconUser size={12} />
                            Assign to me
                        </button>
                        <button
                            type="button"
                            className="jira-chip"
                            disabled={busy || !issue.assignee}
                            onClick={() => void run("Unassign", () => jiraApi.assign(issue.key, { accountId: null }, site || undefined))}>
                            <IconUserMinus size={12} />
                            Unassign
                        </button>
                        <button type="button" className="jira-chip" disabled={busy} onClick={() => setAssigning((open) => !open)}>
                            <IconSearch size={12} />
                            Assign…
                        </button>
                    </div>
                    <section className="jira-fields" aria-label="Details">
                        <div className="jira-fields-title">Details</div>
                        <Field label="Assignee">
                            <span className="jira-person">
                                <Avatar person={issue.assignee} large />
                                {issue.assignee?.name ?? "Unassigned"}
                            </span>
                            {assigning && (
                                <div className="jira-assign">
                                    <input
                                        autoFocus
                                        placeholder="Find a person"
                                        onChange={(event) =>
                                            void jiraApi
                                                .assignable(issue.key, event.target.value, site || undefined)
                                                .then(setPeople)
                                                .catch(swallow("find people"))
                                        }
                                    />
                                    <span className="jira-chips">
                                        {people.map((person) => (
                                            <button
                                                key={person.accountId}
                                                type="button"
                                                className="jira-chip"
                                                onClick={() => {
                                                    setAssigning(false);
                                                    void run("Assign", () =>
                                                        jiraApi.assign(issue.key, { accountId: person.accountId }, site || undefined),
                                                    );
                                                }}>
                                                {person.name}
                                            </button>
                                        ))}
                                    </span>
                                </div>
                            )}
                        </Field>
                        {issue.reporter && (
                            <Field label="Reporter">
                                <span className="jira-person">
                                    <Avatar person={issue.reporter} large />
                                    {issue.reporter.name}
                                </span>
                            </Field>
                        )}
                        {issue.priority && (
                            <Field label="Priority">
                                <span className="jira-person">
                                    <PriorityGlyph priority={issue.priority} />
                                    {issue.priority}
                                </span>
                            </Field>
                        )}
                        {issue.due && (
                            <Field label="Due date">
                                <DueChip due={issue.due} done={issue.statusCategory === "done"} />
                            </Field>
                        )}
                        {issue.parent && (
                            <Field label="Parent">
                                <EpicChip parent={issue.parent} />
                            </Field>
                        )}
                        {issue.issueType && (
                            <Field label="Type">
                                <span className="jira-person">
                                    <IssueTypeIcon type={issue.issueType} />
                                    {issue.issueType}
                                </span>
                            </Field>
                        )}
                        {issue.sprint && <Field label="Sprint">{issue.sprint}</Field>}
                        {issue.labels.length > 0 && (
                            <Field label="Labels">
                                <span className="jira-chips">
                                    {issue.labels.map((label) => (
                                        <span key={label} className="jira-label">
                                            {label}
                                        </span>
                                    ))}
                                </span>
                            </Field>
                        )}
                    </section>
                </aside>
                <div className="jira-detail-main">
                    <section className="jira-description" aria-label="Description">
                        <div className="jira-section-title">Description</div>
                        {issue.description ? (
                            <TaskToggleContext.Provider value={toggleTask}>
                                <Markdown taskBox={TaskBox}>{issue.description}</Markdown>
                            </TaskToggleContext.Provider>
                        ) : (
                            <p className="jira-meta">No description.</p>
                        )}
                    </section>
                    <ProjectCommits issueKey={issue.key} />
                    <JiraActivity
                        issue={issue}
                        busy={busy}
                        onComment={(body) => run("Comment", () => jiraApi.comment(issue.key, body, site || undefined))}
                    />
                </div>
            </div>
        </article>
    );
}

function Field({ label, children }: { label: string; children: ReactNode }) {
    return (
        <div className="jira-field">
            <span className="jira-field-label">{label}</span>
            <div className="jira-field-value">{children}</div>
        </div>
    );
}

/** The open project's recent commits that name the issue in their message. */
function ProjectCommits({ issueKey }: { issueKey: string }) {
    const cwd = useActiveProjectCwd();
    const repo = useResourceEnabled(!!cwd, gitOverviewR, cwd ?? "");
    const commits = (repo.data?.log ?? []).filter((commit) => mentions(commit.subject, issueKey));
    if (commits.length === 0) return null;
    return (
        <section className="jira-commits" aria-label="Commits in this project">
            <div className="jira-section-title">Commits in this project</div>
            {commits.map((commit) => (
                <div key={commit.full_hash} className="jira-commit">
                    <span className="jira-key">{commit.hash}</span>
                    <span>{commit.subject}</span>
                    <span className="jira-meta">{commit.author}</span>
                </div>
            ))}
        </section>
    );
}
