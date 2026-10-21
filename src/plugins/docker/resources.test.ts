import { describe, expect, it } from "vitest";
import { resourceKind } from "../../plugin-api/identity";
import { DOCKER_PLUGIN } from "./kinds";
import { docker, dockerContainersR } from "./resources";

describe("the Docker plugin's cached reads", () => {
    it("names every read under the plugin, so one refresh reaches them all", () => {
        expect(dockerContainersR.kind).toBe(resourceKind(DOCKER_PLUGIN.id, "containers"));
        expect(docker.owns(dockerContainersR.kind)).toBe(true);
        expect(docker.owns("docker.containers")).toBe(false);
    });
});
