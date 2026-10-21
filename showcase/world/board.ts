import type { LoggedMessage } from "../../src/agents/agentMessages";
import type {
  BoardCard,
  CardRun,
  ProjectBoard,
} from "../../src/agents/agentBoard";
import { SIKEMUX } from "./projects";
import { AGENTS } from "./workspace";

const OTTER = AGENTS.rail;
const MAPLE = AGENTS.replay;
const HOUR = 3_600_000;
const now = Date.now();

const card = (
  fields: Partial<BoardCard> & Pick<BoardCard, "id" | "title" | "status">,
): BoardCard => ({
  details: "",
  acceptance: "",
  owner: null,
  ownerName: null,
  ownerCli: null,
  team: OTTER.id,
  teamName: OTTER.name,
  parent: null,
  dependsOn: [],
  waitsOn: [],
  createdBy: OTTER.id,
  createdByName: OTTER.name,
  result: null,
  ticket: null,
  auto: null,
  paidBy: null,
  recordings: [],
  at: now - 3 * HOUR,
  updatedAt: now - fields.id * 600_000,
  ...fields,
});

const otter = { owner: OTTER.id, ownerName: OTTER.name, ownerCli: OTTER.type };
const maple = { owner: MAPLE.id, ownerName: MAPLE.name, ownerCli: MAPLE.type };
const person = {
  team: null,
  teamName: null,
  createdBy: null,
  createdByName: "the person",
};

const run = (
  agent: string,
  slug: string,
  fields: Partial<CardRun> & Pick<CardRun, "stage">,
): CardRun => ({
  agent,
  repo: SIKEMUX,
  worktree: `${SIKEMUX}.worktrees/${slug}`,
  branch: `sikemux/${slug}`,
  base: "main",
  startSha: "4f1c2a9e0b7d3c5a8e6f1b2c3d4e5f6a7b8c9d0e",
  note: null,
  pull: null,
  ...fields,
});

/** The sikemux project's board: Otter's team splitting a ticket, Auto landing tickets, and tickets nobody has taken yet. */
export const DEMO_BOARD: ProjectBoard = {
  project: SIKEMUX,
  settings: {
    auto: true,
    slots: 3,
    launcher: null,
    provider: null,
    model: null,
    finish: "merge",
    reviewer: null,
  },
  running: 1,
  autoNote: null,
  cards: [
    card({
      id: 1,
      title: "Store amounts in cents across billing",
      status: "progress",
      ...otter,
      createdBy: null,
      createdByName: "the person",
      ticket: { key: "PROJ-12", source: "Story", provider: "jira" },
    }),
    card({
      id: 2,
      title: "Migration for the amounts table",
      status: "done",
      parent: 1,
      owner: "agent-flint",
      ownerName: "Flint",
      ownerCli: "codex",
      result: "Added migration 0042 and a backfill for old rows.",
    }),
    card({
      id: 3,
      title: "Ledger import onto amounts",
      status: "progress",
      parent: 1,
      dependsOn: [2],
      ...maple,
    }),
    card({
      id: 4,
      title: "Date formatting in reports",
      status: "progress",
      parent: 1,
      owner: "agent-comet",
      ownerName: "Comet",
      ownerCli: "claude",
    }),
    card({
      id: 5,
      title: "Export CSV of a month's invoices",
      status: "open",
      ...person,
      ticket: { key: "PROJ-19", source: "Story", provider: "jira" },
    }),
    card({
      id: 6,
      title: "Totals are off by a cent on refunds",
      status: "open",
      ...person,
      dependsOn: [3],
      waitsOn: [3],
      ticket: { key: "PROJ-23", source: "Bug", provider: "jira" },
    }),
    card({
      id: 7,
      title: "Invoice PDFs show the customer's tax number",
      status: "progress",
      ...person,
      team: "agent-wren",
      teamName: "Wren",
      owner: "agent-wren",
      ownerName: "Wren",
      ownerCli: "claude",
      ticket: { key: "PROJ-27", source: "Story", provider: "jira" },
      auto: run("agent-wren", "proj-27-invoice-pdfs-show-the-customer-s-tax", {
        stage: "checking",
        note: "Its checks are running",
        pull: {
          number: 41,
          url: "https://github.com/nodelike/sikemux/pull/41",
          host: "github",
        },
      }),
    }),
    card({
      id: 8,
      title: "Retry webhooks that timed out",
      status: "progress",
      ...person,
      team: "agent-heron",
      teamName: "Heron",
      owner: "agent-heron",
      ownerName: "Heron",
      ownerCli: "codex",
      ticket: { key: "PROJ-31", source: "Bug", provider: "jira" },
      auto: run("agent-heron", "proj-31-retry-webhooks-that-timed-out", {
        stage: "held",
        note: "Its checks failed again after a fix:\ntest (ubuntu): failure",
        pull: {
          number: 39,
          url: "https://github.com/nodelike/sikemux/pull/39",
          host: "github",
        },
      }),
    }),
  ],
};

const MINUTE = 60_000;
const NAMES: Record<string, string> = {
  [OTTER.id]: OTTER.name,
  [MAPLE.id]: MAPLE.name,
  "agent-flint": "Flint",
  "agent-comet": "Comet",
};
const logged = (
  ago: number,
  from: string,
  to: string,
  kind: LoggedMessage["kind"],
  outcome: LoggedMessage["outcome"],
): LoggedMessage => ({
  at: now - ago * MINUTE,
  from,
  fromName: NAMES[from],
  kind,
  to,
  toName: NAMES[to],
  outcome,
});

/** The project's log of messages between agents, newest first. */
export const DEMO_MESSAGES: LoggedMessage[] = [
  logged(1, OTTER.id, "agent-flint", "message", "steered"),
  logged(3, "agent-comet", OTTER.id, "question", "waiting"),
  logged(6, MAPLE.id, "agent-flint", "answer", "delivered"),
  logged(9, "agent-flint", OTTER.id, "report", "newTurn"),
  logged(14, MAPLE.id, "agent-flint", "answer", "waiting"),
  logged(22, OTTER.id, "agent-comet", "message", "dropped"),
  logged(31, OTTER.id, "agent-comet", "message", "waiting"),
  logged(40, "agent-flint", MAPLE.id, "question", "steered"),
  logged(52, OTTER.id, MAPLE.id, "message", "refused"),
  logged(64, "person", OTTER.id, "message", "newTurn"),
];
