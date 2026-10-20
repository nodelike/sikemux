import { useState } from "react";
import type { ProjectRules } from "../projects/projectFolders";
import { changeProject, type Guardrail } from "../projects/projectResources";
import { reportError } from "../state/toast";
import { Switch } from "../ui/Controls";
import { SettingsPage, SettingsRow, SettingsRows, SettingsSection } from "./SettingsLayout";

const GUARDRAILS: readonly { key: Guardrail; label: string; desc: string; placeholder: string }[] = [
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
export function ProjectGuardrailsPage({ project, rules }: { project: string; rules: ProjectRules }) {
    const [drafts, setDrafts] = useState<Partial<Record<Guardrail, string>>>({});
    const save = (list: Guardrail, pattern: string, on: boolean) =>
        void changeProject(project, { kind: "guardrail", list, pattern, on }).catch(reportError("Save the guardrails"));
    const add = (list: Guardrail) => {
        const pattern = drafts[list]?.trim();
        if (!pattern) return;
        save(list, pattern, true);
        setDrafts((now) => ({ ...now, [list]: "" }));
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
                                        <button type="button" aria-label={`Remove ${pattern}`} onClick={() => save(guardrail.key, pattern, false)}>
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
