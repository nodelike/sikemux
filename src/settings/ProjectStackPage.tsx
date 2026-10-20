import { useEffect, useState, type ReactNode } from "react";
import { coreCall } from "../api/core";
import { prettyPath } from "../lib/paths";
import { LANGUAGE_NAMES, type FolderLanguage, type FolderSettings } from "../projects/projectFolders";
import type { Drift } from "../projects/projectHealth";
import { DriftBanner } from "./ProjectOverviewPage";
import { useStore } from "../state/store";
import { Switch } from "../ui/Controls";
import { Dropdown } from "../ui/Dropdown";
import { SettingsPage, SettingsRow, SettingsRows, SettingsSection } from "./SettingsLayout";

interface JavaRuntime {
    home: string;
    major: number;
}

type SetVariable = (key: string, value: string | null) => void;

/** How one folder's code in one language is built and run, set through the variables its tools read. */
export function ProjectStackPage({
    where,
    language,
    settings,
    setVariable,
    drifts,
    onFixDrift,
}: {
    where: string;
    language: FolderLanguage;
    settings: FolderSettings | undefined;
    setVariable: SetVariable;
    drifts: readonly Drift[];
    onFixDrift: (drift: Drift) => void;
}) {
    const env = settings?.env ?? {};
    const field = { env, setVariable };
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
            {language.id === "java" && <JavaSections language={language} {...field} />}
            {language.id === "node" && <NodeSections language={language} {...field} />}
            {language.id === "go" && <GoSections language={language} {...field} />}
            {language.id === "rust" && <RustSections language={language} {...field} />}
            {language.id === "python" && <PythonSections language={language} {...field} />}
            <p className="settings-hint">
                These reach every task, terminal and agent started in {where} as environment variables, so an agent running the build tool itself gets
                the same setup. They show on the Environment page too.
            </p>
        </SettingsPage>
    );
}

interface Sections {
    language: FolderLanguage;
    env: Readonly<Record<string, string>>;
    setVariable: SetVariable;
}

function JavaSections({ language, env, setVariable }: Sections) {
    const [runtimes, setRuntimes] = useState<JavaRuntime[] | null>(null);
    const home = useStore((s) => s.home);
    useEffect(() => {
        void coreCall<{ installed: JavaRuntime[] }>("javaRuntimes")
            .then((found) => setRuntimes(found.installed))
            .catch(() => setRuntimes([]));
    }, []);
    const maven = language.markers.some((marker) => marker === "pom.xml" || marker === "mvnw");
    const asked = language.version ? `Java ${language.version}` : null;
    return (
        <>
            <SettingsSection title="Runtime">
                <SettingsRows>
                    <SettingsRow
                        label="JDK"
                        desc={
                            env.JAVA_HOME
                                ? prettyPath(env.JAVA_HOME, home)
                                : asked
                                  ? `The build asks for ${asked}; tasks pick the closest installed one.`
                                  : "The one your shell finds."
                        }
                        wide>
                        <Dropdown
                            className="settings-dd"
                            label="JDK"
                            value={env.JAVA_HOME ?? ""}
                            disabled={runtimes === null}
                            options={[
                                { value: "", label: asked ? `Automatic · ${asked}` : "Automatic" },
                                ...(runtimes ?? []).map((runtime) => ({
                                    value: runtime.home,
                                    label: `Java ${runtime.major}`,
                                    detail: prettyPath(runtime.home, home),
                                })),
                            ]}
                            onChange={(value) => setVariable("JAVA_HOME", value || null)}
                        />
                    </SettingsRow>
                </SettingsRows>
            </SettingsSection>
            <SettingsSection title="Build and run">
                <SettingsRows>
                    {maven && (
                        <TextVariable
                            label="Maven profiles"
                            desc="Every Maven run here gets them as -P, through MAVEN_ARGS (Maven 3.9 or newer)."
                            variable="MAVEN_ARGS"
                            placeholder="dev,local"
                            env={env}
                            setVariable={setVariable}
                            read={(value) => value.replace(/^-P/, "")}
                            write={(profiles) => `-P${profiles.replace(/\s+/g, "")}`}
                        />
                    )}
                    {language.framework === "spring-boot" && (
                        <TextVariable
                            label="Spring profiles"
                            desc="The profiles the app starts with, as SPRING_PROFILES_ACTIVE."
                            variable="SPRING_PROFILES_ACTIVE"
                            placeholder="local"
                            env={env}
                            setVariable={setVariable}
                        />
                    )}
                    <TextVariable
                        label="JVM options"
                        desc="Given to every java started here, Maven and Gradle included, as JDK_JAVA_OPTIONS."
                        variable="JDK_JAVA_OPTIONS"
                        placeholder="-Xmx2g"
                        env={env}
                        setVariable={setVariable}
                    />
                </SettingsRows>
            </SettingsSection>
        </>
    );
}

