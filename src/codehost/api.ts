import { isPluginFailure } from "../plugin-api/backend";

export type * from "./types";
export { hostApi } from "./registry";

export function failureMessage(error: unknown): string {
    return isPluginFailure(error) ? error.message : String(error);
}
