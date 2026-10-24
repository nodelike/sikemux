import { useEffect, useMemo, useState } from "react";
import { confirmDialog, currentBranch, git, notify } from "../../../plugin-api/host";
import { useResourceEnabled } from "../../../plugin-api/resources";
import { EmptyState, IconRun, IconWarning, SkeletonRows } from "../../../plugin-api/ui";
import { errorText, formatDuration } from "../../../plugin-api/format";
import { rundeckApi } from "../api";
import * as cmd from "../state";
import type { JobRef } from "../state";
import { rndDeployV, rndPlanV } from "../resources";
import { localDateTimeToIso } from "../shape";
import { watchRun } from "../runWatch";
import { useDebounced } from "../../../plugin-api/hooks";
import type { RundeckDeployForm, RundeckPlan as Plan, RundeckRunCheck } from "../types.generated";
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
    const form = useResourceEnabled(
        active,
        rndDeployV,
        paneId,
        level.jobId,
        level.project,
        level.name,
        level.group ?? "",
        level.branch ?? "",
        level.options ?? {},
    );
    if (form.data) return <RunForm paneId={paneId} level={level} active={active} form={form.data} onRetry={() => void form.refresh()} />;
    return (
        <div className="rnd-main">
            <FormHead paneId={paneId} level={level} runWord="run" prod={false} />
            <div className="rnd-pad">
                {form.error ? (
                    <EmptyState
                        tone="error"
                        icon={<IconWarning size={14} />}
                        title="Couldn't load the job"
                        message={form.error}
                        action={{ label: "Retry", onClick: () => void form.refresh() }}
                    />
                ) : (
                    <SkeletonRows rows={4} label="Loading job options" />
                )}
            </div>
        </div>
    );
}

function FormHead({ paneId, level, runWord, prod }: { paneId: string; level: Props["level"]; runWord: string; prod: boolean }) {
    return (
        <Header
            paneId={paneId}
            crumbs={levelCrumbs(paneId, level, level.project, level.group, runWord)}
            title={`${runWord === "deploy" ? "Deploy" : "Run"} ${level.name}`}
            aside={<FolderChip project={level.project} group={level.group} prod={prod} />}
        />
    );
}

const FIX_OPTIONS = "Fix the highlighted options first.";

interface FormProps extends Props {
    form: RundeckDeployForm;
    onRetry: () => void;
}

const NO_CHECK: RundeckRunCheck = { errors: {}, send: {}, valuesUrls: {}, running: null };

