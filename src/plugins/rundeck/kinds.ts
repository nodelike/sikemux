import { id, name } from "../../../src-tauri/plugins/rundeck/manifest.json";
import { surfaceKind } from "../../plugin-api/identity";

export const RUNDECK_PLUGIN = { id, name };
export const RUNDECK_DEPLOY = surfaceKind(RUNDECK_PLUGIN, "deploy");
