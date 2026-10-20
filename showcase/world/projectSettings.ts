import { SIKEMUX } from "./projects";
import type {
  FolderSettings,
  ProjectFolders,
  ProjectRules,
} from "../../src/projects/projectFolders";
import type {
  ComposeFolder,
  Guardrail,
  JavaRuntime,
  ProjectChange,
} from "../../src/projects/projectResources";
import type { Suggestion } from "../../src/projects/projectSuggestions";

type Args = Record<string, unknown>;

const variables = (env: Record<string, string>) =>
  Object.entries(env).map(([key, value]) => ({ key, value }));

const FIELDS: ProjectFolders["fields"] = {
  node: [
    {
      section: "Runtime",
      label: "Node",
      desc: "The version the project's .nvmrc or .node-version asks for.",
      kind: "version",
      variable: "",
    },
    {
      section: "Runtime",
      label: "NODE_ENV",
      desc: "Which mode tools and frameworks run in.",
      kind: "choice",
      variable: "NODE_ENV",
      options: ["development", "test", "production"],
    },
    {
      section: "Runtime",
      label: "Node options",
      desc: "Given to every node started here, as NODE_OPTIONS.",
      kind: "text",
      variable: "NODE_OPTIONS",
      placeholder: "--max-old-space-size=4096",
    },
  ],
  rust: [
    {
      section: "Toolchain and run",
      label: "Toolchain",
      desc: "The rustup toolchain cargo uses here, as RUSTUP_TOOLCHAIN.",
      kind: "text",
      variable: "RUSTUP_TOOLCHAIN",
      placeholder: "stable",
    },
    {
      section: "Toolchain and run",
      label: "Log filter",
      desc: "What the program logs, as RUST_LOG.",
      kind: "text",
      variable: "RUST_LOG",
      placeholder: "info",
    },
    {
      section: "Toolchain and run",
      label: "Backtraces",
      desc: "Print where a panic happened, as RUST_BACKTRACE=1.",
      kind: "toggle",
      variable: "RUST_BACKTRACE",
    },
  ],
};

let settings: Record<string, FolderSettings> = {
  ".": { environment: "", done: ["detected.node.test", "detected.node.lint"] },
  "src-tauri": {
    env: { RUST_BACKTRACE: "1" },
    done: ["detected.cargo.test:src-tauri"],
  },
};
let rules: ProjectRules = {
  notes: "Run pnpm check before you push.",
  neverEdit: ["src/api/generated/**"],
  neverRun: ["git push --force"],
};
let suggestions: Suggestion[] = [
  {
    id: "setup",
    by: "Otter",
    at: Date.now() - 4 * 60_000,
    title:
      "Set up sikemux: core tests need the dev socket, and the generated files have their own command",
    changes: [
      {
        id: "socket",
        kind: "variable",
        folder: "src-tauri",
        key: "SIKEMUX_CORE_SOCKET",
        value: "~/.config/sikemux/core.test.sock",
        why: "core tests talk to a core of their own, apart from the running app",
        evidence: "cargo test -p sikemux-core: 496 passed",
        status: "pending",
      },
      {
        id: "generated",
        kind: "guardrail",
        folder: ".",
        key: "neverEdit",
        value: "mobile/app/src/core/wire.generated.ts",
        why: "pnpm wire:generate writes it from the Rust protocol",
        status: "pending",
      },
    ],
  },
];

const FOLDERS = () => ({
  folders: [
    {
      path: ".",
      languages: [{ id: "node", markers: ["package.json"], version: "22" }],
      envFiles: [
        {
          path: ".env",
          environment: null,
          local: false,
          template: false,
          loadedBy: [],
          variables: variables({ SIKEMUX_API_URL: "http://127.0.0.1:4000" }),
        },
        {
          path: ".env.example",
          environment: null,
          local: false,
          template: true,
          loadedBy: [],
          variables: variables({ SIKEMUX_API_URL: "" }),
        },
      ],
      environments: [],
    },
    {
      path: "src-tauri",
      languages: [
        {
          id: "rust",
          markers: ["Cargo.toml", "rust-toolchain.toml"],
          version: "1.90",
        },
      ],
      envFiles: [],
      environments: [],
    },
  ],
  fields: FIELDS,
  settings,
  rules,
});

