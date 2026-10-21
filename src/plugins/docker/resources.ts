import { pluginResources } from "../../plugin-api/service";
import { dockerApi } from "./api";
import { DOCKER_PLUGIN } from "./kinds";

export const docker = pluginResources(DOCKER_PLUGIN);

export const dockerStatusR = docker.resource("status", () => dockerApi.status(), 10_000);

export const dockerContainersR = docker.resource("containers", () => dockerApi.containers(), 5_000);

export const dockerContainerR = docker.resource("container", (id: string) => dockerApi.container(id), 5_000);

export const dockerStatsR = docker.resource("stats", () => dockerApi.stats(), 3_000);

export const dockerStacksR = docker.resource("stacks", () => dockerApi.stacks(), 5_000);

export const dockerImagesR = docker.resource("images", () => dockerApi.images(), 30_000);

export const dockerVolumesR = docker.resource("volumes", () => dockerApi.volumes(), 30_000);

export const dockerDiskR = docker.resource("disk", () => dockerApi.disk(), 60_000);
