import { useEffect, useState } from "react";
import {
    folderName,
    loadProjectBrief,
    saveFolderSettings,
    saveProjectRules,
    type FolderSettings,
    type ProjectFolder,
    type ProjectRules,
} from "../projects/projectFolders";
import { answerSuggestion, describeChange, isPending, setupRequest, type Suggestion } from "../projects/projectSuggestions";
import { SendToAgentMenu } from "../agents/SendToAgentMenu";
import { useStore } from "../state/store";
import { reportError } from "../state/toast";
import { useProjectTasks, type ProjectTask } from "../tasks/coreTasks";
import { Checkbox, Switch } from "../ui/Controls";
import { Dropdown } from "../ui/Dropdown";
import { SettingsPage, SettingsRow, SettingsRows, SettingsSection } from "./SettingsLayout";

const folderOf = (task: ProjectTask) => (task.cwd === "." ? "." : task.cwd.split("/")[0]);

/** What an agent's work must pass before it says it is finished, chosen per folder from its tasks. */
export function ProjectDonePage({
    project,
    name,
    settings,
    rules,
    onChanged,
}: {
    project: string;
    name: string;
    settings: Readonly<Record<string, FolderSettings>>;
    rules: ProjectRules;
    onChanged: () => void;
}) {
    const tasks = (useProjectTasks(project)?.tasks ?? []).filter((task) => task.source !== "rerun" && task.source !== "command");
    const folders = [...new Set(tasks.map(folderOf))];

    const toggle = (folder: string, id: string, on: boolean) => {
        const current = settings[folder] ?? {};
        const done = on ? [...(current.done ?? []), id] : (current.done ?? []).filter((kept) => kept !== id);
        void saveFolderSettings(project, folder, { ...current, done })
            .then(onChanged)
            .catch(reportError("Save Done means"));
    };
    const setRules = (change: ProjectRules) =>
        void saveProjectRules(project, { ...rules, ...change })
            .then(onChanged)
            .catch(reportError("Save Done means"));

    return (
        <SettingsPage>
            <SettingsSection
                title="Done means"
                sub="The tasks an agent's work must pass before it says it is finished. Agents start them themselves with task_start; tick the ones that prove a change works.">
                {folders.length === 0 ? (
                    <p className="settings-hint">This project has no tasks yet. Add some on the Tasks page first.</p>
                ) : (
                    folders.map((folder) => (
                        <div key={folder} className="project-file">
                            <div className="project-file-group-head">
                                <span>{folderName(folder, name)}</span>
                                <span className="project-file-count">{settings[folder]?.done?.length ?? 0}</span>
                            </div>
                            {tasks
                                .filter((task) => folderOf(task) === folder)
                                .map((task) => (
                                    <div key={task.id} className="project-done-task">
                                        <Checkbox
                                            checked={settings[folder]?.done?.includes(task.id) ?? false}
                                            onChange={(on) => toggle(folder, task.id, on)}>
                                            {task.label}
                                        </Checkbox>
                                        <code className="project-file-command">{task.command}</code>
                                    </div>
                                ))}
                        </div>
                    ))
                )}
            </SettingsSection>
            <SettingsSection title="How agents check">
                <SettingsRows>
                    <SettingsRow
                        asLabel
                        label="Only the folders they changed"
                        desc="An agent that only touched one folder runs that folder's checks."
                        control={<Switch checked={!rules.checkEveryFolder} onChange={(on) => setRules({ checkEveryFolder: !on })} />}
                    />
                    <SettingsRow label="When a check fails" wide>
                        <Dropdown
                            className="settings-dd"
                            label="When a check fails"
                            value={rules.stopOnFailure ? "stop" : "fix"}
                            options={[
                                { value: "fix", label: "The agent fixes it, then tells you" },
                                { value: "stop", label: "The agent stops and tells you" },
                            ]}
                            onChange={(value) => setRules({ stopOnFailure: value === "stop" })}
                        />
                    </SettingsRow>
                </SettingsRows>
            </SettingsSection>
        </SettingsPage>
    );
}

