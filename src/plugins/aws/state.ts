import { create } from "zustand";
import { openSurface } from "../../plugin-api/host";
import { definePluginSettings } from "../../plugin-api/settings";
import { awsApi } from "./api";
import { AWS_CONSOLE, AWS_PLUGIN } from "./kinds";
import { aws, awsProfileR } from "./resources";

export type AwsService = "ecs" | "ec2" | "lambda" | "sqs" | "billing" | "s3";
export const AWS_SERVICES: AwsService[] = ["ecs", "ec2", "lambda", "sqs", "billing", "s3"];

export type EcsLevel =
    | { kind: "clusters" }
    | { kind: "services"; cluster: string }
    | {
          kind: "service";
          cluster: string;
          service: string;
          tab: "logs" | "tasks";
          taskFilter?: { taskId: string; stream: string };
      };

export interface AwsSettings {
    service: AwsService;
}

const isRecord = (value: unknown): value is Record<string, unknown> => !!value && typeof value === "object" && !Array.isArray(value);

function decodeSettings(saved: unknown): AwsSettings {
    const raw = isRecord(saved) ? saved : {};
    return {
        service: (AWS_SERVICES as unknown[]).includes(raw.service) ? (raw.service as AwsService) : "ecs",
    };
}

export const awsSettings = definePluginSettings(AWS_PLUGIN.id, decodeSettings);

/** Picks the profile the pane shows and its sign-in is checked as. */
export async function setAwsProfile(profile: string | null): Promise<void> {
    await awsApi.pickProfile(profile);
    aws.refresh(awsProfileR, aws.statusR);
}

export function setAwsService(service: AwsService): void {
    awsSettings.update((settings) => ({ ...settings, service }));
}

/** The row each list has picked out for the side panel, keyed by list. */
export type AwsSelection = Partial<Record<"cluster" | "service" | "ec2" | "lambda" | "sqs" | "s3" | "month", string>>;

interface AwsView {
    /** How far into ECS each profile has drilled. */
    ecsViews: Record<string, EcsLevel>;
    selection: Record<string, AwsSelection>;
    /** The Lambda function whose logs are open, per profile. */
    lambdaLogs: Record<string, string | null>;
    /** How many things each service listed the last time it loaded, per profile. */
    counts: Record<string, Partial<Record<AwsService, string>>>;
}

export const useAws = create<AwsView>()(() => ({ ecsViews: {}, selection: {}, lambdaLogs: {}, counts: {} }));

export function setEcsLevel(profile: string, level: EcsLevel): void {
    useAws.setState((state) => ({ ecsViews: { ...state.ecsViews, [profile]: level } }));
}

export function selectAws(profile: string, list: keyof AwsSelection, value: string): void {
    useAws.setState((state) => ({ selection: { ...state.selection, [profile]: { ...state.selection[profile], [list]: value } } }));
}

export function setLambdaLogs(profile: string, fn: string | null): void {
    useAws.setState((state) => ({ lambdaLogs: { ...state.lambdaLogs, [profile]: fn } }));
}

export function setAwsCount(profile: string, service: AwsService, count: string): void {
    if (useAws.getState().counts[profile]?.[service] === count) return;
    useAws.setState((state) => ({ counts: { ...state.counts, [profile]: { ...state.counts[profile], [service]: count } } }));
}

export function openAwsSession(): void {
    openSurface(AWS_CONSOLE);
}
