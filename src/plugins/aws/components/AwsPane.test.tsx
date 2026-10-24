import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AwsIdentity, AwsProfile } from "../api";

const { profiles, setup, identity, ssoLogin, profile, pickProfile, status } = vi.hoisted(() => ({
    profiles: vi.fn(),
    setup: vi.fn(),
    identity: vi.fn(),
    ssoLogin: vi.fn(),
    profile: vi.fn(),
    pickProfile: vi.fn(),
    status: vi.fn(),
}));
vi.mock("../api", async (importOriginal) => {
    const original = await importOriginal<typeof import("../api")>();
    return { ...original, awsApi: { ...original.awsApi, profiles, setup, identity, ssoLogin, profile, pickProfile, status } };
});

import { invalidate } from "../../../plugin-api/resources";
import { awsSettings, useAws } from "../state";
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
    let picked: string | null = null;
    awsSettings.update(() => ({ service: "ecs" }));
    invalidate(() => true);
    useAws.setState({ ecsViews: {}, selection: {}, lambdaLogs: {}, counts: {} });
    profiles.mockReset().mockResolvedValue([prod]);
    setup.mockReset().mockResolvedValue({ cli: true, homebrew: true });
    identity.mockReset().mockResolvedValue(expired);
    ssoLogin.mockReset().mockReturnValue({ done: new Promise(() => {}), cancel: vi.fn() });
    profile.mockReset().mockImplementation(async () => ({ profile: picked }));
    pickProfile.mockReset().mockImplementation(async (name: string | null) => {
        picked = name;
        return { profile: picked };
    });
    status.mockReset().mockResolvedValue({ configured: false, ok: false, authFailed: false, message: null, profile: null, account: null });
});

describe("AwsPane", () => {
    it("lists the profiles to pick from, then asks to sign in to one whose session ran out", async () => {
        await act(async () => {
            render(<AwsPane paneId="pane-1" active />);
        });
        expect(screen.getByText("Pick an AWS profile")).toBeInTheDocument();

        await act(async () => {
            fireEvent.click(screen.getByRole("button", { name: /prod-admin/ }));
        });
        expect(pickProfile).toHaveBeenCalledWith("prod-admin");
        expect(identity).toHaveBeenCalledWith("prod-admin", false);
        expect(screen.getByText("Sign-in expired")).toBeInTheDocument();

        await act(async () => {
            fireEvent.click(screen.getByRole("button", { name: "Sign in" }));
        });
        expect(ssoLogin).toHaveBeenCalledWith("prod-admin");
        expect(screen.getByText("Approve the request in your browser")).toBeInTheDocument();
    });
});
