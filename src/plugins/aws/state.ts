import { create } from "zustand";
import { openSurface } from "../../plugin-api/host";
import { invalidate } from "../../plugin-api/resources";
import { definePluginSettings } from "../../plugin-api/settings";
import { whenSignedOut } from "./api";
import { AWS_CONSOLE, AWS_PLUGIN_ID } from "./kinds";

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
    profile: string | null;
    service: AwsService;
}

const isRecord = (value: unknown): value is Record<string, unknown> => !!value && typeof value === "object" && !Array.isArray(value);

function decodeSettings(saved: unknown): AwsSettings {
    const raw = isRecord(saved) ? saved : {};
    return {
        profile: typeof raw.profile === "string" && raw.profile ? raw.profile : null,
        service: (AWS_SERVICES as unknown[]).includes(raw.service) ? (raw.service as AwsService) : "ecs",
    };
}

export const awsSettings = definePluginSettings(AWS_PLUGIN_ID, decodeSettings);

export function setAwsProfile(profile: string | null): void {
    awsSettings.update((settings) => ({ ...settings, profile }));
}

export function setAwsService(service: AwsService): void {
    awsSettings.update((settings) => ({ ...settings, service }));
}

interface AwsView {
    authModal: { profile: string; ssoStartUrl: string | null } | null;
    /** How far into ECS each profile has drilled. */
    ecsViews: Record<string, EcsLevel>;
    expandedBillingMonth: Record<string, string | null>;
}

export const useAws = create<AwsView>()(() => ({ authModal: null, ecsViews: {}, expandedBillingMonth: {} }));

export function openAwsAuthModal(profile: string, ssoStartUrl: string | null): void {
    useAws.setState({ authModal: { profile, ssoStartUrl } });
}

export function closeAwsAuthModal(): void {
    useAws.setState({ authModal: null });
}

export function setEcsLevel(profile: string, level: EcsLevel): void {
    useAws.setState((state) => ({ ecsViews: { ...state.ecsViews, [profile]: level } }));
}

export function setBillingExpandedMonth(profile: string, month: string | null): void {
    useAws.setState((state) => ({ expandedBillingMonth: { ...state.expandedBillingMonth, [profile]: month } }));
}

export function openAwsSession(): void {
    openSurface(AWS_CONSOLE);
}

whenSignedOut(() => invalidate((kind) => kind.startsWith("aws.")));
