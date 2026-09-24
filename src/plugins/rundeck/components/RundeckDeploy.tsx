import { useEffect, useMemo, useState, type ReactNode } from "react";
import { confirmDialog, git, notify } from "../../../plugin-api/host";
import { invalidate, useResourceEnabled } from "../../../plugin-api/resources";
import { SkeletonRows } from "../../../plugin-api/ui";
import { errorMessage, rundeckApi, type JobDetail, type JobOption, type PlanResult } from "../api";
import * as cmd from "../state";
import type { JobRef } from "../state";
import { rndJobDetailR, rndJobsR, rndPlanR } from "../resources";
import { branchOptionName, envOf, groupSegments, isLiveStatus, isProdTarget, localDateTimeToIso, qualifiedName } from "../shape";
import { useDebounced } from "./hooks";
import { initialOptionValues, runOptionValues, validateOptions, type OptionValues, type RemoteValues } from "./options";
import { RundeckAdvanced, type AdvancedRun } from "./RundeckAdvanced";
import { RundeckOptionsForm } from "./RundeckOptionsForm";
import { RundeckPlan } from "./RundeckPlan";

interface Props {
    paneId: string;
    level: { kind: "deploy"; branch?: string; options?: Record<string, string> } & JobRef;
    active: boolean;
}

export function RundeckDeploy({ paneId, level, active }: Props) {
    const detail = useResourceEnabled(active, rndJobDetailR, level.jobId);
    const jobs = useResourceEnabled(active, rndJobsR, level.project);
    const branchOptions = cmd.rundeckSettings.useSelect((s) => s.branchOptions);
    const permalink = jobs.data?.find((job) => job.id === level.jobId)?.permalink ?? null;
    const fallback = useMemo(() => (detail.data ? null : fallbackDetail(level, branchOptions)), [detail.data, level, branchOptions]);

    if (detail.data) return <RunForm key="detail" paneId={paneId} level={level} active={active} detail={detail.data} permalink={permalink} />;
    if (detail.error && fallback) {
        return (
            <RunForm key="fallback" paneId={paneId} level={level} active={active} detail={fallback} permalink={permalink}>
                <div className="rnd-banner muted">
                    Couldn't read job options: {detail.error}. Only the branch is sent.{" "}
                    <button type="button" className="rnd-link" onClick={() => void detail.refresh()}>
                        retry
                    </button>
                </div>
            </RunForm>
        );
    }
    return (
        <div className="rnd-deploy">
            <FormHead level={level} runWord="run" />
            <SkeletonRows rows={4} label="Loading job options" />
        </div>
    );
}

/** Older Rundeck servers can't return a job definition as JSON; offer the branch option alone, plus whatever the last run was given. */
function fallbackDetail(level: Props["level"], branchOptions: string[]): JobDetail {
    const branchName = branchOptionName(Object.keys(level.options ?? {}), branchOptions) ?? branchOptions[0] ?? "BRANCH";
    const carried = Object.keys(level.options ?? {}).filter((name) => name.toLowerCase() !== branchName.toLowerCase());
    return {
        id: level.jobId,
        name: level.name,
        group: level.group,
        project: level.project,
        description: null,
        execution_enabled: true,
        schedule_enabled: true,
        scheduled: false,
        node_filter: null,
        options: [textOption(branchName, true), ...carried.map((name) => textOption(name, false))],
        steps: [],
    };
}

function textOption(name: string, required: boolean): JobOption {
    return {
        name,
        label: null,
        description: null,
        required,
        secure: false,
        value_exposed: false,
        default: null,
        values: null,
        values_url: null,
        enforced: false,
        multivalued: false,
        delimiter: null,
        is_date: false,
        date_format: null,
        kind: "text",
    };
}

function FormHead({ level, runWord, isProd = false, children }: { level: JobRef; runWord: string; isProd?: boolean; children?: ReactNode }) {
    const path = groupSegments(level.group).join(" / ");
    return (
        <div className="rnd-section-head">
            <div className="rnd-section-title">
                <span className={`rnd-section-eyebrow${isProd ? " danger" : ""}`}>
                    {runWord} → {envOf(level.project, level.group)}
                    {isProd ? " · production" : ""}
                </span>
                <span className="rnd-section-name">{level.name}</span>
                <span className="rnd-section-proj">{path ? `${level.project} / ${path}` : level.project}</span>
            </div>
            {children}
        </div>
    );
}

