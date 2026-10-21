import { useState } from "react";
import { promptDialog } from "../../../plugin-api/host";
import { useResourceEnabled } from "../../../plugin-api/resources";
import { SendToAgentMenu } from "../../../plugin-api/ui";
import { gcpApi, type Build, type BuildDetail, type Target, type Trigger } from "../api";
import { buildR, buildsR, triggersR } from "../resources";
import { splitOpen, updateGcpView, useGcpView } from "../state";
import type { ProductProps } from "./GcpPane";
import { Filter } from "./RunPage";
import {
    ago,
    Button,
    consoleUrl,
    Copyable,
    Facts,
    keyOf,
    Loaded,
    PageHeader,
    Status,
    Table,
    Tabs,
    took,
    useAction,
    usePollWhile,
    visit,
} from "./parts";

const LISTS = [
    { value: "history", label: "History" },
    { value: "triggers", label: "Triggers" },
] as const;

const underway = (status: string) => status === "QUEUED" || status === "WORKING" || status === "PENDING";

/** A failed build as an agent reads it: where it came from, which step failed and the end of its log. */
export function buildForAgent(build: BuildDetail): string {
    const failed = build.steps.find((step) => step.status === "FAILURE");
    return [
        `Cloud Build ${build.id}: ${build.status}${build.trigger ? ` (trigger ${build.trigger})` : ""}`,
        [
            build.repo && `Repository: ${build.repo}`,
            build.branch && `Branch: ${build.branch}`,
            build.commit && `Commit: ${build.commit}`,
            failed && `Failed step: ${failed.id ?? failed.image}`,
            build.failure && `Failure: ${build.failure}`,
        ]
            .filter(Boolean)
            .join("\n"),
        build.log.length > 0 ? `End of the build log:\n\n\`\`\`\n${build.log.slice(-120).join("\n")}\n\`\`\`` : null,
        `To read more, use gcp_get with kind build and name ${build.id}.`,
    ]
        .filter(Boolean)
        .join("\n\n");
}

export function BuildPage({ paneId, active, target }: ProductProps) {
    const view = useGcpView(paneId);
    const list = view.list === "triggers" ? "triggers" : "history";
    const opened = splitOpen(view.open);
    if (opened)
        return (
            <BuildDetails
                active={active}
                target={target}
                region={opened.location}
                id={opened.name}
                onBack={() => updateGcpView(paneId, { open: null })}
                onOpen={(build) => updateGcpView(paneId, { open: `${build.region ?? ""}/${build.id}` })}
            />
        );
    return (
        <div className="gcp-page">
            <PageHeader title="Cloud Build">
                <Button onClick={() => visit(consoleUrl(list === "triggers" ? "cloud-build/triggers" : "cloud-build/builds", target.project))}>
                    Open in Cloud console
                </Button>
            </PageHeader>
            <Tabs label="Cloud Build" tabs={LISTS} value={list} onChange={(value) => updateGcpView(paneId, { list: value })} />
            <Filter paneId={paneId} placeholder={list === "triggers" ? "Filter triggers" : "Filter builds"} />
            {list === "history" ? (
                <History
                    active={active}
                    target={target}
                    filter={view.filter}
                    onOpen={(build) => updateGcpView(paneId, { open: `${build.region ?? ""}/${build.id}` })}
                />
            ) : (
                <Triggers
                    active={active}
                    target={target}
                    filter={view.filter}
                    onStarted={(build) => updateGcpView(paneId, { list: "history", open: `${build.region ?? ""}/${build.id}` })}
                />
            )}
        </div>
    );
}

