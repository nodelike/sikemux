import { id, name } from "../../../src-tauri/plugins/gcp/manifest.json";
import { surfaceKind } from "../../plugin-api/identity";

export const GCP_PLUGIN = { id, name };
export const GCP_CONSOLE = surfaceKind(GCP_PLUGIN, "console");
