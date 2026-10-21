import { useState } from "react";
import { SendToAgentMenu } from "../agents/SendToAgentMenu";
import { folderName, type FolderSettings } from "../projects/projectFolders";
import { checkSetup, useCheckResults, type CheckResult, type Drift } from "../projects/projectHealth";
import type { Suggestion } from "../projects/projectSuggestions";
import * as cmd from "../state/commands";
import { useStore } from "../state/store";
import { failureFixRequest } from "../tasks/agentRequests";
import { useProjectTasks } from "../tasks/coreTasks";
import { SettingsPage, SettingsSection } from "./SettingsLayout";

/** What needs the person in the project's setup: suggestions waiting, settings the project moved past, and whether Done means passes. */
export function ProjectOverviewPage({
    project,
    name,
    settings,
    waiting,
    drifts,
    fixDrift,
}: {
    project: string;
    name: string;
    settings: Readonly<Record<string, FolderSettings>>;
    waiting: readonly Suggestion[];
    drifts: readonly Drift[];
    fixDrift: (drift: Drift) => void;
}) {
    const sessionId = useStore((s) => s.activeSessionId);
    const tasks = useProjectTasks(project)?.tasks ?? [];
    const checked = Object.entries(settings).filter(([, chosen]) => chosen.done?.length);
    const taskIds = checked.flatMap(([, chosen]) => chosen.done ?? []);
    const results = useCheckResults(project, taskIds);
    const [asking, setAsking] = useState<{ x: number; y: number; text: string } | null>(null);
    const running = [...results.values()].some((result) => result.state === "running");
    const nothingSet = !Object.keys(settings).length && !waiting.length;
    const ask = (anchor: HTMLElement, text: string) => {
        const rect = anchor.getBoundingClientRect();
        setAsking({ x: rect.right, y: rect.bottom + 4, text });
    };
    const label = (id: string) => tasks.find((task) => task.id === id)?.label ?? id;

    return (
        <SettingsPage>
            {waiting.map((suggestion) => (
                <div key={suggestion.id} className="project-banner">
                    <div>
                        <div className="project-banner-title">
                            {suggestion.by} suggests {changes(suggestion.changes.filter((change) => change.status === "pending").length)}
                        </div>
                        <div className="project-row-note">{suggestion.title}</div>
                    </div>
                    <button type="button" className="settings-btn primary" onClick={() => cmd.setProjectSettingsPage(project, "suggestions")}>
                        Review
                    </button>
                </div>
            ))}
            {drifts.map((drift) => (
                <DriftBanner key={`${drift.folder}:${drift.language}`} drift={drift} onFix={() => fixDrift(drift)} />
            ))}
            {nothingSet && (
                <div className="project-banner">
                    <div>
                        <div className="project-banner-title">Nothing is set up yet</div>
                        <div className="project-row-note">An agent can read the repository, run the build and tests, and suggest a full setup.</div>
                    </div>
                    <button type="button" className="settings-btn primary" onClick={() => cmd.setProjectSettingsPage(project, "suggestions")}>
                        Set up with an agent
                    </button>
                </div>
            )}
            <SettingsSection
                title="Health"
                meta={running ? "checking…" : undefined}
                sub="Each folder's Done means, run with these settings. Agents run the same checks before they say they are finished.">
                {checked.length === 0 ? (
                    <p className="settings-hint">
                        Choose what Done means for a folder to see its health here.{" "}
                        <button type="button" className="project-link" onClick={() => cmd.setProjectSettingsPage(project, "done")}>
                            Choose Done means
                        </button>
                    </p>
                ) : (
                    <>
                        <div className="project-health">
                            {checked.map(([folder, chosen]) => (
                                <div key={folder} className="project-health-card">
                                    <div className="project-banner-title">
                                        {folderName(folder, name)}
                                        <StatePill results={(chosen.done ?? []).map((id) => results.get(id))} />
                                    </div>
                                    {(chosen.done ?? []).map((id) => {
                                        const result = results.get(id);
                                        const task = tasks.find((candidate) => candidate.id === id);
                                        return (
                                            <div key={id} className="project-health-line">
                                                <span className={`project-pill ${pillOf(result)}`}>{label(id)}</span>
                                                <span className="project-row-note">{result?.detail}</span>
                                                {result?.state === "failed" && task && (
                                                    <button
                                                        type="button"
                                                        className="project-link"
                                                        onClick={(event) =>
                                                            ask(event.currentTarget, failureFixRequest(task, result.detail, project))
                                                        }>
                                                        Fix with an agent
                                                    </button>
                                                )}
                                            </div>
                                        );
                                    })}
                                </div>
                            ))}
                        </div>
                        <div className="settings-actions start">
                            <button type="button" className="settings-btn" disabled={running} onClick={() => void checkSetup(project, taskIds)}>
                                {running ? "Checking…" : "Check setup"}
                            </button>
                        </div>
                    </>
                )}
            </SettingsSection>
            {asking && (
                <SendToAgentMenu
                    x={asking.x}
                    y={asking.y}
                    sessionId={sessionId}
                    alignRight
                    delivery={() => ({ text: asking.text })}
                    onClose={() => setAsking(null)}
                />
            )}
        </SettingsPage>
    );
}

export function DriftBanner({ drift, onFix }: { drift: Drift; onFix: () => void }) {
    return (
        <div className="project-banner warn">
            <div>
                <div className="project-banner-title">{drift.title}</div>
                <div className="project-row-note">{drift.detail}</div>
            </div>
            <button type="button" className="settings-btn" onClick={onFix}>
                {drift.fix.label}
            </button>
        </div>
    );
}

function pillOf(result: CheckResult | undefined): string {
    if (result?.state === "passed") return "ok";
    if (result?.state === "failed") return "failed";
    if (result?.state === "running") return "waiting";
    return "";
}

function StatePill({ results }: { results: readonly (CheckResult | undefined)[] }) {
    const states = results.map((result) => result?.state ?? "never");
    const [label, tone] = states.includes("failed")
        ? ["failing", "failed"]
        : states.includes("running")
          ? ["checking", "waiting"]
          : states.every((state) => state === "passed")
            ? ["passing", "ok"]
            : ["not checked", ""];
    return <span className={`project-pill ${tone}`}>{label}</span>;
}

const changes = (count: number) => `${count} ${count === 1 ? "change" : "changes"}`;
