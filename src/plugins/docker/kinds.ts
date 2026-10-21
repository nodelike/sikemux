import { id, name } from "../../../src-tauri/plugins/docker/manifest.json";
import { surfaceKind } from "../../plugin-api/identity";

export const DOCKER_PLUGIN = { id, name };
export const DOCKER_ENGINE = surfaceKind(DOCKER_PLUGIN, "engine");
