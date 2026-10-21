import { id, name } from "../../../src-tauri/plugins/linear/manifest.json";
import { surfaceKind } from "../../plugin-api/identity";

export const LINEAR_PLUGIN = { id, name };
export const LINEAR_ISSUES = surfaceKind(LINEAR_PLUGIN, "issues");
