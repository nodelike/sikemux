import { id, name } from "../../../src-tauri/plugins/database/manifest.json";
import { surfaceKind } from "../../plugin-api/identity";

export const DATABASE_PLUGIN = { id, name };
export const DATABASE_BROWSER = surfaceKind(DATABASE_PLUGIN, "browser");