function History({ active, target, filter, onOpen }: { active: boolean; target: Target; filter: string; onOpen: (build: Build) => void }) {
    const builds = useResourceEnabled(active, buildsR, ...keyOf(target));
    usePollWhile(active, builds.data?.some((build) => underway(build.status)) ?? false, builds.refresh);
    const words = filter.trim().toLowerCase();
    return (
        <Loaded handle={builds} label="builds" empty={(rows) => (rows.length === 0 ? "No builds in this project yet." : null)}>
            {(rows) => (
                <Table
                    label="Builds"
                    rows={rows.filter(
                        (build) =>
                            !words ||
                            [build.id, build.trigger, build.repo, build.branch, build.commit].some((field) => field?.toLowerCase().includes(words)),
                    )}
                    rowKey={(build) => build.id}
                    onOpen={onOpen}
                    columns={[
                        { label: "", width: "24px", cell: (build) => <Status state={build.status} /> },
                        {
                            label: "Build",
                            width: "minmax(90px, 0.7fr)",
                            cell: (build) => <span className="gcp-name gcp-mono">{build.id.slice(0, 8)}</span>,
                        },
                        { label: "Source", width: "minmax(140px, 1.2fr)", cell: (build) => build.trigger || build.repo || "Manual" },
                        { label: "Branch", cell: (build) => build.branch ?? "—" },
                        { label: "Commit", cell: (build) => <span className="gcp-mono">{build.commit?.slice(0, 7) ?? "—"}</span> },
                        { label: "Created", cell: (build) => ago(build.createdAt) },
                        { label: "Duration", cell: (build) => took(build.startedAt, build.finishedAt) },
                    ]}
                />
            )}
        </Loaded>
    );
}

function BuildDetails({
    active,
    target,
    region,
    id,
    onBack,
    onOpen,
}: {
    active: boolean;
    target: Target;
    region: string;
    id: string;
    onBack: () => void;
    onOpen: (build: Build) => void;
}) {
    const build = useResourceEnabled(active, buildR, ...keyOf(target), id, region);
    usePollWhile(active, build.data ? underway(build.data.status) : false, build.refresh);
    const action = useAction();
    const [menu, setMenu] = useState<{ x: number; y: number } | null>(null);
    const detail = build.data;
    return (
        <div className="gcp-page">
            <PageHeader title={<>Build details: {id.slice(0, 8)}</>} back={{ label: "history", go: onBack }}>
                {detail && underway(detail.status) ? (
                    <Button
                        danger
                        disabled={action.busy}
                        onClick={() =>
                            void action.run(
                                {
                                    title: "Cancel this build?",
                                    body: `Cloud Build stops build ${id.slice(0, 8)}.`,
                                    confirmLabel: "Cancel build",
                                    destructive: true,
                                },
                                "Cancelling the build",
                                () => gcpApi.buildChange(target, id, region || null, "cancel"),
                            )
                        }>
                        Cancel
                    </Button>
                ) : (
                    <Button
                        disabled={action.busy || !detail}
                        onClick={() =>
                            void (async () => {
                                let started: Build | undefined;
                                const ok = await action.run(
                                    {
                                        title: "Retry this build?",
                                        body: `Cloud Build runs build ${id.slice(0, 8)} again from the same source.`,
                                        confirmLabel: "Retry",
                                    },
                                    "Retrying the build",
                                    async () => {
                                        started = await gcpApi.buildChange(target, id, region || null, "retry");
                                    },
                                );
                                if (ok && started?.id) onOpen(started);
                            })()
                        }>
                        Retry
                    </Button>
                )}
                <button type="button" className="gcp-button" disabled={!detail} onClick={(event) => setMenu({ x: event.clientX, y: event.clientY })}>
                    Send to agent
                </button>
                {detail?.logUrl && <Button onClick={() => visit(detail.logUrl!)}>Open in Cloud console</Button>}
            </PageHeader>
            <Loaded handle={build} label="the build" empty={() => null}>
                {(found) => (
                    <>
                        <div className="gcp-summary">
                            <Status state={found.status} text />
                            {found.failure && <span className="gcp-error-text">{found.failure}</span>}
                        </div>
                        <Facts
                            items={[
                                ["Build id", <Copyable key="id" text={found.id} what="the build id" />],
                                ["Trigger", found.trigger],
                                ["Repository", found.repo],
                                ["Branch", found.branch],
                                ["Commit", found.commit && <Copyable key="c" text={found.commit} what="the commit" />],
                                ["Started", ago(found.startedAt ?? found.createdAt)],
                                ["Duration", took(found.startedAt, found.finishedAt)],
                                ["Images", found.images?.join(", ")],
                            ]}
                        />
                        <h2 className="gcp-section-title">Steps</h2>
                        <Table
                            label="Steps"
                            rows={found.steps}
                            rowKey={(step) => `${step.id ?? ""}-${step.image}-${step.startedAt ?? ""}`}
                            columns={[
                                { label: "", width: "24px", cell: (step) => <Status state={step.status ?? "QUEUED"} /> },
                                { label: "Step", width: "minmax(120px, 1fr)", cell: (step) => step.id ?? "—" },
                                { label: "Image", width: "minmax(160px, 1.6fr)", cell: (step) => <span className="gcp-muted">{step.image}</span> },
                                { label: "Duration", cell: (step) => took(step.startedAt, step.endedAt) },
                            ]}
                        />
                        <h2 className="gcp-section-title">Build log{found.logSkipped > 0 ? ` (last ${found.log.length} lines)` : ""}</h2>
                        <pre className="gcp-terminal">
                            {found.log.length > 0
                                ? found.log.join("\n")
                                : underway(found.status)
                                  ? "Waiting for the build to print…"
                                  : "No log lines were found."}
                        </pre>
                        {menu && (
                            <SendToAgentMenu
                                x={menu.x}
                                y={menu.y}
                                alignRight
                                delivery={() => ({ text: buildForAgent(found) })}
                                onClose={() => setMenu(null)}
                            />
                        )}
                    </>
                )}
            </Loaded>
        </div>
    );
}

