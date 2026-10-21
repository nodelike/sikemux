import type { ProjectBoard } from "../../agents/agentBoard";
import type { Draft } from "immer";
import { mutate, type StoreState } from "../store";
import { ensureRoleWindow } from "./shared";

function keep(d: Draft<StoreState>, board: ProjectBoard): void {
    d.agentBoards[board.project] = board.cards;
    d.agentBoardAuto[board.project] = { settings: board.settings, running: board.running, autoNote: board.autoNote };
}

export const openBoard = (): void => ensureRoleWindow("board");

/** Takes the boards the core sent, which replace the app's copies of those projects. */
export function setAgentBoards(boards: readonly ProjectBoard[]): void {
    mutate((d) => {
        for (const board of boards) keep(d, board);
    });
}

/** Takes one project's board, read or changed for the project open at `folder`. */
export function setAgentBoard(folder: string, board: ProjectBoard): void {
    mutate((d) => {
        keep(d, board);
        if (board.project !== folder) d.boardProjects[folder] = board.project;
    });
}
