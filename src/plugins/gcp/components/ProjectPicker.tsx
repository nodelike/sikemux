import { useEffect, useRef, useState } from "react";
import { useResourceEnabled } from "../../../plugin-api/resources";
import { EmptyState, SkeletonRows } from "../../../plugin-api/ui";
import type { GcpProject } from "../api";
import { Problem } from "./Problem";
import { gcpProjectsR } from "../resources";
import { gcpSettings, setGcpProject } from "../state";

/** The project list, filtered as the person types, as the Cloud console's "Select a project" dialog shows it. */
export function ProjectList({ active, onPicked }: { active: boolean; onPicked?: () => void }) {
    const account = gcpSettings.useSelect((s) => s.account);
    const current = gcpSettings.useSelect((s) => s.project);
    const projects = useResourceEnabled(active, gcpProjectsR, account ?? "");
    const [words, setWords] = useState("");
    const input = useRef<HTMLInputElement>(null);
    useEffect(() => input.current?.focus(), []);
    const needle = words.trim().toLowerCase();
    const shown = (projects.data ?? []).filter(
        (project) => !needle || project.id.toLowerCase().includes(needle) || project.name.toLowerCase().includes(needle),
    );
    const pick = (project: GcpProject) => {
        setGcpProject(project.id);
        onPicked?.();
    };
    return (
        <div className="gcp-projects">
            <input
                ref={input}
                className="gcp-input"
                value={words}
                onChange={(event) => setWords(event.target.value)}
                onKeyDown={(event) => {
                    if (event.key === "Enter" && shown[0]) pick(shown[0]);
                }}
                placeholder="Search projects"
                aria-label="Search projects"
                spellCheck={false}
            />
            {projects.status === "error" && !projects.data ? (
                <Problem error={projects.error} what="projects" onRetry={() => void projects.refresh()} />
            ) : !projects.data ? (
                <SkeletonRows rows={5} label="Loading projects" />
            ) : shown.length === 0 ? (
                <EmptyState message={needle ? "No project matches." : "This account can see no projects."} />
            ) : (
                <div className="gcp-project-rows" role="listbox" aria-label="Projects">
                    <div className="gcp-project-row head">
                        <span>Name</span>
                        <span>ID</span>
                    </div>
                    {shown.map((project) => (
                        <button
                            key={project.id}
                            type="button"
                            role="option"
                            aria-selected={project.id === current}
                            className={`gcp-project-row${project.id === current ? " on" : ""}`}
                            onClick={() => pick(project)}>
                            <span>{project.name || project.id}</span>
                            <span className="gcp-mono">{project.id}</span>
                        </button>
                    ))}
                </div>
            )}
        </div>
    );
}

/** The top bar's project button, which opens the project list in a dialog. */
export function ProjectPicker({ active }: { active: boolean }) {
    const project = gcpSettings.useSelect((s) => s.project);
    const [open, setOpen] = useState(false);
    useEffect(() => {
        if (!open) return;
        const close = (event: KeyboardEvent) => {
            if (event.key === "Escape") setOpen(false);
        };
        window.addEventListener("keydown", close);
        return () => window.removeEventListener("keydown", close);
    }, [open]);
    return (
        <>
            <button type="button" className="gcp-project-button" aria-haspopup="dialog" aria-expanded={open} onClick={() => setOpen(true)}>
                <svg width="16" height="16" viewBox="0 0 24 24" aria-hidden="true">
                    <path
                        d="M12 2 2 7l10 5 10-5zm0 12.5L4.2 10.6 2 11.7l10 5 10-5-2.2-1.1zm0 4.5-7.8-3.9L2 16.2l10 5 10-5-2.2-1.1z"
                        fill="currentColor"
                    />
                </svg>
                <span>{project ?? "Select a project"}</span>
                <svg width="14" height="14" viewBox="0 0 24 24" aria-hidden="true">
                    <path d="m7 10 5 5 5-5z" fill="currentColor" />
                </svg>
            </button>
            {open && (
                <div className="gcp-dialog-scrim" role="presentation" onClick={() => setOpen(false)}>
                    <div className="gcp-dialog" role="dialog" aria-label="Select a project" onClick={(event) => event.stopPropagation()}>
                        <header>
                            <h2>Select a project</h2>
                            <button type="button" className="gcp-icon-button" aria-label="Close" onClick={() => setOpen(false)}>
                                ✕
                            </button>
                        </header>
                        <ProjectList active={active} onPicked={() => setOpen(false)} />
                    </div>
                </div>
            )}
        </>
    );
}