function RunForm({ paneId, level, active, form, onRetry }: FormProps) {
    const isProd = form.prod;
    const repoPath = level.repoPath ?? "";
    const branchKey = form.branchKey;
    const runsBranch = form.runsBranch;
    const runWord = form.runWord.toLowerCase();

    const [values, setValues] = useState<Record<string, string>>(() => form.values);
    const [remote, setRemote] = useState<Record<string, string[] | null>>({});
    const [advanced, setAdvanced] = useState<AdvancedRun>({ debug: false, nodeFilter: form.nodeFilter ?? "", runAt: "", asUser: "" });
    const [attempted, setAttempted] = useState(false);
    const [busy, setBusy] = useState(false);
    const [error, setError] = useState<string | null>(null);
    const check = useRunCheck(form, values, remote);

    const branch = branchKey ? (values[branchKey] ?? "").trim() : "";
    const settledBranch = useDebounced(branch, 400);
    const plan = useResourceEnabled(
        active && runsBranch && !!settledBranch,
        rndPlanV,
        `${paneId}/plan`,
        form.jobId,
        form.project,
        form.service,
        settledBranch,
        repoPath,
    );
    const planReady = !runsBranch || (plan.status === "ok" && plan.data?.targetBranch === branch);
    const hasErrors = Object.keys(check.errors).length > 0;

    useEffect(() => {
        if (!hasErrors && error === FIX_OPTIONS) setError(null);
    }, [hasErrors, error]);

    const setValue = (name: string, value: string) => setValues((prev) => ({ ...prev, [name]: value }));
    const setRemoteValues = (name: string, list: string[] | null) => setRemote((prev) => (prev[name] === list ? prev : { ...prev, [name]: list }));

    const submit = async () => {
        setAttempted(true);
        setError(null);
        if (!form.executionEnabled) return setError("Executions are disabled for this job in Rundeck.");
        if (!planReady) return setError("Wait for the deploy plan to finish.");
        const runAtTime = advanced.runAt ? localDateTimeToIso(advanced.runAt) : null;
        if (advanced.runAt && !runAtTime) return setError("Run later needs a full date and time.");

        setBusy(true);
        let pushed = false;
        try {
            const ready = await rundeckApi.checkRun({ options: form.options, values, remote, jobId: form.jobId, project: form.project });
            if (Object.keys(ready.errors).length > 0) return setError(FIX_OPTIONS);

            if (isProd) {
                const ok = await confirmDialog({
                    title: runsBranch ? `Deploy ${branch} to production?` : `Run ${form.name} in production?`,
                    body: `Project: ${form.project}\nJob: ${form.service}\nEnvironment: ${form.env}`,
                    confirmLabel: runsBranch ? "deploy to production" : "run in production",
                    destructive: true,
                });
                if (!ok) return;
            }

            if (ready.running) {
                const ok = await confirmDialog({
                    title: "A run is already in progress",
                    body: `${form.name} is already running. Start another ${runWord} anyway?`,
                    confirmLabel: `${runWord} anyway`,
                });
                if (!ok) return;
            }

            if (runsBranch && plan.data?.pushes && repoPath) {
                const checkedOut = currentBranch(await git.status(repoPath));
                if (checkedOut !== branch) {
                    setError(`The checkout is now on ${checkedOut ?? "a detached HEAD"}, not ${branch}. Nothing was pushed or run.`);
                    return;
                }
                await git.push(repoPath);
                pushed = true;
            }

            const filter = advanced.nodeFilter.trim();
            const result = await rundeckApi
                .run({
                    jobId: form.jobId,
                    options: ready.send,
                    loglevel: advanced.debug ? "DEBUG" : null,
                    filter: filter && filter !== (form.nodeFilter ?? "") ? filter : null,
                    runAtTime,
                    asUser: advanced.asUser.trim() || null,
                })
                .catch((e: unknown) => {
                    throw pushed ? new Error(`${branch} was pushed, but Rundeck rejected the run: ${errorText(e)}`) : e;
                });
            if (result.recovered) notify("info", "The run request timed out, but the run was found in Rundeck.");
            watchRun(cmd.jobRefOf(level), result.id);
            cmd.rundeckReplace(paneId, { kind: "execution", ...cmd.jobRefOf(level), executionId: result.id });
        } catch (e) {
            setError(errorText(e));
        } finally {
            setBusy(false);
        }
    };

    const canSubmit = !busy && form.executionEnabled && planReady;
    const verb = runsBranch ? (branch ? `Deploy ${branch}` : "Deploy") : "Run";
    const label = busy ? "Starting…" : isProd ? `${verb} to production` : verb;
    const usual = form.usualSecs;

    return (
        <form
            className="rnd-main"
            onSubmit={(e) => {
                e.preventDefault();
                void submit();
            }}>
            <FormHead paneId={paneId} level={level} runWord={runWord} prod={isProd} />
            <div className={`rnd-form${runsBranch ? "" : " single"}`}>
                {form.optionsError && (
                    <div className="rnd-banner muted rnd-span">
                        Couldn't read job options: {form.optionsError}. Only the branch is sent.{" "}
                        <button type="button" className="rnd-link" onClick={onRetry}>
                            retry
                        </button>
                    </div>
                )}
                {!form.executionEnabled && <div className="rnd-banner warn rnd-span">Executions are disabled for this job in Rundeck.</div>}
                {error && <div className="rnd-banner danger rnd-span">{error}</div>}

                <section className="rnd-card">
                    <div className="rnd-card-head">
                        <b>Options</b>
                        {form.description && <span>{form.description}</span>}
                    </div>
                    <div className="rnd-card-body">
                        <RundeckOptionsForm
                            paneId={paneId}
                            options={form.options}
                            values={values}
                            valuesUrls={check.valuesUrls}
                            errors={check.errors}
                            showErrors={attempted}
                            permalink={form.permalink}
                            active={active}
                            onChange={setValue}
                            onRemoteValues={setRemoteValues}
                            hint={(option) =>
                                option.name === branchKey ? (
                                    <BranchPicks
                                        recent={form.recentBranches}
                                        plan={plan.data}
                                        branch={branch}
                                        onPick={(b) => setValue(option.name, b)}
                                    />
                                ) : null
                            }
                        />
                        <RundeckAdvanced value={advanced} onChange={setAdvanced} defaultFilter={form.nodeFilter} />
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
                            This {runWord}s to <b>production</b> ({form.env}). You'll be asked to confirm.
                        </span>
                    </div>
                )}

                <div className="rnd-form-foot">
                    {usual !== null && <span className="rnd-form-usual">Usually takes {formatDuration(usual * 1000)}</span>}
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

/** What the plugin finds wrong with the form as it stands, asked again a moment after each change. */
function useRunCheck(form: RundeckDeployForm, values: Record<string, string>, remote: Record<string, string[] | null>): RundeckRunCheck {
    const asked = useDebounced(
        useMemo(() => ({ values, remote }), [values, remote]),
        150,
    );
    const [check, setCheck] = useState<RundeckRunCheck>(NO_CHECK);
    useEffect(() => {
        let current = true;
        rundeckApi
            .checkRun({ options: form.options, ...asked })
            .then((next) => current && setCheck(next))
            .catch(() => current && setCheck(NO_CHECK));
        return () => {
            current = false;
        };
    }, [form.options, asked]);
    return check;
}

/** Branches worth one click: the checkout's, the live one, and whatever ran recently. */
function BranchPicks({
    recent,
    plan,
    branch,
    onPick,
}: {
    recent: string[];
    plan: Plan | undefined;
    branch: string;
    onPick: (branch: string) => void;
}) {
    const picks: { name: string; note?: string }[] = [];
    const add = (name: string | null | undefined, note?: string) => {
        if (!name) return;
        const found = picks.find((p) => p.name === name);
        if (found) found.note ??= note;
        else picks.push({ name, note });
    };
    add(plan?.currentBranch, "checkout");
    add(plan?.deployedBranch, "live");
    for (const name of recent) {
        if (picks.length >= 6) break;
        add(name);
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
