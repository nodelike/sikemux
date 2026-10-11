import { useEffect, useMemo, useState, type ReactNode } from "react";
import { confirmDialog, git, notify, swallow } from "../../../plugin-api/host";
import { invalidate, useResourceEnabled } from "../../../plugin-api/resources";
import { IconRun, IconWarning, SkeletonRows } from "../../../plugin-api/ui";
import { errorMessage, rundeckApi, type JobDetail, type JobOption, type PlanResult } from "../api";
import * as cmd from "../state";
import type { JobRef } from "../state";
import { rndExecutionsR, rndJobDetailR, rndJobsR, rndPlanR } from "../resources";
import { branchOf, branchOptionName, envOf, isLiveStatus, isProdTarget, localDateTimeToIso, qualifiedName } from "../shape";
import { watchRun } from "../runWatch";
import { useDebounced } from "./hooks";
import { initialOptionValues, runOptionValues, validateOptions, type OptionValues, type RemoteValues } from "./options";
import { RundeckAdvanced, type AdvancedRun } from "./RundeckAdvanced";
import { RundeckOptionsForm } from "./RundeckOptionsForm";
import { RundeckPlan } from "./RundeckPlan";
import { FolderChip, Header, levelCrumbs } from "./parts";

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
                <div className="rnd-banner muted rnd-span">
                    Couldn't read job options: {detail.error}. Only the branch is sent.{" "}
                    <button type="button" className="rnd-link" onClick={() => void detail.refresh()}>
                        retry
                    </button>
                </div>
            </RunForm>
        );
    }
    return (
        <div className="rnd-main">
            <FormHead paneId={paneId} level={level} runWord="run" />
            <div className="rnd-pad">
                <SkeletonRows rows={4} label="Loading job options" />
            </div>
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

function FormHead({ paneId, level, runWord }: { paneId: string; level: Props["level"]; runWord: string }) {
    return (
        <Header
            paneId={paneId}
            crumbs={levelCrumbs(paneId, level, level.project, level.group, runWord)}
            title={`${runWord === "deploy" ? "Deploy" : "Run"} ${level.name}`}
            aside={<FolderChip project={level.project} group={level.group} />}
        />
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
            void watchRun(cmd.jobRefOf(level), result.id).catch(swallow("watch the run"));
            cmd.rundeckReplace(paneId, { kind: "execution", ...cmd.jobRefOf(level), executionId: result.id });
        } catch (e) {
            setError(errorMessage(e));
        } finally {
            setBusy(false);
        }
    };

    const canSubmit = !busy && detail.execution_enabled && planReady;
    const label = busy ? "Starting…" : isProd ? (runsBranch ? "Deploy to production" : "Run in production") : runsBranch ? "Deploy" : "Run";

    return (
        <form
            className="rnd-main"
            onSubmit={(e) => {
                e.preventDefault();
                void submit();
            }}>
            <FormHead paneId={paneId} level={level} runWord={runWord} />
            <div className={`rnd-form${runsBranch ? "" : " single"}`}>
                {children}
                {!detail.execution_enabled && <div className="rnd-banner warn rnd-span">Executions are disabled for this job in Rundeck.</div>}
                {error && <div className="rnd-banner danger rnd-span">{error}</div>}

                <section className="rnd-card">
                    <div className="rnd-card-head">
                        <b>Options</b>
                        {detail.description && <span>{detail.description}</span>}
                    </div>
                    <div className="rnd-card-body">
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
                                option.name === branchKey ? (
                                    <BranchPicks
                                        level={level}
                                        active={active}
                                        plan={plan.data}
                                        branch={branch}
                                        onPick={(b) => setValue(option.name, b)}
                                    />
                                ) : null
                            }
                        />
                        <RundeckAdvanced value={advanced} onChange={setAdvanced} defaultFilter={detail.node_filter} />
                    </div>
                </section>

                {runsBranch && (
                    <RundeckPlan
                        plan={plan.data ?? null}
                        loading={plan.status === "loading" && !!settledBranch}
                        error={plan.error ?? null}
                        branch={branch}
                        onRecheck={() => void plan.refresh()}
                    />
                )}

                {isProd && (
                    <div className="rnd-banner warn rnd-span">
                        <IconWarning size={14} />
                        <span>
                            This {runWord}s to <b>production</b> ({envOf(level.project, level.group)}). You'll be asked to confirm.
                        </span>
                    </div>
                )}

                <div className="rnd-form-foot">
                    <button type="button" className="rnd-btn lg" onClick={() => cmd.rundeckPop(paneId)} disabled={busy}>
                        Cancel
                    </button>
                    <button type="submit" className={`rnd-btn lg ${isProd ? "prod" : "primary"}`} disabled={!canSubmit}>
                        <IconRun size={11} />
                        {label}
                    </button>
                </div>
            </div>
        </form>
    );
}

/** Branches worth one click: the checkout's, the live one, and whatever ran recently. */
function BranchPicks({
    level,
    active,
    plan,
    branch,
    onPick,
}: {
    level: JobRef;
    active: boolean;
    plan: PlanResult | undefined;
    branch: string;
    onPick: (branch: string) => void;
}) {
    const branchOptions = cmd.rundeckSettings.useSelect((s) => s.branchOptions);
    const execs = useResourceEnabled(active, rndExecutionsR, level.jobId, level.project, 25);
    const picks: { name: string; note?: string }[] = [];
    const add = (name: string | null | undefined, note?: string) => {
        if (!name) return;
        const found = picks.find((p) => p.name === name);
        if (found) found.note ??= note;
        else picks.push({ name, note });
    };
    add(plan?.current_branch, "checkout");
    add(plan?.deployed_branch, "live");
    for (const ex of execs.data ?? []) {
        if (picks.length >= 6) break;
        add(branchOf(ex.job?.options, branchOptions));
    }
    if (picks.length === 0) return null;
    return (
        <div className="rnd-picks" role="group" aria-label="Recent branches">
            {picks.map((pick) => (
                <button key={pick.name} type="button" className={`rnd-pick${pick.name === branch ? " on" : ""}`} onClick={() => onPick(pick.name)}>
                    {pick.name}
                    {pick.note && <small>{pick.note}</small>}
                </button>
            ))}
        </div>
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
