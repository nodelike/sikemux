import { useRef, useState, type ReactNode } from "react";
import { confirmDialog, notify, swallow } from "../../../plugin-api/host";
import { useResourceEnabled, type ResourceHandle } from "../../../plugin-api/resources";
import { Dropdown, ResizeHandle, SkeletonRows, StaleNotice } from "../../../plugin-api/ui";
import { errorText } from "../../../plugin-api/format";
import { sentryApi, type IssueQuery, type SentryStatus } from "../api";
import { sentry, sentryEnvironmentsR, sentryIssuesR, sentryOrganizationsR, sentryProjectsR, sentryReleasesR } from "../resources";
import { updateSentryView, useSentryView, type SentrySection } from "../state";
import { IssueView } from "./IssueView";
import { Issues } from "./Issues";
import { Releases } from "./Releases";
import { SentryMark } from "./SentryMark";
import { SentrySignIn } from "./SentrySignIn";
import "../sentry.css";

const SIDE_MIN = 180;
const SIDE_MAX = 360;
const ALL_PROJECTS = "";
const ALL_ENVIRONMENTS = "";

export function SentryPane({ paneId, active }: { paneId: string; active: boolean }) {
    const status = useResourceEnabled(active, sentry.statusR);
    if (status.data === undefined && status.status !== "error") return <SkeletonRows rows={5} label="Connecting to Sentry" />;
    if (!status.data?.configured || status.data.authFailed) return <SentrySignIn status={status.data} onSignedIn={() => sentry.refresh()} />;
    return <Organization paneId={paneId} active={active} status={status.data} />;
}

const SECTIONS: { value: SentrySection; label: string }[] = [
    { value: "issues", label: "Issues" },
    { value: "releases", label: "Releases" },
];

