import type { CardChanges, NewCard, ProjectBoard, SettingsChanges } from "../agents/agentBoard";
import { invokeCommand as invoke } from "./invoke";

/** Sent by the window whenever boards change, with the boards that did. */
export const AGENT_BOARD_EVENT = "agent_board";

const board = <T>(method: string, params: unknown) => invoke<T>("agent_board_call", { method, params });

export const agentBoardApi = {
    all: () => board<ProjectBoard[]>("boards", null),
    get: (project: string) => board<ProjectBoard>("board", { project }),
    add: (project: string, card: NewCard) => board<ProjectBoard>("boardAdd", { project, ...card }),
    update: (project: string, id: number, changes: CardChanges) => board<ProjectBoard>("boardUpdate", { project, id, ...changes }),
    remove: (project: string, id: number) => board<ProjectBoard>("boardRemove", { project, id }),
    settings: (project: string, changes: SettingsChanges) => board<ProjectBoard>("boardSettings", { project, ...changes }),
    retry: (project: string, id: number) => board<ProjectBoard>("boardRetry", { project, id }),
};
