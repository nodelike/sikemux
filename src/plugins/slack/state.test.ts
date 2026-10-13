import { describe, expect, it } from "vitest";
import { threadForAgent } from "./state";

const thread = {
    workspace: "T1",
    channel: "C9",
    ts: "1700000000.123456",
    permalink: "https://acme.slack.com/archives/C9/p1700000000123456",
    truncated: false,
    messages: [
        {
            ts: "1",
            user: "U1",
            name: "Irwan",
            bot: false,
            text: "Deploy failed on staging",
            at: "2026-10-10T09:00:00Z",
            threadTs: "1",
            replyCount: 1,
            files: ["deploy.log"],
        },
        { ts: "2", user: "U2", name: "Ankit", bot: false, text: "Looking", at: "2026-10-10T09:02:00Z", threadTs: "1", replyCount: 0, files: [] },
    ],
};

describe("a thread handed to an agent", () => {
    it("says where it is, who said what and when, and how to reply", () => {
        const text = threadForAgent(thread, "deploys");
        expect(text.startsWith("Slack thread in #deploys — https://acme.slack.com/archives/C9/p1700000000123456")).toBe(true);
        expect(text).toContain("**Irwan** (2026-10-10T09:00:00Z):\nDeploy failed on staging\n  (attached: deploy.log)");
        expect(text).toContain("**Ankit** (2026-10-10T09:02:00Z):\nLooking");
        expect(text).toContain("use slack_post with link https://acme.slack.com/archives/C9/p1700000000123456");
    });

    it("names the channel by id when its name is unknown, and says when the thread was cut", () => {
        const text = threadForAgent({ ...thread, permalink: null, truncated: true }, null);
        expect(text.startsWith("Slack thread in C9\n")).toBe(true);
        expect(text).toContain("only the first 1000 messages");
        expect(text).not.toContain("slack_post");
    });
});
