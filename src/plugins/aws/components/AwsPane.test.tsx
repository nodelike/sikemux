import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AwsIdentity, AwsProfile } from "../api";

const { profiles, identity } = vi.hoisted(() => ({ profiles: vi.fn(), identity: vi.fn() }));
vi.mock("../api", async (importOriginal) => {
    const original = await importOriginal<typeof import("../api")>();
    return { ...original, awsApi: { ...original.awsApi, profiles, identity } };
});

import { awsSettings, useAws } from "../state";
import { AwsOverlay } from "./AwsOverlay";
import { AwsPane } from "./AwsPane";

const prod: AwsProfile = {
    name: "prod-admin",
    region: "ap-south-1",
    sso_start_url: "https://example.awsapps.com/start",
    sso_region: "ap-south-1",
    sso_account_id: "123456789012",
    sso_role_name: "Admin",
    kind: "sso",
};

const expired: AwsIdentity = { arn: null, account: null, user_id: null, status: "expired", message: "Token has expired" };

afterEach(cleanup);

beforeEach(() => {
    awsSettings.update(() => ({ profile: null, service: "ecs" }));
    useAws.setState({ authModal: null, ecsViews: {}, expandedBillingMonth: {} });
    profiles.mockReset().mockResolvedValue([prod]);
    identity.mockReset().mockResolvedValue(expired);
});

describe("AwsPane", () => {
    it("lists the profiles to pick from, then asks to sign in to one whose session ran out", async () => {
        await act(async () => {
            render(
                <>
                    <AwsPane active />
                    <AwsOverlay />
                </>,
            );
        });
        expect(screen.getByText("Pick a profile")).toBeInTheDocument();

        await act(async () => {
            fireEvent.click(screen.getByRole("button", { name: /prod-admin/ }));
        });
        expect(awsSettings.get().profile).toBe("prod-admin");
        expect(identity).toHaveBeenCalledWith("prod-admin", false);
        expect(screen.getByText("Session expired")).toBeInTheDocument();

        await act(async () => {
            fireEvent.click(screen.getByRole("button", { name: /sign in/i }));
        });
        expect(useAws.getState().authModal).toEqual({ profile: "prod-admin", ssoStartUrl: prod.sso_start_url });
        expect(screen.getByText("https://example.awsapps.com/start")).toBeInTheDocument();
    });
});
