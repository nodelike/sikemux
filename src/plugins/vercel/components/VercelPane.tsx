import { useRef, useState } from "react";
import { errorText } from "../../../plugin-api/format";
import { notify, openSignInUrl, swallow } from "../../../plugin-api/host";
import { useResourceEnabled } from "../../../plugin-api/resources";
import { Dropdown, Pending, ResizeHandle, SignedIn, StaleNotice } from "../../../plugin-api/ui";
import { vercelApi, type VercelProject, type VercelStatus } from "../api";
import { vercel, vercelProjectsR } from "../resources";
import { updateVercelView, useVercelView, type VercelSection } from "../state";
import { Deployment } from "./Deployment";
import { Deployments } from "./Deployments";
import { BuildSettings } from "./BuildSettings";
import { Crons } from "./Crons";
import { DeployFolder } from "./DeployFolder";
import { Firewall } from "./Firewall";
import { Domains } from "./Domains";
import { Environment } from "./Settings";
import { Overview } from "./Overview";
import { Projects } from "./Projects";
import { StateDot } from "./parts";
import { VercelMark } from "./VercelMark";
import { VercelSignIn } from "./VercelSignIn";
import "../vercel.css";

const ADD_TEAM = "add-team";
const SIDE_MIN = 180;
const SIDE_MAX = 420;

export function VercelPane({ paneId, active }: { paneId: string; active: boolean }) {
    return (
        <SignedIn service={vercel} active={active} signIn={(status) => <VercelSignIn status={status} onSignedIn={() => vercel.refresh()} />}>
            {(status) => <Dashboard paneId={paneId} active={active} status={status} />}
        </SignedIn>
    );
}

const SECTIONS: { value: VercelSection; label: string }[] = [
    { value: "overview", label: "Overview" },
    { value: "deployments", label: "Deployments" },
    { value: "env", label: "Environment Variables" },
    { value: "domains", label: "Domains" },
    { value: "firewall", label: "Firewall" },
    { value: "crons", label: "Cron Jobs" },
    { value: "settings", label: "Settings" },
];