const SERVICES: ComposeFolder[] = [
  {
    folder: ".",
    file: "docker-compose.yml",
    docker: true,
    services: [
      {
        name: "postgres",
        image: "postgres:16",
        ports: ["5432:5432"],
        state: "running",
      },
      {
        name: "minio",
        image: "minio/minio",
        ports: ["9000:9000"],
        state: "exited",
      },
    ],
  },
];

const RUNTIMES: JavaRuntime[] = [];

const BRIEF = `root: Node 22
  environment: the shared env files loaded for you
src-tauri/: Rust 1.90 · Backtraces on
Done means: before you say your work is finished, run the checks below for each folder you changed with task_start and wait for them to pass. If one fails, fix the cause and run it again; tell the person if you cannot.
  root → test (detected.node.test), lint (detected.node.lint)
  src-tauri/ → test (detected.cargo.test:src-tauri)
Never edit without asking the person: src/api/generated/**
Never run: git push --force
From the person: Run pnpm check before you push.`;

function toggle(
  list: readonly string[] | undefined,
  item: string,
  on: boolean,
): string[] {
  const kept = (list ?? []).filter((known) => known !== item);
  return on ? [...kept, item] : kept;
}

function apply(folder: string, change: ProjectChange): void {
  const current = settings[folder] ?? {};
  switch (change.kind) {
    case "variable": {
      const { [change.key]: _replaced, ...env } = current.env ?? {};
      settings = {
        ...settings,
        [folder]: {
          ...current,
          env:
            change.value === null
              ? env
              : { ...env, [change.key]: change.value },
        },
      };
      return;
    }
    case "environment":
      settings = {
        ...settings,
        [folder]: { ...current, environment: change.value ?? undefined },
      };
      return;
    case "agentEnvironment":
      settings = {
        ...settings,
        [folder]: { ...current, agentEnvironment: change.value ?? undefined },
      };
      return;
    case "done":
      settings = {
        ...settings,
        [folder]: {
          ...current,
          done: toggle(current.done, change.task, change.on),
        },
      };
      return;
    case "service":
      settings = {
        ...settings,
        [folder]: {
          ...current,
          services: toggle(current.services, change.name, change.on),
        },
      };
      return;
    case "notes":
      rules = { ...rules, notes: change.text };
      return;
    case "checkEveryFolder":
      rules = { ...rules, checkEveryFolder: change.on };
      return;
    case "stopOnFailure":
      rules = { ...rules, stopOnFailure: change.on };
      return;
    case "guardrail":
      rules = {
        ...rules,
        [change.list]: toggle(rules[change.list], change.pattern, change.on),
      };
      return;
  }
}

const isDemo = (params: Args) => params.project === SIKEMUX;

/** The core's answers about project settings, for the demo project; other projects have none. */
export const PROJECT_SETTINGS: Record<string, (params: Args) => unknown> = {
  projectFolders: (params) =>
    isDemo(params)
      ? FOLDERS()
      : { folders: [], fields: FIELDS, settings: {}, rules: {} },
  projectSuggestions: (params) => (isDemo(params) ? suggestions : []),
  projectServices: (params) => (isDemo(params) ? SERVICES : []),
  projectBrief: (params) => (isDemo(params) ? BRIEF : ""),
  javaRuntimes: () => ({ default: null, installed: RUNTIMES }),
  changeProjectSettings: (params) => {
    apply(String(params.folder ?? "."), params.change as ProjectChange);
    return null;
  },
  answerSuggestion: (params) => {
    const chosen = new Set(params.apply as string[]);
    for (const change of suggestions.find(
      (suggestion) => suggestion.id === params.id,
    )?.changes ?? []) {
      if (!chosen.has(change.id)) continue;
      if (change.kind === "variable")
        apply(change.folder, {
          kind: "variable",
          key: change.key ?? "",
          value: change.value || null,
        });
      if (change.kind === "guardrail")
        apply(".", {
          kind: "guardrail",
          list: change.key as Guardrail,
          pattern: change.value ?? "",
          on: true,
        });
    }
    suggestions = suggestions.map((suggestion) =>
      suggestion.id === params.id
        ? {
            ...suggestion,
            changes: suggestion.changes.map((change) => ({
              ...change,
              status: chosen.has(change.id) ? "applied" : "discarded",
            })),
          }
        : suggestion,
    );
    return null;
  },
  serviceAction: () => null,
};
