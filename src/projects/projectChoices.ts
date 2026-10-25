import { basename, expandHome, normalizePath, prettyPath, relativePath } from "@shared/lib/paths";
import type { ProjectRoot, Session } from "../state/types";

export interface ProjectChoice {
    path: string;
    name: string;
    sub: string;
    /** The open project's session, or null for a folder found on disk. */
    sessionId: string | null;
}

/**
 * What a project is called in a picker: the folder's name when it sits under
 * a project root, or the session's own name for a root opened as a project.
 */
export function projectLabel(cwd: string, fallback: string, roots: readonly ProjectRoot[], home: string | null): string {
    const paths = roots
        .map((root) => normalizePath(expandHome(root.path, home)))
        .filter(Boolean)
        .sort((a, b) => b.length - a.length);
    for (const root of paths) {
        if (relativePath(cwd, root) === "") return fallback;
        if (relativePath(cwd, root) !== null) return basename(cwd);
    }
    return fallback;
}

/** The open projects, then the folders found under the project roots that are not open yet. */
export function projectChoices(
    sessions: readonly (Session | undefined)[],
    found: readonly { path: string }[],
    roots: readonly ProjectRoot[],
    home: string | null,
): ProjectChoice[] {
    const open: ProjectChoice[] = sessions.flatMap((session) =>
        session?.kind === "project"
            ? [
                  {
                      path: session.cwd,
                      name: projectLabel(session.cwd, session.name, roots, home),
                      sub: prettyPath(session.cwd, home),
                      sessionId: session.id,
                  },
              ]
            : [],
    );
    const notOpen = found.filter((entry) => !open.some((choice) => relativePath(entry.path, choice.path) === ""));
    return [
        ...open,
        ...notOpen.map((entry) => ({ path: entry.path, name: basename(entry.path), sub: prettyPath(entry.path, home), sessionId: null })),
    ];
}
