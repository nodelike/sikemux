import { id, name } from "../../../src-tauri/plugins/signoz/manifest.json";
import { surfaceKind } from "../../plugin-api/identity";

export const SIGNOZ_PLUGIN = { id, name };
export const SIGNOZ_EXPLORE = surfaceKind(SIGNOZ_PLUGIN, "explore");
