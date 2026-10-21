import { id, name } from "../../../src-tauri/plugins/bruno/manifest.json";
import { surfaceKind } from "../../plugin-api/identity";

export const BRUNO_PLUGIN = { id, name };
export const BRUNO_CLIENT = surfaceKind(BRUNO_PLUGIN, "client");
