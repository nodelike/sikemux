import { defineService } from "../../plugin-api/service";
import { signozApi, type PanelRequest, type LogSearch, type Scope, type Signal, type TraceSearch } from "./api";
import { SIGNOZ_PLUGIN } from "./kinds";

export const signoz = defineService(SIGNOZ_PLUGIN, signozApi.status);

export const signozServicesR = signoz.resource("services", (scope: Scope) => signozApi.services(scope), 30_000);

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
