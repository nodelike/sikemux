const run = (page, action, arg) => page.evaluate(action, arg);

export const openWindow = (page, sessionId, windowId) =>
  run(
    page,
    ([session, window]) => {
      showcase.cmd.selectSession(session);
      showcase.cmd.selectWindowId(window);
    },
    [sessionId, windowId],
  );

/* Agents across the projects working, waiting on the person and done, as a busy afternoon looks. */
export const agentsAtWork = (page) =>
  run(page, () => {
    const state = (kind, minutesAgo) => ({
      state: kind,
      backendState: kind === "done" ? "idle" : kind,
      unread: kind === "done",
      updatedAt: Date.now() - minutesAgo * 60_000,
      lastWorkedAt: Date.now() - minutesAgo * 60_000,
      sequence: 1,
      source: "acp",
      confidence: "high",
      reason: "",
    });
    showcase.store.setState({
      agentActivity: {
        "tab-rail": state("working", 0),
        "agent-replay": state("blocked", 3),
        "agent-hero": state("done", 12),
        "agent-palette": state("working", 1),
        "agent-notes": state("blocked", 7),
      },
    });
  });

/* The composer's tag menu over the demo repo's open issues and pull requests. `loading` holds them back. */
const composerScene = async (page, write, { loading = false } = {}) => {
  await openWindow(page, "s-sikemux", "w-tab-rail");
  await run(
    page,
    ({ loading }) => {
      const backend = showcase.backend;
      const base = backend.handlers.get("plugin_call");
      const template = base({ plugin: "sikemux.github", method: "pulls", params: { state: "open" } })[0];
      const ago = (minutes) => new Date(Date.now() - minutes * 60_000).toISOString();
      const url = (kind, number) => `https://github.com/nodelike/sikemux/${kind}/${number}`;
      const issue = (number, title, minutes) => ({
        number,
        title,
        body: "",
        state: "open",
        stateReason: null,
        author: "nodelike",
        avatarUrl: null,
        createdAt: ago(minutes),
        updatedAt: ago(minutes),
        closedAt: null,
        comments: 0,
        labels: [],
        assignees: [],
        url: url("issues", number),
      });
      const pull = (number, title, minutes, draft = false) => ({
        ...template,
        number,
        title,
        draft,
        createdAt: ago(minutes),
        updatedAt: ago(minutes),
        url: url("pull", number),
      });
      const issues = [
        issue(135, "Agents: a draft survives switching between GUI and TUI", 40),
        issue(133, "Composer: tag issues, pull requests, agents and files inline", 90),
        issue(115, "Epic: desktop UI on GPUI for speed and memory", 600),
        issue(110, "Phone: hold sends while disconnected and say when the phone is the one offline", 900),
      ];
      const pulls = [
        pull(136, "feat(chat): a draft survives switching an agent between its chat and its TUI", 30),
        pull(134, "feat(agents): Sign in from Settings and the agent palette", 200, true),
        pull(131, "fix(rail): picking an account switches its chats too", 400),
      ];
      const never = new Promise(() => {});
      backend.on("plugin_call", (args) => {
        if (args.plugin === "sikemux.github" && args.method === "issues")
          return loading ? never : { issues, total: issues.length, nextPage: null };
        if (args.plugin === "sikemux.github" && args.method === "pulls")
          return loading ? never : args.params?.state === "closed" ? [] : pulls;
        return base(args);
      });
    },
    { loading },
  );
  const box = page.locator('[role="textbox"][aria-label="Message agent"]:visible').first();
  await box.click();
  await write(box);
  await page.waitForTimeout(500);
};

const composerCrop = {
  composer: {
    selector: ".agent-chat-pane",
    region: { left: 0, top: 0.6, width: 1, height: 0.4 },
  },
};

export const README_SCREENSHOTS = {
  "hero-framed": "sikemux-hero.png",
  "files-focus": "project-editor-view.png",
  "terminals-focus": "project-term-view.png",
  "git-focus": "project-git-view.png",
  "agents-focus": "project-agents-view.png",
  "aws-billing-card": "cloud-aws-billing-view.png",
  "aws-ecs-logs-card": "cloud-aws-ecs-tasks-logs-view.png",
  "rundeck-deploy-card": "cicd-rundeck-deploy-view.png",
  "signoz-dashboard-card": "observability-signoz-view.png",
  "bruno-card": "api-bruno-pane-view.png",
  "github-run-card": "cicd-github-run-view.png",
};

