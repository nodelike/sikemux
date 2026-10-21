import { useEffect, useState } from "react";
import { confirmDialog, notify } from "../../../plugin-api/host";
import { useResourceEnabled } from "../../../plugin-api/resources";
import { Dropdown, SkeletonRows } from "../../../plugin-api/ui";
import { errorText } from "../../../plugin-api/format";
import { vercelApi, type SettingsChange, type VercelSettings } from "../api";
import { vercel, vercelSettingsR } from "../resources";

/** The frameworks Vercel presets most projects use, by the slug Vercel stores. */
export const FRAMEWORKS: { value: string; label: string }[] = [
    { value: "nextjs", label: "Next.js" },
    { value: "vite", label: "Vite" },
    { value: "remix", label: "Remix" },
    { value: "astro", label: "Astro" },
    { value: "sveltekit-1", label: "SvelteKit" },
    { value: "nuxtjs", label: "Nuxt" },
    { value: "create-react-app", label: "Create React App" },
    { value: "gatsby", label: "Gatsby" },
    { value: "angular", label: "Angular" },
    { value: "vue", label: "Vue.js" },
    { value: "docusaurus-2", label: "Docusaurus" },
    { value: "hugo", label: "Hugo" },
    { value: "", label: "Other" },
];

type Field = "buildCommand" | "outputDirectory" | "installCommand" | "devCommand" | "rootDirectory";

const FIELDS: { key: Field; label: string; hint: string }[] = [
    { key: "buildCommand", label: "Build Command", hint: "The framework's default, such as npm run build" },
    { key: "outputDirectory", label: "Output Directory", hint: "The framework's default, such as dist" },
    { key: "installCommand", label: "Install Command", hint: "Found from the lockfile" },
    { key: "devCommand", label: "Development Command", hint: "The framework's default" },
    { key: "rootDirectory", label: "Root Directory", hint: "The repository's root" },
];

type Draft = Record<Field | "framework" | "nodeVersion", string>;

const draftOf = (settings: VercelSettings): Draft => ({
    framework: settings.framework ?? "",
    nodeVersion: settings.nodeVersion ?? "",
    buildCommand: settings.buildCommand ?? "",
    outputDirectory: settings.outputDirectory ?? "",
    installCommand: settings.installCommand ?? "",
    devCommand: settings.devCommand ?? "",
    rootDirectory: settings.rootDirectory ?? "",
});

/** Only what the person changed, with an emptied field going back to the framework's default. */
export function changesOf(before: Draft, after: Draft): SettingsChange {
    const changes: SettingsChange = {};
    for (const key of Object.keys(after) as (keyof Draft)[]) {
        if (after[key].trim() !== before[key].trim()) changes[key] = after[key].trim() || null;
    }
    return changes;
}

/** A project's build settings, as Vercel's Build and Deployment settings page has them. */
export function BuildSettings({ active, team, project }: { active: boolean; team: string; project: string }) {
    const settings = useResourceEnabled(active, vercelSettingsR, team, project);
    const [draft, setDraft] = useState<Draft | null>(null);
    const [saving, setSaving] = useState(false);
    useEffect(() => {
        if (settings.data) setDraft(draftOf(settings.data));
    }, [settings.data]);

    if (settings.status === "error" && !settings.data) return <p className="vercel-note">{settings.error}</p>;
    if (!settings.data || !draft) return <SkeletonRows rows={6} label="Loading settings" />;
    const saved = draftOf(settings.data);
    const changes = changesOf(saved, draft);
    const changed = Object.keys(changes).length > 0;
    const set = (key: keyof Draft, value: string) => setDraft({ ...draft, [key]: value });

    const save = async () => {
        const sure = await confirmDialog({
            title: `Save ${project}'s settings?`,
            body: "They apply from the next deployment. The one serving now keeps the settings it was built with.",
            confirmLabel: "Save",
        });
        if (!sure) return;
        setSaving(true);
        try {
            await vercelApi.updateSettings(team, project, changes);
            vercel.refresh(vercelSettingsR);
            notify("success", `Saved ${project}'s settings`);
        } catch (error) {
            notify("error", `Save settings: ${errorText(error)}`);
        } finally {
            setSaving(false);
        }
    };

    return (
        <form
            className="vercel-settings vercel-form"
            aria-label="Build and Deployment Settings"
            onSubmit={(event) => {
                event.preventDefault();
                if (changed && !saving) void save();
            }}>
            <section className="vercel-box vercel-form-box">
                <h3>Framework Settings</h3>
                <label className="vercel-form-row">
                    <span>Framework Preset</span>
                    <Dropdown
                        label="Framework Preset"
                        value={draft.framework}
                        options={
                            FRAMEWORKS.some((each) => each.value === draft.framework)
                                ? FRAMEWORKS
                                : [...FRAMEWORKS, { value: draft.framework, label: draft.framework }]
                        }
                        onChange={(framework) => set("framework", framework)}
                    />
                </label>
                {FIELDS.map((field) => (
                    <label key={field.key} className="vercel-form-row">
                        <span>{field.label}</span>
                        <input
                            className="vercel-find vercel-mono"
                            value={draft[field.key]}
                            placeholder={field.hint}
                            onChange={(event) => set(field.key, event.target.value)}
                            spellCheck={false}
                        />
                    </label>
                ))}
                <label className="vercel-form-row">
                    <span>Node.js Version</span>
                    <Dropdown
                        label="Node.js Version"
                        value={draft.nodeVersion}
                        options={settings.data.nodeVersions.map((version) => ({ value: version, label: version }))}
                        onChange={(nodeVersion) => set("nodeVersion", nodeVersion)}
                    />
                </label>
            </section>
            <div className="vercel-form-foot">
                <span className="vercel-muted">Leave a command empty to use the framework's default.</span>
                <button type="button" className="vercel-button" disabled={!changed || saving} onClick={() => setDraft(saved)}>
                    Discard
                </button>
                <button type="submit" className="vercel-button primary" disabled={!changed || saving}>
                    {saving ? "Saving…" : "Save"}
                </button>
            </div>
        </form>
    );
}