/** The brief every agent working in the project reads, written from its settings, with the person's own notes. */
export function ProjectBriefPage({
    project,
    folders,
    settings,
    rules,
    onChanged,
}: {
    project: string;
    folders: readonly ProjectFolder[];
    settings: Readonly<Record<string, FolderSettings>>;
    rules: ProjectRules;
    onChanged: () => void;
}) {
    const [brief, setBrief] = useState<string | null>(null);
    const [draft, setDraft] = useState<string | null>(null);
    const notes = rules.notes ?? "";
    useEffect(() => {
        let cancelled = false;
        void loadProjectBrief(project)
            .then((text) => {
                if (!cancelled) setBrief(text);
            })
            .catch(() => {});
        return () => {
            cancelled = true;
        };
    }, [project, folders, settings, rules]);

    const saveNotes = () => {
        const next = draft?.trim();
        setDraft(null);
        if (next === undefined || next === notes) return;
        void saveProjectRules(project, { ...rules, notes: next })
            .then(onChanged)
            .catch(reportError("Save notes for agents"));
    };

    return (
        <SettingsPage>
            <SettingsSection
                title="What agents know"
                sub="Every agent working here reads this in workspace_inspect before it builds, runs or tests anything, so it uses your JDK, profiles and tasks instead of guessing. Secret values are never in it.">
                <div className="project-file">
                    <pre className="project-brief">
                        {brief === null ? "…" : brief || "Nothing set up yet. Settings on the other pages show up here."}
                    </pre>
                </div>
            </SettingsSection>
            <SettingsSection
                title="Your notes for agents"
                sub="Added to the end of the brief. Agents also read the project's own AGENTS.md or CLAUDE.md.">
                <textarea
                    className="settings-input project-notes"
                    aria-label="Notes for agents"
                    placeholder="Use the money helpers in api/util/Money.java; never call Stripe from tests."
                    value={draft ?? notes}
                    onChange={(event) => setDraft(event.target.value)}
                    onBlur={saveNotes}
                />
            </SettingsSection>
        </SettingsPage>
    );
}

/** What agents suggested for the project's settings, applied change by change, and a way to have one set the project up. */
export function ProjectSuggestionsPage({
    project,
    name,
    suggestions,
    onChanged,
}: {
    project: string;
    name: string;
    suggestions: readonly Suggestion[];
    onChanged: () => void;
}) {
    const sessionId = useStore((s) => s.activeSessionId);
    const [asking, setAsking] = useState<{ x: number; y: number } | null>(null);
    const pending = suggestions.filter(isPending).reverse();
    const settled = suggestions.filter((suggestion) => !isPending(suggestion)).reverse();
    return (
        <SettingsPage>
            {pending.map((suggestion) => (
                <PendingSuggestion key={suggestion.id} project={project} suggestion={suggestion} onAnswered={onChanged} />
            ))}
            <SettingsSection title="Set up with an agent">
                <SettingsRows>
                    <SettingsRow
                        label="Set up this project"
                        desc="An agent reads the repository, runs the build and the tests, and suggests a full setup here. Nothing changes until you apply it."
                        control={
                            <button
                                type="button"
                                className={`settings-btn${pending.length ? "" : " primary"}`}
                                onClick={(event) => {
                                    const rect = event.currentTarget.getBoundingClientRect();
                                    setAsking({ x: rect.right, y: rect.bottom + 4 });
                                }}>
                                Choose an agent…
                            </button>
                        }
                    />
                </SettingsRows>
            </SettingsSection>
            {settled.length > 0 && (
                <SettingsSection title="Earlier">
                    <div className="project-file">
                        {settled.map((suggestion) => {
                            const applied = suggestion.changes.filter((change) => change.status === "applied").length;
                            return (
                                <div key={suggestion.id} className="project-file-item">
                                    <span className="project-file-name">{suggestion.by}</span>
                                    <span className="project-file-command">{suggestion.title}</span>
                                    <span className="project-file-note">
                                        {applied ? `applied ${applied} of ${suggestion.changes.length}` : "discarded"} · {ago(suggestion.at)}
                                    </span>
                                </div>
                            );
                        })}
                    </div>
                </SettingsSection>
            )}
            {asking && (
                <SendToAgentMenu
                    x={asking.x}
                    y={asking.y}
                    sessionId={sessionId}
                    alignRight
                    delivery={() => ({ text: setupRequest(name) })}
                    onClose={() => setAsking(null)}
                />
            )}
        </SettingsPage>
    );
}

function PendingSuggestion({ project, suggestion, onAnswered }: { project: string; suggestion: Suggestion; onAnswered: () => void }) {
    const open = suggestion.changes.filter((change) => change.status === "pending");
    const [picked, setPicked] = useState<ReadonlySet<string>>(() => new Set(open.map((change) => change.id)));
    const [saving, setSaving] = useState(false);
    const answer = (apply: readonly string[]) => {
        setSaving(true);
        void answerSuggestion(project, suggestion.id, apply)
            .then(onAnswered)
            .catch(reportError("Apply the suggestion"))
            .finally(() => setSaving(false));
    };
    return (
        <SettingsSection
            title={`${suggestion.by} suggests ${open.length} ${open.length === 1 ? "change" : "changes"}`}
            meta={ago(suggestion.at)}
            sub={suggestion.title}>
            <div className="project-file">
                {open.map((change) => (
                    <div key={change.id} className="project-suggestion">
                        <Checkbox
                            label={describeChange(change)}
                            checked={picked.has(change.id)}
                            onChange={(on) =>
                                setPicked((current) => {
                                    const next = new Set(current);
                                    if (on) next.add(change.id);
                                    else next.delete(change.id);
                                    return next;
                                })
                            }
                        />
                        <div className="project-suggestion-text">
                            <code className="project-suggestion-change">{describeChange(change)}</code>
                            <span className="project-file-note">{change.why}</span>
                        </div>
                        <span className="project-suggestion-evidence">{change.evidence}</span>
                    </div>
                ))}
                <div className="project-file-foot">
                    <span className="settings-hint">Nothing changes until you apply. Tasks are added for you only.</span>
                    <button type="button" className="settings-btn" disabled={saving} onClick={() => answer([])}>
                        Discard
                    </button>
                    <button type="button" className="settings-btn primary" disabled={saving || picked.size === 0} onClick={() => answer([...picked])}>
                        Apply {picked.size}
                    </button>
                </div>
            </div>
        </SettingsSection>
    );
}

