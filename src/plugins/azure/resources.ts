import { defineService } from "../../plugin-api/service";
import { azureApi, type Scope } from "./api";
import { AZURE_PLUGIN } from "./kinds";

export const azure = defineService(AZURE_PLUGIN, azureApi.status);

export const azureGroupsR = azure.resource("groups", (subscription: string) => azureApi.groups(subscription), 120_000);

export const azureResourcesR = azure.resource("resources", (scope: Scope, find: string) => azureApi.resources(scope, find), 30_000);

export const azureVmsR = azure.resource("vms", (scope: Scope) => azureApi.vms(scope), 15_000);

export const azureVmR = azure.resource("vm", (scope: Scope, name: string) => azureApi.vm(scope, name), 15_000);

export const azureAppsR = azure.resource("apps", (scope: Scope) => azureApi.apps(scope), 30_000);

export const azureAppR = azure.resource("app", (scope: Scope, name: string) => azureApi.app(scope, name), 30_000);

export const azureStorageR = azure.resource("storage", (scope: Scope) => azureApi.storage(scope), 60_000);

export const azureVaultsR = azure.resource("vaults", (scope: Scope) => azureApi.vaults(scope), 60_000);

export const azureActivityR = azure.resource(
    "activity",
    (scope: Scope, hours: number, failedOnly: boolean) => azureApi.activity(scope, hours, failedOnly),
    30_000,
);
