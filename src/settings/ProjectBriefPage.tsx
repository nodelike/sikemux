import { useState } from "react";
import type { ProjectRules } from "../projects/projectFolders";
import { changeProject, projectBriefR } from "../projects/projectResources";
import { useResource } from "../state/resources";
import { reportError } from "../state/toast";
import { SettingsPage, SettingsSection } from "./SettingsLayout";

/** The brief every agent working in the project reads, written from its settings, with the person's own notes. */
export function ProjectBriefPage({ project, rules }: { project: string; rules: ProjectRules }) {
    const brief = useResource(projectBriefR, project).data;
    const [draft, setDraft] = useState<string | null>(null);
    const notes = rules.notes ?? "";

    const saveNotes = () => {
        const next = draft?.trim();
        setDraft(null);
        if (next === undefined || next === notes) return;
        void changeProject(project, { kind: "notes", text: next }).catch(reportError("Save notes for agents"));
    };

    return (
        <SettingsPage>
            <SettingsSection
                title="What agents know"
                sub="Every agent working here reads this in workspace_inspect before it builds, runs or tests anything, so it uses your JDK, profiles and tasks instead of guessing. Secret values are never in it.">
                <div className="project-card">
                    <pre className="project-brief">
                        {brief === undefined ? "…" : brief || "Nothing set up yet. Settings on the other pages show up here."}
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
