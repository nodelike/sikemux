import { useCallback, useEffect, useState } from "react";
import { errorText } from "@shared/lib/format";
import { dialogsApi } from "../api/dialogs";
import { fsapi } from "../api/fs";
import { skillsApi, type Skill, type SkillsOverview } from "../api/skills";
import { reportError } from "../state/toast";
import { SettingsRow, SettingsRows, SettingsSection } from "./SettingsLayout";

type Form = "create" | "install" | null;

export function SkillsSection() {
    const [overview, setOverview] = useState<SkillsOverview | null>(null);
    const [form, setForm] = useState<Form>(null);
    const [name, setName] = useState("");
    const [description, setDescription] = useState("");
    const [body, setBody] = useState("");
    const [source, setSource] = useState("");
    const [subfolder, setSubfolder] = useState("");
    const [busy, setBusy] = useState(false);
    /* What failed, and on which row: an open form, or one skill's Remove or Move. */
    const [failure, setFailure] = useState<{ at: string; message: string } | null>(null);
    const [moving, setMoving] = useState<string | null>(null);

    const refresh = useCallback(() => skillsApi.overview().then(setOverview).catch(reportError("Skills")), []);
    useEffect(() => {
        void refresh();
    }, [refresh]);

    const show = (next: Form) => {
        setForm(next);
        setFailure(null);
        setName("");
        setDescription("");
        setBody("");
        setSource("");
        setSubfolder("");
    };

    const run = async (at: string, work: () => Promise<unknown>, after: () => void = () => show(null)) => {
        setBusy(true);
        setFailure(null);
        try {
            await work();
            after();
            await refresh();
        } catch (error) {
            setFailure({ at, message: errorText(error) });
        } finally {
            setBusy(false);
        }
    };
    const failed = (at: string) => (failure?.at === at ? <span className="skills-failure">{failure.message}</span> : null);

    const chooseFolder = async () => {
        const picked = await dialogsApi.pickFolder();
        if (picked) setSource(picked);
    };

    const skills = overview?.skills ?? [];
    const clashes = new Set(overview?.clashes ?? []);
    const reveal = (skill: Skill) => void fsapi.revealInFinder(skill.path).catch(reportError("Show in Finder"));

    return (
        <>
            <SettingsSection
                title="Skills"
                meta={skills.length ? `${skills.length} in Sikemux` : undefined}
                sub="Skills every agent Sikemux starts can use, in a terminal or a chat. They live only in Sikemux: the same agents run elsewhere never see them. Agents can make them too, with the skills tool. A new skill loads in the next session.">
                <SettingsRows>
                    {skills.map((skill) => (
                        <SettingsRow
                            key={skill.name}
                            label={skill.name}
                            desc={
                                failed(`remove:${skill.name}`) ??
                                (clashes.has(skill.name) ? (
                                    <>
                                        {skill.description} <span className="skills-clash">One of your own skills has this name too.</span>
                                    </>
                                ) : (
                                    skill.description
                                ))
                            }>
                            <button className="settings-btn" type="button" onClick={() => reveal(skill)}>
                                Show in Finder
                            </button>
                            <button
                                className="settings-btn danger"
                                type="button"
                                disabled={busy}
                                aria-label={`Remove ${skill.name}`}
                                onClick={() =>
                                    void run(
                                        `remove:${skill.name}`,
                                        () => skillsApi.remove(skill.name),
                                        () => undefined,
                                    )
                                }>
                                Remove
                            </button>
                        </SettingsRow>
                    ))}
                    {form === "create" ? (
                        <SettingsRow label="New skill" wide desc={failed("form")}>
                            <form
                                className="skills-form"
                                onSubmit={(event) => {
                                    event.preventDefault();
                                    void run("form", () => skillsApi.create(name.trim(), description, body));
                                }}>
                                <input
                                    className="settings-input mono"
                                    aria-label="Skill name"
                                    placeholder="deploy-preview"
                                    spellCheck={false}
                                    autoFocus
                                    value={name}
                                    onChange={(event) => setName(event.target.value)}
                                />
                                <input
                                    className="settings-input"
                                    aria-label="What the skill does and when to use it"
                                    placeholder="What it does and when an agent should use it"
                                    value={description}
                                    onChange={(event) => setDescription(event.target.value)}
                                />
                                <textarea
                                    className="settings-input skills-body"
                                    aria-label="Instructions"
                                    placeholder="The instructions, in markdown"
                                    value={body}
                                    onChange={(event) => setBody(event.target.value)}
                                />
                                <span className="skills-actions">
                                    <button className="settings-btn" type="button" disabled={busy} onClick={() => show(null)}>
                                        Cancel
                                    </button>
                                    <button className="settings-btn primary" type="submit" disabled={busy || !name.trim() || !description.trim()}>
                                        Create
                                    </button>
                                </span>
                            </form>
                        </SettingsRow>
                    ) : form === "install" ? (
                        <SettingsRow label="Install a skill" wide desc={failed("form")}>
                            <form
                                className="skills-form"
                                onSubmit={(event) => {
                                    event.preventDefault();
                                    void run("form", () => skillsApi.install(source.trim(), subfolder.trim()));
                                }}>
                                <span className="skills-actions">
                                    <input
                                        className="settings-input mono"
                                        aria-label="Folder or Git URL"
                                        placeholder="A folder with a SKILL.md, or a Git URL"
                                        spellCheck={false}
                                        autoFocus
                                        value={source}
                                        onChange={(event) => setSource(event.target.value)}
                                    />
                                    <button className="settings-btn" type="button" onClick={() => void chooseFolder()}>
                                        Choose folder
                                    </button>
                                </span>
                                {!source.trim().startsWith("/") && source.trim() && (
                                    <input
                                        className="settings-input mono"
                                        aria-label="Folder inside the repository"
                                        placeholder="Folder inside the repository, if the skill is not at its root"
                                        spellCheck={false}
                                        value={subfolder}
                                        onChange={(event) => setSubfolder(event.target.value)}
                                    />
                                )}
                                <span className="skills-actions">
                                    <button className="settings-btn" type="button" disabled={busy} onClick={() => show(null)}>
                                        Cancel
                                    </button>
                                    <button
                                        className="settings-btn primary"
                                        type="submit"
                                        disabled={busy || !source.trim()}
                                        aria-busy={busy || undefined}>
                                        {busy ? "Installing…" : "Install"}
                                    </button>
                                </span>
                            </form>
                        </SettingsRow>
                    ) : (
                        <SettingsRow
                            label="Add a skill"
                            desc={overview && (skills.length ? `Kept in ${overview.library}` : `None yet. They are kept in ${overview.library}`)}>
                            <button className="settings-btn" type="button" onClick={() => show("install")}>
                                Install…
                            </button>
                            <button className="settings-btn primary" type="button" onClick={() => show("create")}>
                                New skill
                            </button>
                        </SettingsRow>
                    )}
                </SettingsRows>
            </SettingsSection>
            {(overview?.own.length ?? 0) > 0 && (
                <SettingsSection
                    title="Your own skills"
                    sub="Skills in each agent's global folder. Agents load these inside and outside Sikemux, and Sikemux leaves them alone unless you move one in.">
                    <SettingsRows>
                        {overview?.own.flatMap((own) =>
                            own.skills.map((skill) => (
                                <SettingsRow
                                    key={skill.path}
                                    label={skill.name}
                                    desc={
                                        failed(`move:${skill.path}`) ??
                                        (moving === skill.path
                                            ? `Moves it out of ${own.folder}, so ${own.agent} outside Sikemux no longer has it.`
                                            : `${own.agent} · ${skill.description}`)
                                    }>
                                    {moving === skill.path ? (
                                        <>
                                            <button className="settings-btn" type="button" onClick={() => setMoving(null)}>
                                                Cancel
                                            </button>
                                            <button
                                                className="settings-btn primary"
                                                type="button"
                                                disabled={busy}
                                                onClick={() =>
                                                    void run(
                                                        `move:${skill.path}`,
                                                        () => skillsApi.adopt(skill.path),
                                                        () => setMoving(null),
                                                    )
                                                }>
                                                Move
                                            </button>
                                        </>
                                    ) : (
                                        <button
                                            className="settings-btn"
                                            type="button"
                                            aria-label={`Move ${skill.name} into Sikemux`}
                                            onClick={() => setMoving(skill.path)}>
                                            Move into Sikemux
                                        </button>
                                    )}
                                </SettingsRow>
                            )),
                        )}
                    </SettingsRows>
                </SettingsSection>
            )}
        </>
    );
}
