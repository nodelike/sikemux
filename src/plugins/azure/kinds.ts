import { id, name } from "../../../src-tauri/plugins/azure/manifest.json";
import { surfaceKind } from "../../plugin-api/identity";

export const AZURE_PLUGIN = { id, name };
export const AZURE_PORTAL = surfaceKind(AZURE_PLUGIN, "portal");