/** The account's dashboard laid out as Vercel's: a sidebar of the team and the project's pages, and the page beside it. */
function Dashboard({ paneId, active, status }: { paneId: string; active: boolean; status: VercelStatus }) {
    const view = useVercelView(paneId);
    const sideRef = useRef<HTMLDivElement>(null);
    const [deploying, setDeploying] = useState(false);
    const team = status.team ?? "";
    const scopeSlug = status.teams.find((each) => each.id === team)?.slug ?? status.account?.username ?? "";
    const projects = useResourceEnabled(active, vercelProjectsR, team);
    const project = projects.data?.find((each) => each.name === view.project) ?? null;

    const chooseTeam = (next: string) => {
        if (next === team) return;
        if (next === ADD_TEAM) {
            void vercelApi
                .signInWithBrowser((url) => void openSignInUrl(url).catch(swallow("open Vercel")))
                .done.then(() => vercel.refresh())
                .catch((error: unknown) => notify("error", `Add a team: ${errorText(error)}`));
            return;
        }
        updateVercelView(paneId, { project: null, deployment: null, section: "overview" });
        void vercelApi
            .setTeam(next)
            .then(() => vercel.refresh())
            .catch((error: unknown) => notify("error", `Switch team: ${errorText(error)}`));
    };
    const open = (patch: Parameters<typeof updateVercelView>[1]) => updateVercelView(paneId, { deployment: null, ...patch });

    return (
        <div className="vercel-pane">
            <aside className="vercel-side" ref={sideRef} style={view.sideWidth ? { width: view.sideWidth } : undefined}>
                <Dropdown
                    className="vercel-team"
                    label="Team"
                    value={team}
                    icon={
                        <span className="vercel-team-mark">
                            <VercelMark size={12} />
                        </span>
                    }
                    options={[
                        ...(status.personal
                            ? [{ value: "", label: status.account?.name || status.account?.username || "Personal", detail: "Personal account" }]
                            : []),
                        ...status.teams.map((each) => ({ value: each.id, label: each.name, detail: each.slug })),
                        ...(status.account?.signIn === "browser" && status.browserSignIn
                            ? [{ value: ADD_TEAM, label: "Add a team…", detail: "Install Sikemux on another team" }]
                            : []),
                    ]}
                    onChange={chooseTeam}
                />
                <input
                    className="vercel-find"
                    value={view.find}
                    onChange={(event) => updateVercelView(paneId, { find: event.target.value })}
                    onKeyDown={(event) => {
                        if (event.key === "Escape") updateVercelView(paneId, { find: "" });
                    }}
                    placeholder="Find…"
                    aria-label="Find"
                    spellCheck={false}
                />
                <nav className="vercel-nav" aria-label="Vercel">
                    {project ? (
                        <>
                            {SECTIONS.map((section) => (
                                <button
                                    key={section.value}
                                    type="button"
                                    className={`vercel-nav-item${view.section === section.value && !view.deployment ? " active" : ""}`}
                                    onClick={() => open({ section: section.value })}>
                                    {section.label}
                                </button>
                            ))}
                            <div className="vercel-nav-rule" />
                            <button type="button" className="vercel-nav-item" onClick={() => open({ project: null, section: "overview" })}>
                                ← All projects
                            </button>
                        </>
                    ) : (
                        <>
                            <button type="button" className="vercel-nav-item active" onClick={() => open({ project: null })}>
                                Projects
                            </button>
                            <div className="vercel-nav-rule" />
                            {(projects.data ?? []).map((each) => (
                                <button
                                    key={each.id}
                                    type="button"
                                    className="vercel-nav-item project"
                                    onClick={() => open({ project: each.name, section: "overview" })}>
                                    <StateDot state={each.production?.state ?? "NONE"} />
                                    {each.name}
                                </button>
                            ))}
                        </>
                    )}
                </nav>
                <div className="vercel-side-foot">
                    <span className="vercel-side-account">{status.account?.name || status.account?.username}</span>
                    <button
                        type="button"
                        className="vercel-link"
                        onClick={() =>
                            void vercelApi
                                .signOut()
                                .then(() => vercel.refresh())
                                .catch(swallow("sign out of Vercel"))
                        }>
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
                label="Resize the Vercel sidebar"
                className="vercel-resize"
                onResize={(size) => {
                    if (size === null && sideRef.current) sideRef.current.style.width = "";
                    updateVercelView(paneId, { sideWidth: size });
                }}
            />
            <main className="vercel-main">
                <header className="vercel-topbar">
                    {project && projects.data ? (
                        <Dropdown
                            className="vercel-project-switch"
                            label="Project"
                            value={project.name}
                            options={projects.data.map((each: VercelProject) => ({
                                value: each.name,
                                label: each.name,
                                detail: each.framework ?? undefined,
                            }))}
                            onChange={(name) => open({ project: name })}
                        />
                    ) : (
                        <span className="vercel-topbar-title">Projects</span>
                    )}
                    <span className="vercel-topbar-section">
                        {project ? (view.deployment ? "Deployment Details" : SECTIONS.find((each) => each.value === view.section)?.label) : ""}
                    </span>
                    <span className="vercel-actions">
                        {project ? (
                            <button type="button" className="vercel-button" onClick={() => setDeploying((was) => !was)}>
                                Deploy
                            </button>
                        ) : null}
                        <button type="button" className="vercel-button" onClick={() => vercel.refresh()}>
                            Refresh
                        </button>
                    </span>
                </header>
                <div className="vercel-page">
                    <StaleNotice handle={projects} />
                    {deploying && project ? (
                        <DeployFolder
                            team={team}
                            project={project}
                            onDeployed={(id) => {
                                setDeploying(false);
                                updateVercelView(paneId, { deployment: id });
                            }}
                            onClose={() => setDeploying(false)}
                        />
                    ) : null}
                    {!projects.data ? (
                        <Pending handle={projects} loading="Loading projects" />
                    ) : !project ? (
                        <Projects projects={projects.data} find={view.find} onOpen={(name) => open({ project: name, section: "overview" })} />
                    ) : view.deployment ? (
                        <Deployment
                            active={active}
                            team={team}
                            id={view.deployment}
                            production={project.production?.id ?? null}
                            onBack={() => open({})}
                        />
                    ) : view.section === "overview" ? (
                        <Overview
                            active={active}
                            team={team}
                            scopeSlug={scopeSlug}
                            project={project}
                            onOpen={(id) => updateVercelView(paneId, { deployment: id })}
                            onDeployments={() => open({ section: "deployments" })}
                        />
                    ) : view.section === "deployments" ? (
                        <Deployments
                            active={active}
                            team={team}
                            project={project}
                            find={view.find}
                            environment={view.environment}
                            status={view.status}
                            onFilter={(patch) => updateVercelView(paneId, patch)}
                            onOpen={(id) => updateVercelView(paneId, { deployment: id })}
                        />
                    ) : view.section === "firewall" ? (
                        <Firewall active={active} team={team} project={project.name} />
                    ) : view.section === "crons" ? (
                        <Crons active={active} team={team} project={project} />
                    ) : view.section === "settings" ? (
                        <BuildSettings active={active} team={team} project={project.name} />
                    ) : view.section === "env" ? (
                        <Environment active={active} team={team} project={project.name} find={view.find} />
                    ) : (
                        <Domains active={active} team={team} project={project.name} />
                    )}
                </div>
            </main>
        </div>
    );
}
