import { useResourceEnabled } from "../../plugin-api/resources";
import { defineService } from "../../plugin-api/service";
import { signozApi, type PanelRequest, type LogSearch, type Scope, type Signal, type TraceSearch } from "./api";
import { SIGNOZ_PLUGIN } from "./kinds";
import { signozSettings, type ServiceSort } from "./state";
import type { SignozServices } from "./types.generated";

export const signoz = defineService(SIGNOZ_PLUGIN, signozApi.status);

/** The services in the environment picked, sorted, beside every environment there is. */
export const signozServicesV = signoz.view<[scope: Scope, sort: ServiceSort], SignozServices>("servicesView", (scope, sort) => ({ ...scope, sort }));

/** The services as the settings pick and sort them, so every part of a pane asks for the same view. */
export function useServices(active: boolean, slot: string) {
    const minutes = signozSettings.useSelect((settings) => settings.minutes);
    const environment = signozSettings.useSelect((settings) => settings.environment);
    const sort = signozSettings.useSelect((settings) => settings.serviceSort);
    return useResourceEnabled(active, signozServicesV, slot, { minutes, environment: environment ?? undefined }, sort);
}

export const signozOverviewR = signoz.resource("overview", (scope: Scope) => signozApi.serviceOverview(scope), 30_000);

export const signozOperationsR = signoz.resource("operations", (scope: Scope) => signozApi.operations(scope), 30_000);

export const signozErrorGroupsR = signoz.resource("errorGroups", (scope: Scope) => signozApi.errorGroups(scope), 30_000);

export const signozTracesR = signoz.resource("traces", (search: TraceSearch) => signozApi.searchTraces(search), 30_000);

export const signozTraceR = signoz.resource("trace", (traceId: string) => signozApi.trace(traceId), 5 * 60_000);

export const signozTraceLogsR = signoz.resource("traceLogs", (search: LogSearch) => signozApi.searchLogs(search), 60_000);

export const signozFieldKeysR = signoz.resource("fieldKeys", (signal: Signal, search: string) => signozApi.fieldKeys(signal, search), 5 * 60_000);

export const signozFieldValuesR = signoz.resource(
    "fieldValues",
    (signal: Signal, name: string, search: string) => signozApi.fieldValues(signal, name, search),
    60_000,
);

export const signozVolumeR = signoz.resource("volume", (search: LogSearch & { buckets?: number }) => signozApi.logVolume(search), 15_000);

export const signozDashboardsR = signoz.resource("dashboards", () => signozApi.dashboards(), 5 * 60_000);

export const signozDashboardR = signoz.resource("dashboard", (id: string) => signozApi.dashboard(id), 5 * 60_000);

export const signozPanelR = signoz.resource("panel", (request: PanelRequest) => signozApi.panel(request), 30_000);
