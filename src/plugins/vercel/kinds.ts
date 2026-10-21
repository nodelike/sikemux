import { id, name } from "../../../src-tauri/plugins/vercel/manifest.json";
import { surfaceKind } from "../../plugin-api/identity";

export const VERCEL_PLUGIN = { id, name };
export const VERCEL_PROJECTS = surfaceKind(VERCEL_PLUGIN, "projects");