function Organization({ paneId, active, status }: { paneId: string; active: boolean; status: SentryStatus }) {
    const view = useSentryView(paneId);
    const sideRef = useRef<HTMLDivElement>(null);
    const [words, setWords] = useState(view.query);
    const org = status.org;
    const organizations = useResourceEnabled(active, sentryOrganizationsR);
    const projects = useResourceEnabled(active, sentryProjectsR, org);
    const slug = view.project ?? status.project ?? ALL_PROJECTS;
    const project = projects.data?.find((each) => each.slug === slug) ?? null;
    const environments = useResourceEnabled(active, sentryEnvironmentsR, org, project?.slug ?? null);
    const query: IssueQuery = {
        projects: project ? [project.id] : [],
        query: view.query,
        status: view.status,
        sort: view.sort,
        environment: view.environment,
        period: view.period,
    };
    const waitingForProject = slug !== ALL_PROJECTS && !project && projects.data === undefined;
    const issues = useResourceEnabled(active && view.section === "issues" && !waitingForProject, sentryIssuesR, org, query);
    const releases = useResourceEnabled(
        active && view.section === "releases" && !!project,
        sentryReleasesR,
        org,
        project ?? { id: "", slug: "", name: "", platform: null },
    );

    const chooseProject = (next: string) => {
        updateSentryView(paneId, { project: next, issue: null, environment: null });
        void sentryApi.choose({ project: next }).catch(swallow("remember the Sentry project"));
    };
    const chooseOrg = async (next: string) => {
        try {
            await sentryApi.choose({ org: next });
            updateSentryView(paneId, { project: null, issue: null, environment: null });
            sentry.refresh();
        } catch (failure) {
            notify("error", errorText(failure));
        }
    };
    const signOut = async () => {
        const yes = await confirmDialog({
            title: "Sign out of Sentry?",
            body: "Sikemux forgets the token. The token itself stays valid in Sentry until you revoke it there.",
            confirmLabel: "Sign out",
        });
        if (!yes) return;
        await sentryApi.signOut().catch(swallow("sign out of Sentry"));
        sentry.refresh();
    };

    return (
        <div className="sentry-pane">
            <aside className="sentry-side" ref={sideRef} style={view.sideWidth ? { width: view.sideWidth } : undefined}>
                {(organizations.data?.length ?? 0) > 1 ? (
                    <Dropdown
                        className="sentry-pick"
                        label="Organization"
                        value={org}
                        options={(organizations.data ?? []).map((each) => ({ value: each.slug, label: each.name || each.slug }))}
                        onChange={(next) => void chooseOrg(next)}
                    />
                ) : (
                    <div className="sentry-org">
                        <SentryMark size={14} className="icon-sentry" />
                        <span>{status.orgName || org}</span>
                    </div>
                )}
                <Dropdown
                    className="sentry-pick"
                    label="Project"
                    value={slug}
                    options={[
                        { value: ALL_PROJECTS, label: "All projects" },
                        ...(projects.data ?? []).map((each) => ({ value: each.slug, label: each.slug, detail: each.platform ?? undefined })),
                    ]}
                    onChange={chooseProject}
                />
                <Dropdown
                    className="sentry-pick"
                    label="Environment"
                    value={view.environment ?? ALL_ENVIRONMENTS}
                    options={[
                        { value: ALL_ENVIRONMENTS, label: "All environments" },
                        ...(environments.data ?? []).map((name) => ({ value: name, label: name })),
                    ]}
                    onChange={(next) => updateSentryView(paneId, { environment: next || null, issue: null })}
                />
                <input
                    className="sentry-find"
                    value={words}
                    onChange={(event) => setWords(event.target.value)}
                    onKeyDown={(event) => {
                        if (event.key === "Enter") updateSentryView(paneId, { query: words.trim(), issue: null, section: "issues" });
                        if (event.key === "Escape") {
                            setWords("");
                            updateSentryView(paneId, { query: "" });
                        }
                    }}
                    placeholder="Search issues…"
                    aria-label="Search issues"
                    title="Sentry search, such as release:4.2.0 or checkout. Enter searches."
                    spellCheck={false}
                />
                <nav className="sentry-nav" aria-label="Sentry">
                    {SECTIONS.map((section) => (
                        <button
                            key={section.value}
                            type="button"
                            className={`sentry-nav-item${view.section === section.value ? " active" : ""}`}
                            aria-current={view.section === section.value ? "page" : undefined}
                            onClick={() => updateSentryView(paneId, { section: section.value, issue: null })}>
                            {section.label}
                        </button>
                    ))}
                </nav>
                <div className="sentry-side-foot">
                    <span className="sentry-side-user">{status.user || "Signed in"}</span>
                    <span className="sentry-muted sentry-mono">{status.url.replace(/^https?:\/\//, "")}</span>
                    <button type="button" className="sentry-link" onClick={() => void signOut()}>
                        Sign out
                    </button>
                </div>
            </aside>
            <ResizeHandle
                targetRef={sideRef}
                axis="x"
                grows={1}
                min={SIDE_MIN}
                max={() => SIDE_MAX}
                size={view.sideWidth}
                label="Resize the Sentry sidebar"
                className="sentry-resize"
                onResize={(size) => {
                    if (size === null && sideRef.current) sideRef.current.style.width = "";
                    updateSentryView(paneId, { sideWidth: size });
                }}
            />
            <main className="sentry-main">
                <header className="sentry-topbar">
                    <span className="sentry-topbar-title">{SECTIONS.find((each) => each.value === view.section)?.label}</span>
                    <span className="sentry-muted">
                        {project?.slug ?? "All projects"}
                        {view.environment ? ` · ${view.environment}` : ""}
                        {view.query ? ` · “${view.query}”` : ""}
                    </span>
                    <button type="button" className="sentry-button" onClick={() => sentry.refresh()}>
                        Refresh
                    </button>
                </header>
                <div className="sentry-page">
                    {view.issue ? (
                        <IssueView
                            key={view.issue}
                            org={org}
                            id={view.issue}
                            active={active}
                            onBack={() => updateSentryView(paneId, { issue: null })}
                        />
                    ) : view.section === "releases" ? (
                        project ? (
                            <Loaded handle={releases} label="releases">
                                {(data) => (
                                    <Releases
                                        releases={data}
                                        onIssues={(release) => {
                                            const search = `firstRelease:${release.version}`;
                                            setWords(search);
                                            updateSentryView(paneId, { section: "issues", query: search, status: "all" });
                                        }}
                                    />
                                )}
                            </Loaded>
                        ) : (
                            <p className="sentry-note">Pick a project to see its releases and how many sessions crashed in each.</p>
                        )
                    ) : (
                        <Loaded handle={issues} label="issues">
                            {(data) => (
                                <Issues
                                    issues={data.issues}
                                    next={data.next}
                                    query={query}
                                    showProject={!project}
                                    onQuery={(change) => updateSentryView(paneId, change)}
                                    onOpen={(issue) => updateSentryView(paneId, { issue: issue.id })}
                                />
                            )}
                        </Loaded>
                    )}
                </div>
            </main>
        </div>
    );
}

function Loaded<T>({ handle, label, children }: { handle: ResourceHandle<T>; label: string; children: (data: T) => ReactNode }) {
    if (handle.status === "error" && handle.data === undefined) return <p className="sentry-note sentry-error">{handle.error}</p>;
    if (handle.data === undefined) return <SkeletonRows rows={8} label={`Loading ${label}`} />;
    return (
        <>
            <StaleNotice handle={handle} />
            {children(handle.data)}
        </>
    );
}
