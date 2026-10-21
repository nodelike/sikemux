import { id, name } from "../../../src-tauri/plugins/aws/manifest.json";
import { surfaceKind } from "../../plugin-api/identity";

export const AWS_PLUGIN = { id, name };
export const AWS_CONSOLE = surfaceKind(AWS_PLUGIN, "console");
