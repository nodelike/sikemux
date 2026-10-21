import type { KeybindingActionId } from "../commands/keybindings";
import * as cmd from "../state/commands";
import { CORE_WINDOW_ROLES } from "../state/windowRoles";

export type ProjectSurfaceRole = "files" | "git" | "search" | "board";

export interface ProjectSurface {
    role: ProjectSurfaceRole;
    label: string;
    shortcut: KeybindingActionId;
    open: () => void;
}

/** A project's own surfaces, named, opened and bound the same way from the tab strip, the rail and the new tab picker. */
export const PROJECT_SURFACES: Record<ProjectSurfaceRole, ProjectSurface> = {
    files: { role: "files", label: CORE_WINDOW_ROLES.files, shortcut: "window.files", open: () => cmd.openEditorPane() },
    git: { role: "git", label: CORE_WINDOW_ROLES.git, shortcut: "window.git", open: () => cmd.openGitWorkbench() },
    search: { role: "search", label: CORE_WINDOW_ROLES.search, shortcut: "search.global", open: () => cmd.focusGlobalSearch() },
    board: { role: "board", label: CORE_WINDOW_ROLES.board, shortcut: "window.board", open: () => cmd.openBoard() },
};

export const PROJECT_SURFACE_ORDER: readonly ProjectSurfaceRole[] = ["files", "git", "search", "board"];
