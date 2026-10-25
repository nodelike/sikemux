import type { DiffHunk } from "@shared/wire.generated";
import { invokeCommand as invoke } from "./invoke";

export type { DiffHunk } from "@shared/wire.generated";

export const diffApi = {
    hunks: (baseline: string, current: string) => invoke<DiffHunk[]>("diff_hunks", { baseline, current }),
};