export const SCENES = [
  {
    name: "hero",
    settle: 1500,
    setup: async (page) => {
      await openWindow(page, "s-mood", "w-agent-hero");
      await run(page, () =>
        showcase.cmd.openDesk("agent-hero", { focus: false }),
      );
    },
    crops: { stage: ".stage" },
  },
  {
    name: "agents",
    setup: (page) => openWindow(page, "s-sikemux", "w-agent-replay"),
    crops: {
      chat: ".stage",
      rail: ".workspace-rail",
      focus: {
        selector: ".stage",
        region: { left: 0.1405, top: 0.3957, width: 0.7189, height: 0.3411 },
      },
    },
  },
  {
    name: "agents-all-projects",
    settle: 1200,
    setup: async (page) => {
      await openWindow(page, "s-sikemux", "w-tab-rail");
      await agentsAtWork(page);
      await run(page, () => {
        showcase.cmd.setTabRailAllAgents(true);
        showcase.cmd.setTabRailScope("all");
      });
    },
    crops: { rail: ".workspace-rail" },
  },
  {
    name: "files",
    setup: (page) => openWindow(page, "s-sikemux", "w-sikemux-files"),
    crops: {
      editor: ".stage",
      tree: ".ed-tree",
      focus: {
        selector: ".stage",
        region: { left: 0.0017, top: 0.0019, width: 0.6474, height: 0.5010 },
      },
    },
  },
  {
    name: "terminals",
    setup: (page) => openWindow(page, "s-sikemux", "w-sikemux-term"),
    crops: {
      stage: ".stage",
      focus: {
        selector: ".stage",
        region: { left: 0.5273, top: 0.0078, width: 0.4659, height: 0.3119 },
      },
    },
  },
  {
    name: "git",
    setup: (page) => openWindow(page, "s-sikemux", "w-sikemux-git"),
    crops: {
      stage: ".stage",
      focus: {
        selector: ".stage",
        region: { left: 0.0017, top: 0.0019, width: 0.5562, height: 0.3304 },
      },
    },
  },
  {
    name: "activity",
    settle: 1200,
    setup: (page) => run(page, () => showcase.cmd.openSettings("activity")),
    crops: {
      page: ".settings-page",
      stats: ".activity-stats",
      calendar: ".activity-calendar",
    },
  },
  {
    name: "command-deck",
    setup: async (page) => {
      await openWindow(page, "s-sikemux", "w-tab-rail");
      await run(page, () => showcase.cmd.openCommandPalette());
    },
    crops: { deck: "[role=dialog]" },
  },
  {
    name: "rundeck",
    settle: 1400,
    setup: async (page) => {
      await openWindow(page, "s-rundeck", "w-rundeck");
      await page.waitForTimeout(500);
      await run(page, async () => {
        const rundeck = await import("/src/plugins/rundeck/state.ts");
        rundeck.rundeckPush("p-rundeck", { kind: "matrix" });
      });
    },
    crops: { stage: ".stage" },
  },
  {
    name: "rundeck-deploy",
    settle: 1400,
    setup: async (page) => {
      await openWindow(page, "s-rundeck", "w-rundeck");
      await page.waitForTimeout(500);
      await run(page, async () => {
        const rundeck = await import("/src/plugins/rundeck/state.ts");
        rundeck.rundeckPush("p-rundeck", {
          kind: "execution",
          executionId: 48199,
          project: "platform",
          jobId: "job-production-billing-service",
          name: "billing-service",
          group: "deploy/production",
        });
      });
    },
    crops: {
      stage: ".stage",
      card: {
        selector: ".stage",
        region: { left: 0.1549, top: 0.0019, width: 0.8438, height: 0.4620 },
      },
    },
  },
  {
    name: "signoz",
    settle: 1400,
    setup: async (page) => {
      await openWindow(page, "s-signoz", "w-signoz");
      await page.waitForTimeout(500);
      await run(page, async () => {
        const signoz = await import("/src/plugins/signoz/state.ts");
        signoz.showService("p-signoz", "api-gateway", "overview");
      });
    },
    crops: { stage: ".stage" },
  },
  {
    name: "signoz-dashboard",
    settle: 1600,
    setup: async (page) => {
      await openWindow(page, "s-signoz", "w-signoz");
      await page.waitForTimeout(500);
      await run(page, async () => {
        const signoz = await import("/src/plugins/signoz/state.ts");
        signoz.openDashboard("p-signoz", "dash-api");
      });
    },
    crops: {
      stage: ".stage",
      card: {
        selector: ".stage",
        region: { left: 0.1486, top: 0.0019, width: 0.8500, height: 0.5205 },
      },
    },
  },
  {
    name: "jira",
    settle: 1400,
    setup: async (page) => {
      await openWindow(page, "s-jira", "w-jira");
      await page.waitForTimeout(500);
      await run(page, async () => {
        const jira = await import("/src/plugins/jira/state.ts");
        jira.updateJiraView("p-jira", { list: { kind: "mine" }, issue: "ORB-212" });
      });
    },
    crops: { stage: ".stage" },
  },
  {
    name: "agent-board",
    settle: 1200,
    setup: async (page) => {
      await openWindow(page, "s-sikemux", "w-tab-rail");
      await run(page, () => showcase.cmd.openBoard());
      await page.waitForSelector(".kanban-card");
      await page.click(".kanban-card");
    },
    crops: { stage: ".stage" },
  },
  {
    name: "jira-board",
    settle: 1400,
    setup: async (page) => {
      await openWindow(page, "s-jira", "w-jira");
      await page.waitForTimeout(500);
      await run(page, async () => {
        const jira = await import("/src/plugins/jira/state.ts");
        jira.updateJiraView("p-jira", {
          list: { kind: "board", id: 7, name: "Orbit board" },
          issue: null,
        });
      });
    },
    crops: { stage: ".stage" },
  },
  {
    name: "github-run",
    settle: 1600,
    setup: async (page) => {
      await openWindow(page, "s-sikemux", "w-sikemux-git");
      await page.waitForTimeout(500);
      await run(page, async () => {
        const commands = await import("/src/state/commands/index.ts");
        const host = await import("/src/codehost/state.ts");
        commands.setGitView("p-git", { area: "actions" });
        host.showRun("p-git", 36316473434);
      });
    },
    crops: {
      stage: ".stage",
      card: {
        selector: ".stage",
        region: { left: 0, top: 0, width: 1, height: 0.8 },
      },
    },
  },
  {
    name: "aws-ecs-logs",
    settle: 1500,
    setup: (page) =>
      run(page, async () => {
        const aws = await import("/src/plugins/aws/state.ts");
        await aws.setAwsProfile("acme-prod");
        aws.setAwsService("ecs");
        aws.setEcsLevel("acme-prod", {
          kind: "service",
          cluster: "prod",
          service: "api-gateway",
          tab: "logs",
        });
        aws.openAwsSession();
      }),
    crops: {
      stage: ".stage",
      card: {
        selector: ".stage",
        region: { left: 0.1458, top: 0.0019, width: 0.8528, height: 0.3645 },
      },
    },
  },
  {
    name: "aws-billing",
    settle: 1500,
    setup: (page) =>
      run(page, async () => {
        const aws = await import("/src/plugins/aws/state.ts");
        await aws.setAwsProfile("acme-prod");
        aws.setAwsService("billing");
        aws.openAwsSession();
      }),
    crops: {
      stage: ".stage",
      card: {
        selector: ".stage",
        region: { left: 0.1458, top: 0.0000, width: 0.8528, height: 0.5585 },
      },
    },
  },
  {
    name: "bruno",
    settle: 1500,
    setup: async (page) => {
      await run(page, async () => {
        const bruno = await import("/src/plugins/bruno/state.ts");
        const collection = "/Users/edon/api/acme-store";
        bruno.brunoSettings.update((settings) => ({
          ...settings,
          selectedEnvs: { [collection]: "production" },
        }));
        bruno.openBrunoSession(collection);
        await new Promise((resolve) => setTimeout(resolve, 600));
        const state = showcase.store.getState();
        const paneId =
          state.windows[state.sessions[state.activeSessionId].activeWindowId]
            .activePaneId;
        for (const request of [
          "catalog/search.bru",
          "users/me.bru",
          "checkout/create.bru",
        ]) {
          bruno.brunoSelectRequest(paneId, `${collection}/${request}`);
        }
      });
      await page.waitForTimeout(500);
      await page.keyboard.press("Meta+Enter");
      await page.getByRole("button", { name: "Trust collection" }).click();
      await page.locator(".bruno-tab", { hasText: /^body/i }).first().click();
    },
    crops: {
      stage: ".stage",
      card: {
        selector: ".stage",
        region: { left: 0.1736, top: 0.0390, width: 0.8250, height: 0.3567 },
      },
    },
  },
  {
    name: "visual-diagram",
    settle: 1800,
    setup: (page) => openWindow(page, "s-billing", "w-agent-flow"),
    crops: { stage: ".stage" },
  },
  {
    name: "visual-chart",
    settle: 1800,
    setup: (page) => openWindow(page, "s-billing", "w-agent-latency"),
    crops: { stage: ".stage" },
  },
  {
    name: "visual-mockup",
    settle: 1800,
    setup: (page) => openWindow(page, "s-front", "w-agent-empty"),
    crops: { stage: ".stage" },
  },
  {
    name: "simulator",
    settle: 2500,
    setup: async (page) => {
      await openWindow(page, "s-sikemux", "w-agent-phone");
      await run(page, () =>
        showcase.cmd.openDeskSimulator("agent-phone", {
          focus: false,
          device: { udid: "5C1A0E77-8D2F-4B1E-9F63-2A7D41C0B9E8", name: "iPhone 18 Pro" },
        }),
      );
    },
    crops: { stage: ".stage" },
  },
  {
    name: "database",
    settle: 1800,
    setup: async (page) => {
      await openWindow(page, "s-database", "w-database");
      await page.waitForTimeout(600);
      await run(page, async () => {
        const state = await import("/src/plugins/database/state.ts");
        const tabs = await import("/src/plugins/database/tabs.ts");
        const queries = await import("/src/plugins/database/queryState.ts");
        const { DATABASE_QUERY } = await import("/showcase/world/database.ts");
        const console = tabs.consoleTab("db-shop", 1);
        state.expandNodes("p-database", ["c:db-shop", "s:db-shop:public", "g:db-shop:public:tables"]);
        state.showTab("p-database", console);
        queries.loadQuery(console.id, DATABASE_QUERY);
        await queries.runQuery(console.id, "db-shop", DATABASE_QUERY);
      });
    },
    crops: { stage: ".stage" },
  },
  {
    name: "agent-tools",
    setup: (page) => openWindow(page, "s-billing", "w-agent-incident"),
  },
  {
    name: "accounts",
    setup: async (page) => {
      await openWindow(page, "s-sikemux", "w-tab-rail");
      await page.click(".agent-account-switch");
      await page.waitForTimeout(600);
    },
  },
  {
    name: "spaces",
    setup: (page) => openWindow(page, "s-shell", "w-shell"),
    crops: { rail: ".side-rail" },
  },
  {
    name: "projects-rail",
    setup: (page) => openWindow(page, "s-sikemux", "w-tab-rail"),
    crops: { rail: ".side-rail" },
  },
  ...["available", "downloading", "error"].map((state) => ({
    name: `update-${state}`,
    setup: async (page) => {
      await openWindow(page, "s-sikemux", "w-tab-rail");
      await run(
        page,
        (state) =>
          showcase.store.setState({
            pendingUpdate: {
              version: "0.5.3-nightly.7",
              currentVersion: "0.5.3-nightly.6",
              notes: null,
              date: null,
              credits: null,
              state,
              error: state === "error" ? "the download was cut short" : null,
              downloadedBytes: state === "downloading" ? 9_800_000 : 0,
              totalBytes: state === "downloading" ? 16_000_000 : null,
            },
          }),
        state,
      );
    },
    crops: { rail: ".side-rail" },
  })),
  {
    name: "composer-pulls",
    setup: (page) => composerScene(page, (box) => box.pressSequentially("Land !13")),
    crops: composerCrop,
  },
  {
    name: "composer-loading",
    setup: (page) =>
      composerScene(page, (box) => box.pressSequentially("Fix #"), { loading: true }),
    crops: composerCrop,
  },
  {
    name: "composer-agents",
    setup: (page) => composerScene(page, (box) => box.pressSequentially("ask @")),
    crops: composerCrop,
  },
  {
    name: "composer-buddies",
    setup: (page) =>
      composerScene(page, async (box) => {
        await run(page, () => showcase.store.setState({ buddies: true }));
        await box.pressSequentially("ask @Ma");
        await page.waitForTimeout(400);
        await box.press("Enter");
        await box.pressSequentially("then @");
      }),
    crops: composerCrop,
  },
  {
    name: "composer-files",
    setup: (page) => composerScene(page, (box) => box.pressSequentially("check +app")),
    crops: {
      composer: { ...composerCrop.composer, region: { left: 0, top: 0.4, width: 1, height: 0.6 } },
    },
  },
  {
    name: "composer-tags",
    setup: (page) =>
      composerScene(page, async (box) => {
        const pick = async (text) => {
          await box.pressSequentially(text);
          await page.waitForTimeout(400);
          await box.press("Enter");
        };
        await pick("Land !13");
        await pick("so it closes #13");
        await pick("then ask @Ma");
        await pick("to check +app");
        await box.press("Backspace");
        await box.press("Backspace");
      }),
    crops: composerCrop,
  },
];
