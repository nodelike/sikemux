import { useState } from "react";
import { SendToAgentMenu } from "../agents/SendToAgentMenu";
import { answerSuggestion, describeChange, isPending, setupRequest, type Suggestion } from "../projects/projectSuggestions";
import { useStore } from "../state/store";
import { reportError } from "../state/toast";
import { Checkbox } from "../ui/Controls";
import { SettingsPage, SettingsRow, SettingsRows, SettingsSection } from "./SettingsLayout";

/** What agents suggested for the project's settings, applied change by change, and a way to have one set the project up. */
export function ProjectSuggestionsPage({ project, name, suggestions }: { project: string; name: string; suggestions: readonly Suggestion[] }) {
    const sessionId = useStore((s) => s.activeSessionId);
    const [asking, setAsking] = useState<{ x: number; y: number } | null>(null);
    const pending = suggestions.filter(isPending).reverse();
    const settled = suggestions.filter((suggestion) => !isPending(suggestion)).reverse();
    return (
        <SettingsPage>
            {pending.map((suggestion) => (
                <PendingSuggestion key={suggestion.id} project={project} suggestion={suggestion} />
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
                    <div className="project-card">
                        {settled.map((suggestion) => {
                            const applied = suggestion.changes.filter((change) => change.status === "applied").length;
                            return (
                                <div key={suggestion.id} className="project-row">
                                    <span className="project-row-name">{suggestion.by}</span>
                                    <span className="project-row-command">{suggestion.title}</span>
                                    <span className="project-row-note">
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

function PendingSuggestion({ project, suggestion }: { project: string; suggestion: Suggestion }) {
    const open = suggestion.changes.filter((change) => change.status === "pending");
    const [picked, setPicked] = useState<ReadonlySet<string>>(() => new Set(open.map((change) => change.id)));
    const [saving, setSaving] = useState(false);
    const answer = (apply: readonly string[]) => {
        setSaving(true);
        void answerSuggestion(project, suggestion.id, apply)
            .catch(reportError("Apply the suggestion"))
            .finally(() => setSaving(false));
    };
    return (
        <SettingsSection
            title={`${suggestion.by} suggests ${open.length} ${open.length === 1 ? "change" : "changes"}`}
            meta={ago(suggestion.at)}
            sub={suggestion.title}>
            <div className="project-card">
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
                            <span className="project-row-note">{change.why}</span>
                        </div>
                        <span className="project-suggestion-evidence">{change.evidence}</span>
                    </div>
                ))}
                <div className="project-card-foot">
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
