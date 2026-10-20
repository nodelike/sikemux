import { useState } from "react";
import { prettyPath } from "../lib/paths";
import { LANGUAGE_NAMES, type FolderLanguage, type FolderSettings, type LanguageField } from "../projects/projectFolders";
import type { Drift } from "../projects/projectHealth";
import { javaRuntimesR } from "../projects/projectResources";
import { useResource } from "../state/resources";
import { useStore } from "../state/store";
import { Switch } from "../ui/Controls";
import { Dropdown } from "../ui/Dropdown";
import { DriftBanner } from "./ProjectOverviewPage";
import { SettingsPage, SettingsRow, SettingsRows, SettingsSection } from "./SettingsLayout";

type SetVariable = (key: string, value: string | null) => void;

const NOT_SET = "";

/** Whether a field belongs on this folder's page, such as Maven profiles only when it builds with Maven. */
function applies(field: LanguageField, language: FolderLanguage): boolean {
    if (field.when === "maven") return language.markers.some((marker) => marker === "pom.xml" || marker === "mvnw");
    if (field.when === "springBoot") return language.framework === "spring-boot";
    if (field.kind === "version") return Boolean(language.version);
    return true;
}

/** How one folder's code in one language is built and run, set through the variables its tools read. */
export function ProjectStackPage({
    where,
    language,
    fields,
    settings,
    setVariable,
    drifts,
    onFixDrift,
}: {
    where: string;
    language: FolderLanguage;
    fields: readonly LanguageField[];
    settings: FolderSettings | undefined;
    setVariable: SetVariable;
    drifts: readonly Drift[];
    onFixDrift: (drift: Drift) => void;
}) {
    const env = settings?.env ?? {};
    const shown = fields.filter((field) => applies(field, language));
    const sections = [...new Set(shown.map((field) => field.section))];
    return (
        <SettingsPage>
            {drifts.map((drift) => (
                <DriftBanner key={drift.fix.key} drift={drift} onFix={() => onFixDrift(drift)} />
            ))}
            <p className="project-folder-found">
                <span className="project-folder-language">
                    {LANGUAGE_NAMES[language.id]}
                    {language.version && <span>{language.version}</span>}
                    <code>{language.markers.join(", ")}</code>
                </span>
            </p>
            {sections.map((section) => (
                <SettingsSection key={section} title={section}>
                    <SettingsRows>
                        {shown
                            .filter((field) => field.section === section)
                            .map((field) => (
                                <FieldRow key={field.label} field={field} language={language} env={env} setVariable={setVariable} />
                            ))}
                    </SettingsRows>
                </SettingsSection>
            ))}
            <p className="settings-hint">
                These reach every task, terminal and agent started in {where} as environment variables, so an agent running the build tool itself gets
                the same setup. They show on the Environment page too.
            </p>
        </SettingsPage>
    );
}

function FieldRow({
    field,
    language,
    env,
    setVariable,
}: {
    field: LanguageField;
    language: FolderLanguage;
    env: Readonly<Record<string, string>>;
    setVariable: SetVariable;
}) {
    switch (field.kind) {
        case "jdk":
            return <JdkRow field={field} language={language} env={env} setVariable={setVariable} />;
        case "version":
            return (
                <SettingsRow label={field.label} desc={field.desc}>
                    <code className="project-row-command">{language.version}</code>
                </SettingsRow>
            );
        case "toggle":
            return (
                <SettingsRow
                    asLabel
                    label={field.label}
                    desc={field.desc}
                    control={<Switch checked={env[field.variable] === "1"} onChange={(on) => setVariable(field.variable, on ? "1" : null)} />}
                />
            );
        case "choice":
            return (
                <SettingsRow label={field.label} desc={field.desc} wide>
                    <Dropdown
                        className="settings-dd"
                        label={field.label}
                        value={env[field.variable] ?? NOT_SET}
                        options={[{ value: NOT_SET, label: "Not set" }, ...(field.options ?? []).map((value) => ({ value, label: value }))]}
                        onChange={(value) => setVariable(field.variable, value || null)}
                    />
                </SettingsRow>
            );
        case "text":
            return <TextRow field={field} env={env} setVariable={setVariable} />;
    }
}

function JdkRow({
    field,
    language,
    env,
    setVariable,
}: {
    field: LanguageField;
    language: FolderLanguage;
    env: Readonly<Record<string, string>>;
    setVariable: SetVariable;
}) {
    const runtimes = useResource(javaRuntimesR).data;
    const home = useStore((s) => s.home);
    const chosen = env[field.variable];
    const asked = language.version ? `Java ${language.version}` : null;
    return (
        <SettingsRow
            label={field.label}
            desc={chosen ? prettyPath(chosen, home) : asked ? `The build asks for ${asked}; tasks pick the closest installed one.` : field.desc}
            wide>
            <Dropdown
                className="settings-dd"
                label={field.label}
                value={chosen ?? NOT_SET}
                disabled={runtimes === undefined}
                options={[
                    { value: NOT_SET, label: asked ? `Automatic · ${asked}` : "Automatic" },
                    ...(runtimes ?? []).map((runtime) => ({
                        value: runtime.home,
                        label: `Java ${runtime.major}`,
                        detail: prettyPath(runtime.home, home),
                    })),
                ]}
                onChange={(value) => setVariable(field.variable, value || null)}
            />
        </SettingsRow>
    );
}

/** A setting kept in one variable, saved when the field is left; a prefix such as `-P` is kept off what is shown. */
function TextRow({ field, env, setVariable }: { field: LanguageField; env: Readonly<Record<string, string>>; setVariable: SetVariable }) {
    const prefix = field.prefix ?? "";
    const stored = env[field.variable];
    const saved = stored === undefined ? "" : stored.startsWith(prefix) ? stored.slice(prefix.length) : stored;
    const [draft, setDraft] = useState<string | null>(null);
    const commit = () => {
        const next = draft?.trim();
        setDraft(null);
        if (next === undefined || next === saved) return;
        setVariable(field.variable, next ? `${prefix}${prefix ? next.replace(/\s+/g, "") : next}` : null);
    };
    return (
        <SettingsRow label={field.label} desc={field.desc} wide>
            <input
                className="settings-input mono"
                aria-label={field.label}
                spellCheck={false}
                placeholder={field.placeholder}
                value={draft ?? saved}
                onChange={(event) => setDraft(event.target.value)}
                onBlur={commit}
                onKeyDown={(event) => {
                    if (event.key === "Enter") event.currentTarget.blur();
                }}
            />
        </SettingsRow>
    );
}