function NodeSections({ language, env, setVariable }: Sections) {
    return (
        <SettingsSection title="Runtime">
            <SettingsRows>
                {language.version && (
                    <SettingsRow label="Node" desc="The version the project's .nvmrc or .node-version asks for.">
                        <code className="project-file-command">{language.version}</code>
                    </SettingsRow>
                )}
                <SettingsRow label="NODE_ENV" desc="Which mode tools and frameworks run in." wide>
                    <Dropdown
                        className="settings-dd"
                        label="NODE_ENV"
                        value={env.NODE_ENV ?? ""}
                        options={[
                            { value: "", label: "Not set" },
                            ...["development", "test", "production"].map((value) => ({ value, label: value })),
                        ]}
                        onChange={(value) => setVariable("NODE_ENV", value || null)}
                    />
                </SettingsRow>
                <TextVariable
                    label="Node options"
                    desc="Given to every node started here, as NODE_OPTIONS."
                    variable="NODE_OPTIONS"
                    placeholder="--max-old-space-size=4096"
                    env={env}
                    setVariable={setVariable}
                />
            </SettingsRows>
        </SettingsSection>
    );
}

function GoSections({ language, env, setVariable }: Sections) {
    return (
        <SettingsSection title="Build and test">
            <SettingsRows>
                {language.version && (
                    <SettingsRow label="Go" desc="The version go.mod asks for.">
                        <code className="project-file-command">{language.version}</code>
                    </SettingsRow>
                )}
                <TextVariable
                    label="Go flags"
                    desc="Added to every go build, test and run here, as GOFLAGS."
                    variable="GOFLAGS"
                    placeholder="-tags=integration"
                    env={env}
                    setVariable={setVariable}
                />
                <TextVariable
                    label="Private modules"
                    desc="Fetched straight from their source rather than the public proxy, as GOPRIVATE."
                    variable="GOPRIVATE"
                    placeholder="github.com/acme/*"
                    env={env}
                    setVariable={setVariable}
                />
            </SettingsRows>
        </SettingsSection>
    );
}

function RustSections({ language, env, setVariable }: Sections) {
    return (
        <SettingsSection title="Toolchain and run">
            <SettingsRows>
                <TextVariable
                    label="Toolchain"
                    desc={`The rustup toolchain cargo uses here, as RUSTUP_TOOLCHAIN${language.version ? `; the project asks for ${language.version}` : ""}.`}
                    variable="RUSTUP_TOOLCHAIN"
                    placeholder={language.version ?? "stable"}
                    env={env}
                    setVariable={setVariable}
                />
                <TextVariable
                    label="Log filter"
                    desc="What the program logs, as RUST_LOG."
                    variable="RUST_LOG"
                    placeholder="info"
                    env={env}
                    setVariable={setVariable}
                />
                <SettingsRow
                    asLabel
                    label="Backtraces"
                    desc="Print where a panic happened, as RUST_BACKTRACE=1."
                    control={<Switch checked={env.RUST_BACKTRACE === "1"} onChange={(on) => setVariable("RUST_BACKTRACE", on ? "1" : null)} />}
                />
            </SettingsRows>
        </SettingsSection>
    );
}

function PythonSections({ language, env, setVariable }: Sections) {
    return (
        <SettingsSection title="Runtime">
            <SettingsRows>
                {language.version && (
                    <SettingsRow label="Python" desc="The version the project's .python-version asks for.">
                        <code className="project-file-command">{language.version}</code>
                    </SettingsRow>
                )}
                <TextVariable
                    label="Import folders"
                    desc="Extra folders Python imports from, as PYTHONPATH."
                    variable="PYTHONPATH"
                    placeholder="src"
                    env={env}
                    setVariable={setVariable}
                />
                <SettingsRow
                    asLabel
                    label="Unbuffered output"
                    desc="Print output as it happens, as PYTHONUNBUFFERED=1."
                    control={<Switch checked={env.PYTHONUNBUFFERED === "1"} onChange={(on) => setVariable("PYTHONUNBUFFERED", on ? "1" : null)} />}
                />
            </SettingsRows>
        </SettingsSection>
    );
}

/** A setting kept in one variable, saved when the field is left. */
function TextVariable({
    label,
    desc,
    variable,
    placeholder,
    env,
    setVariable,
    read = (value) => value,
    write = (value) => value,
}: {
    label: string;
    desc: ReactNode;
    variable: string;
    placeholder: string;
    env: Readonly<Record<string, string>>;
    setVariable: SetVariable;
    read?: (value: string) => string;
    write?: (value: string) => string;
}) {
    const saved = env[variable] === undefined ? "" : read(env[variable]);
    const [draft, setDraft] = useState<string | null>(null);
    const commit = () => {
        const next = draft?.trim();
        setDraft(null);
        if (next === undefined || next === saved) return;
        setVariable(variable, next ? write(next) : null);
    };
    return (
        <SettingsRow label={label} desc={desc} wide>
            <input
                className="settings-input mono"
                aria-label={label}
                spellCheck={false}
                placeholder={placeholder}
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