function ago(at: number): string {
    const minutes = Math.max(0, Math.round((Date.now() - at) / 60_000));
    if (minutes < 1) return "just now";
    if (minutes < 60) return `${minutes}m ago`;
    const hours = Math.round(minutes / 60);
    return hours < 24 ? `${hours}h ago` : `${Math.round(hours / 24)}d ago`;
}

type GuardrailKey = "neverEdit" | "askBefore" | "neverRun";

const GUARDRAILS: readonly { key: GuardrailKey; label: string; desc: string; placeholder: string }[] = [
    {
        key: "neverEdit",
        label: "Never edit",
        desc: "Paths, as globs, an agent asks you about before changing. Sikemux's git_commit refuses commits that touch them.",
        placeholder: "db/migrations/**",
    },
    {
        key: "askBefore",
        label: "Ask before running",
        desc: "Commands an agent asks you about first. Sikemux refuses to run them for an agent until it has.",
        placeholder: "terraform apply",
    },
    {
        key: "neverRun",
        label: "Never run",
        desc: "Commands agents never run. Sikemux refuses to run them for an agent. * stands for any text.",
        placeholder: "kubectl * --context prod*",
    },
];

/** How agents may change the project: paths they leave alone and commands they ask about or never run. */
export function ProjectGuardrailsPage({ project, rules, onChanged }: { project: string; rules: ProjectRules; onChanged: () => void }) {
    const [drafts, setDrafts] = useState<Partial<Record<GuardrailKey, string>>>({});
    const save = (key: GuardrailKey, next: readonly string[]) =>
        void saveProjectRules(project, { ...rules, [key]: next })
            .then(onChanged)
            .catch(reportError("Save the guardrails"));
    const add = (key: GuardrailKey) => {
        const value = drafts[key]?.trim();
        if (!value) return;
        const current = rules[key] ?? [];
        if (!current.includes(value)) save(key, [...current, value]);
        setDrafts((now) => ({ ...now, [key]: "" }));
    };
    return (
        <SettingsPage>
            <SettingsSection
                title="Guardrails"
                sub="How agents may change this project, whichever agent or account runs. Sikemux enforces them where it runs a command or commits for an agent; an agent's own shell commands follow them from its brief.">
                <SettingsRows>
                    {GUARDRAILS.map((guardrail) => (
                        <SettingsRow key={guardrail.key} label={guardrail.label} desc={guardrail.desc} stack>
                            <div className="project-guardrail">
                                {(rules[guardrail.key] ?? []).map((pattern) => (
                                    <span key={pattern} className="project-guardrail-chip">
                                        <code>{pattern}</code>
                                        <button
                                            type="button"
                                            aria-label={`Remove ${pattern}`}
                                            onClick={() =>
                                                save(
                                                    guardrail.key,
                                                    (rules[guardrail.key] ?? []).filter((kept) => kept !== pattern),
                                                )
                                            }>
                                            ×
                                        </button>
                                    </span>
                                ))}
                                <form
                                    className="project-guardrail-add"
                                    onSubmit={(event) => {
                                        event.preventDefault();
                                        add(guardrail.key);
                                    }}>
                                    <input
                                        className="settings-input mono"
                                        aria-label={`Add to ${guardrail.label}`}
                                        placeholder={guardrail.placeholder}
                                        spellCheck={false}
                                        value={drafts[guardrail.key] ?? ""}
                                        onChange={(event) => setDrafts((now) => ({ ...now, [guardrail.key]: event.target.value }))}
                                    />
                                    <button type="submit" className="settings-btn" disabled={!drafts[guardrail.key]?.trim()}>
                                        Add
                                    </button>
                                </form>
                            </div>
                        </SettingsRow>
                    ))}
                    <SettingsRow
                        label="Production environments"
                        desc="An environment that looks like production never reaches an agent, whatever is chosen on the Environment page."
                        control={<Switch checked disabled onChange={() => {}} label="Production environments never reach agents" />}
                    />
                </SettingsRows>
            </SettingsSection>
        </SettingsPage>
    );
}