function Triggers({ active, target, filter, onStarted }: { active: boolean; target: Target; filter: string; onStarted: (build: Build) => void }) {
    const triggers = useResourceEnabled(active, triggersR, ...keyOf(target));
    const action = useAction();
    const words = filter.trim().toLowerCase();
    const run = async (trigger: Trigger) => {
        const branch = await promptDialog({
            title: `Run ${trigger.name}`,
            body: "The branch to build. Leave it as it is to build the trigger's own.",
            label: "Branch",
            initial: trigger.branch?.replace(/^\^|\$$/g, "") ?? "",
            confirmLabel: "Run",
        });
        if (branch === null) return;
        let started: Build | undefined;
        const ok = await action.run(
            {
                title: `Run ${trigger.name}?`,
                body: `Cloud Build starts ${trigger.name}${branch ? ` on ${branch}` : ""} now.`,
                confirmLabel: "Run trigger",
            },
            `${trigger.name} is running`,
            async () => {
                started = await gcpApi.triggerRun(target, trigger.id, branch.trim() || null);
            },
        );
        if (ok && started?.id) onStarted(started);
    };
    return (
        <Loaded handle={triggers} label="triggers" empty={(rows) => (rows.length === 0 ? "No triggers in this project." : null)}>
            {(rows) => (
                <Table
                    label="Triggers"
                    rows={rows.filter(
                        (trigger) => !words || trigger.name.toLowerCase().includes(words) || trigger.repo?.toLowerCase().includes(words),
                    )}
                    rowKey={(trigger) => trigger.id}
                    columns={[
                        { label: "Name", width: "minmax(140px, 1.2fr)", cell: (trigger) => trigger.name },
                        { label: "Repository", cell: (trigger) => trigger.repo ?? "—" },
                        { label: "Event", cell: (trigger) => (trigger.pullRequests ? "Pull request" : `Push to ${trigger.branch ?? "branch"}`) },
                        { label: "Build file", cell: (trigger) => trigger.file ?? "Inline" },
                        {
                            label: "",
                            width: "90px",
                            align: "end",
                            cell: (trigger) =>
                                trigger.disabled ? (
                                    <span className="gcp-muted">Disabled</span>
                                ) : (
                                    <Button disabled={action.busy} onClick={() => void run(trigger)}>
                                        Run
                                    </Button>
                                ),
                        },
                    ]}
                />
            )}
        </Loaded>
    );
}