const FIX_OPTIONS = "Fix the highlighted options first.";

interface FormProps extends Props {
    detail: JobDetail;
    permalink: string | null;
    children?: ReactNode;
}

function RunForm({ paneId, level, active, detail, permalink, children }: FormProps) {
    const branchOptions = cmd.rundeckSettings.useSelect((s) => s.branchOptions);
    const isProd = cmd.rundeckSettings.useSelect((s) => isProdTarget(level.project, level.group, s.prodEnvs));
    const repoPath = level.repoPath ?? "";
    const branchKey = useMemo(
        () =>
            branchOptionName(
                detail.options.map((o) => o.name),
                branchOptions,
            ),
        [detail.options, branchOptions],
    );
    const runsBranch = branchKey !== null;
    const runWord = runsBranch ? "deploy" : "run";

    const [values, setValues] = useState<OptionValues>(() => initialOptionValues(detail.options, level.options, branchKey, level.branch));
    const [remote, setRemote] = useState<RemoteValues>({});
    const [advanced, setAdvanced] = useState<AdvancedRun>({ debug: false, nodeFilter: detail.node_filter ?? "", runAt: "", asUser: "" });
    const [attempted, setAttempted] = useState(false);
    const [busy, setBusy] = useState(false);
    const [error, setError] = useState<string | null>(null);

    const branch = branchKey ? (values[branchKey] ?? "").trim() : "";
    const settledBranch = useDebounced(branch, 400);
    const planRequest = useMemo(
        () => ({
            jobId: level.jobId,
            project: level.project,
            service: qualifiedName(level.name, level.group),
            targetBranch: settledBranch,
            repoPath,
            branchOptions,
        }),
        [level.jobId, level.project, level.name, level.group, settledBranch, repoPath, branchOptions],
    );
    const plan = useResourceEnabled(active && runsBranch && !!settledBranch, rndPlanR, planRequest);
    const planReady = !runsBranch || (plan.status === "ok" && plan.data?.target_branch === branch);

    const errors = useMemo(() => {
        const found = validateOptions(detail.options, values, remote);
        if (branchKey && !branch && !found[branchKey]) found[branchKey] = "enter a branch to deploy";
        return found;
    }, [detail.options, values, remote, branchKey, branch]);
    const hasErrors = Object.keys(errors).length > 0;

    useEffect(() => {
        if (!hasErrors && error === FIX_OPTIONS) setError(null);
    }, [hasErrors, error]);

    const setValue = (name: string, value: string) => setValues((prev) => ({ ...prev, [name]: value }));
    const setRemoteValues = (name: string, list: string[] | null) => setRemote((prev) => (prev[name] === list ? prev : { ...prev, [name]: list }));

    const submit = async () => {
        setAttempted(true);
        setError(null);
        if (!detail.execution_enabled) return setError("Executions are disabled for this job in Rundeck.");
        if (hasErrors) return setError(FIX_OPTIONS);
        if (!planReady) return setError("Wait for the deploy plan to finish.");
        const runAtTime = advanced.runAt ? localDateTimeToIso(advanced.runAt) : null;
        if (advanced.runAt && !runAtTime) return setError("Run later needs a full date and time.");

        if (isProd) {
            const ok = await confirmDialog({
                title: runsBranch ? `Deploy ${branch} to production?` : `Run ${level.name} in production?`,
                body: `Project: ${level.project}\nJob: ${qualifiedName(level.name, level.group)}\nEnvironment: ${envOf(level.project, level.group)}`,
                confirmLabel: runsBranch ? "deploy to production" : "run in production",
                destructive: true,
            });
            if (!ok) return;
        }

        setBusy(true);
        let pushed = false;
        try {
            const recent = await rundeckApi.executions(level.jobId, level.project, 10);
            if (recent.some((ex) => isLiveStatus(ex.status))) {
                const ok = await confirmDialog({
                    title: "A run is already in progress",
                    body: `${level.name} is already running. Start another ${runWord} anyway?`,
                    confirmLabel: `${runWord} anyway`,
                });
                if (!ok) return;
            }

            if (runsBranch && plan.data?.push_action === "will-push-current" && repoPath) {
                const status = await git.status(repoPath);
                if (status.branch !== branch) {
                    setError(`The checkout is now on ${status.branch}, not ${branch}. Nothing was pushed or run.`);
                    return;
                }
                await git.push(repoPath);
                pushed = true;
            }

            const result = await runJob(detail.options, values, advanced, detail.node_filter, level.jobId, runAtTime).catch((e: unknown) => {
                throw pushed ? new Error(`${branch} was pushed, but Rundeck rejected the run: ${errorMessage(e)}`) : e;
            });
            if (result.recovered) notify("info", "The run request timed out, but the run was found in Rundeck.");
            invalidate(
                (kind, args) => (kind === "rnd.executions" && args[0] === level.jobId) || (kind === "rnd.matrix" && args[0] === level.project),
            );
            cmd.rundeckReplace(paneId, { kind: "execution", ...stripOptions(level), executionId: result.id });
        } catch (e) {
            setError(errorMessage(e));
        } finally {
            setBusy(false);
        }
    };

    const canSubmit = !busy && detail.execution_enabled && planReady;
    const label = busy ? "starting…" : isProd ? (runsBranch ? "deploy to production" : "run in production") : runWord;

    return (
        <form
            className="rnd-deploy"
            onSubmit={(e) => {
                e.preventDefault();
                void submit();
            }}>
            <FormHead level={level} runWord={runWord} isProd={isProd}>
                <div className="rnd-deploy-actions">
                    <button type="button" className="rnd-btn" onClick={() => cmd.rundeckPop(paneId)} disabled={busy}>
                        cancel
                    </button>
                    <button type="submit" className={`rnd-btn rnd-btn-primary${isProd ? " rnd-btn-danger" : ""}`} disabled={!canSubmit}>
                        <svg className="rnd-btn-icon" viewBox="0 0 16 16" width="11" height="11" aria-hidden="true">
                            <path d="M4 2.5v11l9-5.5z" fill="currentColor" />
                        </svg>
                        {label}
                    </button>
                </div>
            </FormHead>

            {children}
            {!detail.execution_enabled && <div className="rnd-banner warn">Executions are disabled for this job in Rundeck.</div>}
            {error && <div className="rnd-banner danger">{error}</div>}

            <div className="rnd-deploy-form">
                <RundeckOptionsForm
                    options={detail.options}
                    values={values}
                    errors={errors}
                    showErrors={attempted}
                    permalink={permalink}
                    active={active}
                    onChange={setValue}
                    onRemoteValues={setRemoteValues}
                    hint={(option) =>
                        option.name === branchKey ? <UseCurrent plan={plan.data} branch={branch} onUse={(b) => setValue(option.name, b)} /> : null
                    }
                />
                <RundeckAdvanced value={advanced} onChange={setAdvanced} defaultFilter={detail.node_filter} />
            </div>

            {runsBranch && (
                <RundeckPlan
                    plan={plan.data ?? null}
                    loading={plan.status === "loading" && !!settledBranch}
                    error={plan.error ?? null}
                    branch={branch}
                    isProd={isProd}
                />
            )}
        </form>
    );
}

function UseCurrent({ plan, branch, onUse }: { plan: PlanResult | undefined; branch: string; onUse: (branch: string) => void }) {
    const current = plan?.current_branch;
    if (!current || current === branch) return null;
    return (
        <button type="button" className="rnd-field-hint" onClick={() => onUse(current)}>
            use current ({current})
        </button>
    );
}

function runJob(
    options: JobOption[],
    values: OptionValues,
    advanced: AdvancedRun,
    jobFilter: string | null,
    jobId: string,
    runAtTime: string | null,
) {
    const filter = advanced.nodeFilter.trim();
    return rundeckApi.run({
        jobId,
        options: runOptionValues(options, values),
        loglevel: advanced.debug ? "DEBUG" : null,
        filter: filter && filter !== (jobFilter ?? "") ? filter : null,
        runAtTime,
        asUser: advanced.asUser.trim() || null,
    });
}

function stripOptions(level: Props["level"]): JobRef {
    return { project: level.project, jobId: level.jobId, name: level.name, group: level.group, repoPath: level.repoPath };
}
