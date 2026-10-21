import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const api = vi.hoisted(() => ({}) as ReturnType<typeof import("../testing").mockApi>);
vi.mock("../api", async (importOriginal) => {
    const { mockApi } = await import("../testing");
    Object.assign(api, mockApi());
    return { ...(await importOriginal<object>()), gcpApi: api };
});
const notify = vi.hoisted(() => vi.fn());
vi.mock("../../../plugin-api/host", async (importOriginal) => ({ ...(await importOriginal<object>()), notify }));

import { answerEmpty } from "../testing";
import { attributesOf, PubSubPage } from "./PubSubPage";
import { gcp } from "../resources";

const target = { account: null, project: "acme-prod" };

let pane = 0;
const renderPage = () => render(<PubSubPage paneId={`ps-${++pane}`} active target={target} />);

beforeEach(() => {
    Object.values(api).forEach((mock) => mock.mockReset());
    answerEmpty(api);
    api.pubsub.mockResolvedValue({
        topics: [{ name: "orders", retention: null }],
        subscriptions: [
            {
                name: "orders-worker",
                topic: "orders",
                kind: "pull",
                pushEndpoint: null,
                ackDeadlineSeconds: 30,
                retention: null,
                deadLetterTopic: "orders-dead",
                maxDeliveryAttempts: 5,
                filter: null,
                ordering: false,
                state: "ACTIVE",
            },
        ],
    });
    api.publish.mockResolvedValue({ messageId: "42" });
    api.peek.mockResolvedValue([
        { id: "7", publishedAt: "2024-10-18T11:00:00Z", data: { text: '{"order": 1}' }, attributes: { kind: "created" }, deliveryAttempt: 2 },
    ]);
    notify.mockClear();
});

afterEach(() => {
    cleanup();
    gcp.refresh();
});

describe("Pub/Sub", () => {
    it("reads attributes one key=value a line", () => {
        expect(attributesOf("kind = created\nsource=web\nnoise\n=empty")).toEqual({ kind: "created", source: "web" });
    });

    it("publishes a message with attributes to a topic", async () => {
        renderPage();
        fireEvent.click(await screen.findByRole("row", { name: /orders/ }));
        fireEvent.change(screen.getByRole("textbox", { name: "Message body" }), { target: { value: '{"order": 2}' } });
        fireEvent.change(screen.getByRole("textbox", { name: "Attributes" }), { target: { value: "kind=created" } });
        fireEvent.click(screen.getByRole("button", { name: "Publish" }));
        await waitFor(() => expect(api.publish).toHaveBeenCalledWith(target, "orders", '{"order": 2}', { kind: "created" }));
        expect(notify).toHaveBeenCalledWith("success", "Published message 42");
    });

    it("peeks at a subscription's waiting messages", async () => {
        renderPage();
        fireEvent.click(await screen.findByRole("tab", { name: "Subscriptions" }));
        fireEvent.click(await screen.findByRole("row", { name: /orders-worker/ }));
        expect(screen.getByText("orders-dead after 5 attempts")).toBeInTheDocument();
        fireEvent.click(screen.getByRole("button", { name: "View messages" }));
        expect(await screen.findByText('{"order": 1}')).toBeInTheDocument();
        expect(screen.getByText("kind=created")).toBeInTheDocument();
    });
});
