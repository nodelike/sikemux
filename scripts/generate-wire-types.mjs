#!/usr/bin/env node
// Writes the types clients read the core's JSON with, from the Rust that
// serializes it: TypeScript for the app and the phone, Swift for the notch,
// and the app's copy of the defaults of the types in DEFAULTS.
// It reads the serde declarations itself and fails on any it does not know,
// rather than guess. `--check` fails when an output is out of date.

import { mkdir, readFile, writeFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { format, resolveConfig } from "prettier";

const root = resolve(import.meta.dirname, "..");
const check = process.argv.includes("--check");

const WIRE = "src-tauri/crates/sikemux-wire/src";
const CORE = "src-tauri/crates/sikemux-core/src";
const PLUGIN_API = "src-tauri/crates/sikemux-plugin-api/src";
const SOURCES = [
  `${WIRE}/protocol.rs`,
  `${WIRE}/chat.rs`,
  `${WIRE}/theme.rs`,
  `${WIRE}/notch.rs`,
  `${WIRE}/pty/shell_protocol.rs`,
  `${WIRE}/pty/task.rs`,
  `${WIRE}/accounts/protocol.rs`,
  `${CORE}/server/inbox.rs`,
  `${CORE}/server/questions.rs`,
  `${CORE}/server/catalog.rs`,
  `${CORE}/server/drafts.rs`,
  `${CORE}/server/changed.rs`,
  `${CORE}/server/board.rs`,
  `${CORE}/server/board/auto.rs`,
  `${CORE}/server/workspace.rs`,
  `${CORE}/server/remote.rs`,
  `${CORE}/server/hosts/mod.rs`,
  `${CORE}/server/updates.rs`,
  `${CORE}/frozen.rs`,
  `${CORE}/releases.rs`,
  ...[
    "status",
    "branches",
    "log",
    "revisions",
    "stash",
    "remote",
    "worktree",
    "blame",
    "diff",
  ].map((file) => `${CORE}/git/${file}.rs`),
  `${WIRE}/codehost.rs`,
  `${WIRE}/jira.rs`,
  `${WIRE}/rundeck.rs`,
  `${WIRE}/aws.rs`,
  `${WIRE}/linear.rs`,
  `${WIRE}/database.rs`,
  `${WIRE}/bruno.rs`,
  `${WIRE}/slack.rs`,
  `${WIRE}/vercel.rs`,
  `${WIRE}/signoz.rs`,
  `${PLUGIN_API}/ratelimit.rs`,
];

/** Jira's types share names with the code hosts', so each is known as Jira's. */
const JIRA_TYPES = [
  "Person",
  "IssueSummary",
  "Parent",
  "Comment",
  "Change",
  "FieldChange",
  "Transition",
  "Board",
  "Sprint",
  "Column",
  "BoardView",
  "Filter",
  "Project",
  "IssueListKind",
  "IssueList",
  "StatusCount",
  "Spaces",
  "Space",
  "BoardPage",
  "BoardPerson",
  "Swimlane",
  "IssueView",
  "Activity",
];

/** The Bruno plugin's types are known as its own, as the pane always named them. */
const BRUNO_TYPES = [
  "HttpMethod",
  "KeyVal",
  "BodyMode",
  "GraphqlBody",
  "Body",
  "AuthMode",
  "KeyPlacement",
  "BearerAuth",
  "BasicAuth",
  "KeyAuth",
  "Auth",
  "Meta",
  "RequestSettings",
  "RawBlock",
  "Params",
  "Vars",
  "Scripts",
  "Request",
  "Scope",
  "Env",
  "TreeNode",
  "Collection",
  "RequestView",
  "Header",
  "Response",
  "Sent",
  "ScriptLog",
  "Check",
  "RunResult",
  "KnownWorkspace",
  "KnownWorkspaces",
];

/** The Database plugin's types are known as its own. */
const DATABASE_TYPES = [
  "Engine",
  "TableKind",
  "Explorer",
  "Connection",
  "Fact",
  "Schema",
  "TableGroup",
  "TableEntry",
  "Column",
  "TableView",
  "IndexRow",
  "ForeignKeyRow",
  "History",
  "HistoryEntry",
  "QueryOutcome",
  "ResultSet",
  "ResultColumn",
  "CopyFormat",
  "ProfileForm",
  "EngineChoice",
  "TlsChoice",
  "ProfileDraft",
];

/** Linear's types are known as Linear's, as Jira's are known as Jira's. */
const LINEAR_TYPES = [
  "State",
  "IssueRow",
  "IssueList",
  "Label",
  "Comment",
  "IssueView",
];

/** The AWS plugin's types, known as its own beside the others'. */
const AWS_TYPES = [
  "Billing",
  "BillingMonth",
  "BillingCharge",
  "BillingChange",
  "Health",
  "Ec2",
  "Ec2Instance",
  "Lambda",
  "RuntimeFamily",
  "LambdaFunction",
  "Sqs",
  "SqsQueue",
  "S3",
  "S3Bucket",
  "EcsClusters",
  "EcsEnv",
  "EcsClusterRow",
  "EcsServices",
  "EcsServiceRow",
  "EcsTasks",
  "EcsTaskRow",
  "AgentProfiles",
  "LogLine",
  "LogLevel",
  "Account",
  "AccountProfile",
];

/** SigNoz's types are known as SigNoz's, as Jira's are known as Jira's. */
const SIGNOZ_TYPES = [
  "Point",
  "ServiceHealth",
  "ServiceRow",
  "ServiceSort",
  "Services",
  "Overview",
  "Operation",
  "ErrorGroup",
  "ServicePage",
  "LogLine",
  "LogPage",
  "VolumeBucket",
  "TraceSummary",
  "TracePage",
  "TraceSpan",
  "Trace",
  "DashboardSummary",
  "Variable",
  "Layout",
  "Panel",
  "Dashboard",
  "Series",
  "Column",
  "PanelData",
  "FieldKey",
];

/** Rundeck's types are known as Rundeck's, as Jira's are known as Jira's. */
const RUNDECK_TYPES = [
  "Tree",
  "TreeProject",
  "GroupNode",
  "StatusKind",
  "RunSummary",
  "JobRow",
  "Section",
  "Jobs",
  "Run",
  "StepState",
  "LiveStep",
  "LiveRun",
  "JobPage",
  "ActivityPage",
  "OptionKind",
  "JobOption",
  "OptionChoice",
  "DeployForm",
  "RunCheck",
  "Tone",
  "PlanCheck",
  "Plan",
  "RunStep",
  "ExecutionPage",
  "RunNotice",
  "EnvTone",
  "JobPlace",
  "DeployTarget",
  "LastRun",
  "JobCatalog",
  "TermColor",
  "StyledText",
];

/** Vercel's types are known as Vercel's. */
const VERCEL_TYPES = [
  "Production",
  "Project",
  "Target",
  "Deployment",
  "LogLine",
  "Log",
  "Domain",
  "Variable",
  "Projects",
  "Deployments",
  "OpenLog",
  "Domains",
  "Variables",
];

/** Slack's types are known as Slack's. */
const SLACK_TYPES = [
  "ChannelKind",
  "Channel",
  "Channels",
  "Message",
  "History",
  "Thread",
  "OpenThread",
];

/** Names the clients know a type by where the Rust one would clash or say too little. */
const RENAMES = {
  [`${CORE}/server/inbox.rs`]: {
    View: "Inbox",
    Inbox: "InboxTurns",
    Item: "InboxItem",
    Tier: "InboxTier",
    Kind: "InboxKind",
    Recording: "InboxRecording",
    Handoff: "InboxHandoff",
    CardLink: "InboxCard",
    Shown: "AgentShown",
    Rollup: "AgentRollup",
  },
  [`${CORE}/server/questions.rs`]: {
    View: "AgentQuestion",
    Choice: "QuestionChoice",
  },
  [`${CORE}/server/catalog.rs`]: { Driver: "ChatDriver" },
  [`${CORE}/server/changed.rs`]: { Topic: "CoreChangeTopic" },
  [`${CORE}/server/board.rs`]: {
    Status: "CardStatus",
    Ticket: "CardTicket",
    CardView: "BoardCard",
    Column: "BoardColumn",
    BoardView: "ProjectBoard",
  },
  [`${CORE}/server/board/auto.rs`]: {
    Stage: "RunStage",
    Settings: "BoardSettings",
    RunView: "CardRun",
    PullView: "CardPull",
    Run: "AutoRun",
    Pull: "AutoPull",
  },
  [`${CORE}/server/remote.rs`]: {
    Pairing: "RemotePairing",
    LinkedHost: "StoredLink",
  },
  [`${CORE}/server/hosts/mod.rs`]: { LinkView: "LinkedHost" },
  [`${CORE}/releases.rs`]: { CommitAuthor: "ReleaseCommitAuthor" },
  [`${PLUGIN_API}/ratelimit.rs`]: { Budget: "RateLimit" },
  [`${WIRE}/jira.rs`]: Object.fromEntries(
    JIRA_TYPES.map((name) => [name, `Jira${name}`]),
  ),
  [`${WIRE}/bruno.rs`]: Object.fromEntries(
    BRUNO_TYPES.map((name) => [name, `Bru${name}`]),
  ),
  [`${WIRE}/database.rs`]: Object.fromEntries(
    DATABASE_TYPES.map((name) => [name, `Database${name}`]),
  ),
  [`${WIRE}/linear.rs`]: Object.fromEntries(
    LINEAR_TYPES.map((name) => [name, `Linear${name}`]),
  ),
  [`${WIRE}/rundeck.rs`]: Object.fromEntries(
    RUNDECK_TYPES.map((name) => [name, `Rundeck${name}`]),
  ),
  [`${WIRE}/aws.rs`]: Object.fromEntries(
    AWS_TYPES.map((name) => [name, `Aws${name}`]),
  ),
  [`${WIRE}/slack.rs`]: Object.fromEntries(
    SLACK_TYPES.map((name) => [name, `Slack${name}`]),
  ),
  [`${WIRE}/vercel.rs`]: Object.fromEntries(
    VERCEL_TYPES.map((name) => [name, `Vercel${name}`]),
  ),
  [`${WIRE}/signoz.rs`]: Object.fromEntries(
    SIGNOZ_TYPES.map((name) => [name, `Signoz${name}`]),
  ),
};

/** Fields the Rust keeps as JSON so phones read changes over the air, and the type that JSON has. */
const JSON_FIELDS = {
  "DeviceView.inbox": "Inbox",
};

const TS_ROOTS = [
  "DeviceView",
  "ChatView",
  "ChatPage",
  "ChatChange",
  "ToolDetail",
  "CoreChangeTopic",
  "Preferences",
  "Palette",
  "ThemeSummary",
  "ProjectBoard",
  "HostInfo",
  "RemoteStatus",
  "AllowedDevice",
  "RemotePairing",
  "NetworkChange",
  "Inbox",
  "AgentQuestion",
  "CatalogAgent",
  "GitOverview",
  "DiscoveredRepo",
  "GitCompare",
  "GitStash",
  "GitRemote",
  "GitRemoteBranch",
  "GitWorktree",
  "GitBlame",
  "DiffHunk",
  "Draft",
  "UpdateStatus",
];

/** What the code-host plugins hand the Git pane. */
const CODEHOST_ROOTS = [
  "PullList",
  "RunList",
  "IssueList",
  "ReleaseList",
  "TrackedItem",
  "OpenPull",
  "OpenRun",
  "OpenJobLog",
  "RateLimit",
  "Resolved",
  "RepoListing",
  "Workflow",
  "RunPage",
  "RunDetail",
  "RunTiming",
  "JobLog",
  "LogExcerpt",
  "Annotation",
  "JobSummary",
  "WorkflowFile",
  "Artifact",
  "SavedArtifact",
  "PendingApproval",
  "Comment",
  "Pull",
  "PullCounts",
  "TimelineItem",
  "CommitAuthor",
  "PullCommit",
  "ChangedFile",
  "Review",
  "IssuePage",
  "Release",
  "Notification",
];

/** What the Jira plugin's views hand its pane. */
const JIRA_ROOTS = [
  "JiraIssueList",
  "JiraIssueListKind",
  "JiraSpaces",
  "JiraBoardPage",
  "JiraIssueView",
];

/** What the Bruno plugin's views hand its pane. */
const BRUNO_ROOTS = [
  "BruCollection",
  "BruRequestView",
  "BruRunResult",
  "BruKnownWorkspaces",
];

/** What the Database plugin's views hand its pane. */
const DATABASE_ROOTS = [
  "DatabaseExplorer",
  "DatabaseTableView",
  "DatabaseHistory",
  "DatabaseQueryOutcome",
  "DatabaseCopyFormat",
  "DatabaseProfileForm",
];

/** What the Linear plugin's views hand its pane. */
const LINEAR_ROOTS = ["LinearIssueList", "LinearIssueView"];

/** What the AWS plugin's views hand its pane. */
const AWS_ROOTS = [
  "AwsAccount",
  "AwsAgentProfiles",
  "AwsBilling",
  "AwsEcsClusters",
  "AwsEcsServices",
  "AwsEcsTasks",
  "AwsEc2",
  "AwsLambda",
  "AwsSqs",
  "AwsS3",
  "AwsLogLine",
];

/** What the Rundeck plugin's views hand its pane. */
const RUNDECK_ROOTS = [
  "RundeckTree",
  "RundeckJobs",
  "RundeckJobPage",
  "RundeckActivityPage",
  "RundeckDeployForm",
  "RundeckRunCheck",
  "RundeckOptionChoice",
  "RundeckPlan",
  "RundeckExecutionPage",
  "RundeckRunNotice",
  "RundeckDeployTarget",
  "RundeckJobCatalog",
  "RundeckStyledText",
];

/** What the SigNoz plugin's views hand its pane. */
const SIGNOZ_ROOTS = [
  "SignozServices",
  "SignozServicePage",
  "SignozLogPage",
  "SignozVolumeBucket",
  "SignozTracePage",
  "SignozTrace",
  "SignozDashboardSummary",
  "SignozDashboard",
  "SignozPanelData",
  "SignozFieldKey",
];

/** What the Slack plugin's views hand its pane. */
const SLACK_ROOTS = ["SlackChannels", "SlackHistory", "SlackOpenThread"];

/** What the Vercel plugin's views hand its pane. */
const VERCEL_ROOTS = [
  "VercelProjects",
  "VercelDeployments",
  "VercelOpenLog",
  "VercelDomains",
  "VercelVariables",
];

/** The fields the notch reads of each type. Swift's decoder ignores the rest. */
const SWIFT_FIELDS = {
  DeviceView: "workspace chats sessions attentions inbox faceStyle",
  Workspace: "projects launchers",
  ProjectInfo: "id name path",
  LauncherInfo: "id provider label permissionMode model effort",
  ConfigPicker: "id current choices",
  ConfigChoice: "value label",
  ChatInfo: "agentId name provider cwd reconnecting",
  SessionInfo: "id project agentId agentName agentType",
  Attention: "id agentId choices",
  PermissionChoices: "allowOnce allowAlways reject command title",
  Inbox: "items working agents rollup",
  AgentShown: "agentId title group subagents tasks face",
  AgentFace: "pack shape color box",
  FaceStyle: "motion",
  AgentRollup: "subagents tasks",
  InboxItem:
    "id tier kind agentId name cli project title what detail at session card requestId questionId url",
  InboxCard: "title",
  RemoteStatus: "pending",
  PendingDevice: "id name platform expiresAt",
  AgentQuestion: "id text choices viaNote",
  QuestionChoice: "label",
  CatalogAgent: "type label",
};
const SWIFT_ROOTS = [
  "DeviceView",
  "RemoteStatus",
  "DeviceAccess",
  "AgentQuestion",
  "CatalogAgent",
];

/** Types whose `Default` the app shows until the core has answered with its own values. */
const DEFAULTS = ["Preferences"];

/** Limits the clients check before they send, read from where the core enforces them. */
const LIMITS = [
  [`${WIRE}/protocol.rs`, "MAX_ATTACHMENT_BYTES"],
  [`${WIRE}/protocol.rs`, "DEFAULT_TERMINAL_FONT_SIZE"],
  [`${WIRE}/protocol.rs`, "MIN_TERMINAL_FONT_SIZE"],
  [`${WIRE}/protocol.rs`, "MAX_TERMINAL_FONT_SIZE"],
  [`${WIRE}/protocol.rs`, "DEFAULT_TEXT_SCALE"],
  [`${WIRE}/protocol.rs`, "MIN_TEXT_SCALE"],
  [`${WIRE}/protocol.rs`, "MAX_TEXT_SCALE"],
  [`${WIRE}/protocol.rs`, "MAX_PROJECT_DEPTH"],
  [`${CORE}/acp/mod.rs`, "MAX_ATTACHMENTS"],
];

const OUTPUTS = {
  mac: "src/api/generated/wire.ts",
  codehost: "src/codehost/types.generated.ts",
  jira: "src/plugins/jira/types.generated.ts",
  rundeck: "src/plugins/rundeck/types.generated.ts",
  aws: "src/plugins/aws/types.generated.ts",
  linear: "src/plugins/linear/types.generated.ts",
  database: "src/plugins/database/types.generated.ts",
  bruno: "src/plugins/bruno/types.generated.ts",
  slack: "src/plugins/slack/types.generated.ts",
  vercel: "src/plugins/vercel/types.generated.ts",
  signoz: "src/plugins/signoz/types.generated.ts",
  phone: "mobile/app/src/core/wire.generated.ts",
  notch: "src-tauri/notch/Sources/NotchKit/Wire.generated.swift",
};

const fail = (message) => {
  throw new Error(`generate-wire-types: ${message}`);
};

/** Reads the text from `open` to the bracket that closes it. */
function balanced(text, start) {
  const pairs = { "(": ")", "[": "]", "{": "}", "<": ">" };
  const stack = [];
  for (let index = start; index < text.length; index += 1) {
    const char = text[index];
    if (char === '"' || text.startsWith("//", index)) {
      index = text.indexOf(char === '"' ? '"' : "\n", index + 1);
      continue;
    }
    if (pairs[char]) stack.push(pairs[char]);
    else if (char === stack.at(-1)) {
      stack.pop();
      if (stack.length === 0) return index + 1;
    } else if (")]}".includes(char) && char !== ">") {
      fail(`unbalanced ${char} in ${text.slice(start, index + 1)}`);
    }
  }
  fail(`unclosed bracket in ${text.slice(start, start + 80)}`);
}

/** Splits at commas outside brackets and strings. */
function splitTop(text) {
  const parts = [];
  let depth = 0;
  let from = 0;
  for (let index = 0; index < text.length; index += 1) {
    const char = text[index];
    if (char === '"') index = text.indexOf('"', index + 1);
    else if (text.startsWith("//", index)) index = text.indexOf("\n", index);
    else if ("([{<".includes(char)) depth += 1;
    else if (")]}".includes(char) || (char === ">" && text[index - 1] !== "-"))
      depth -= 1;
    else if (char === "," && depth === 0) {
      parts.push(text.slice(from, index));
      from = index + 1;
    }
  }
  parts.push(text.slice(from));
  return parts.map((part) => part.trim()).filter(Boolean);
}

/** Doc lines, attributes and the rest, for one member or item. */
function readMember(text) {
  const docs = [];
  const attrs = [];
  let rest = text.trim();
  for (;;) {
    if (rest.startsWith("///")) {
      const end = rest.indexOf("\n");
      docs.push(rest.slice(3, end < 0 ? undefined : end).replace(/^ /, ""));
      rest = end < 0 ? "" : rest.slice(end + 1).trim();
    } else if (rest.startsWith("//")) {
      const end = rest.indexOf("\n");
      rest = end < 0 ? "" : rest.slice(end + 1).trim();
    } else if (rest.startsWith("#[")) {
      const end = balanced(rest, 1);
      attrs.push(rest.slice(2, end - 1).trim());
      rest = rest.slice(end).trim();
    } else return { docs, attrs, rest };
  }
}

/** The serde settings among `attrs`, as `{ key: value | true }`. */
function serde(attrs, where) {
  const settings = {};
  for (const attr of attrs) {
    if (attr.startsWith("cfg")) fail(`${where} is compiled conditionally`);
    if (!attr.startsWith("serde(")) continue;
    for (const part of splitTop(attr.slice(6, -1))) {
      const match = /^(\w+)(?:\s*=\s*"([^"]*)")?$/.exec(part);
      if (!match) fail(`${where} has serde setting ${part}`);
      settings[match[1]] = match[2] ?? true;
    }
  }
  return settings;
}

const KNOWN = {
  container: ["rename_all", "default", "tag", "rename_all_fields"],
  field: ["rename", "default", "skip_serializing_if", "skip", "alias"],
  variant: ["rename", "other", "alias"],
};

function only(settings, allowed, where) {
  for (const key of Object.keys(settings))
    if (!allowed.includes(key)) fail(`${where} uses serde(${key})`);
}

/** Every top-level struct, enum and type alias in a file, unparsed. */
function items(file, text) {
  const found = new Map();
  const declaration =
    /^(?:pub(?:\([^)]*\))?\s+)?(struct|enum|type)\s+(\w+)(<[^>]*>)?/;
  const lines = text.split("\n");
  let pending = [];
  for (let index = 0; index < lines.length; index += 1) {
    const line = lines[index];
    if (/^(\/\/\/|#\[)/.test(line)) {
      pending.push(line);
      if (line.startsWith("#[")) {
        let attr = line;
        while (
          [...attr].filter((c) => c === "[").length >
          [...attr].filter((c) => c === "]").length
        ) {
          index += 1;
          attr += `\n${lines[index]}`;
          pending.push(lines[index]);
        }
      }
      continue;
    }
    const match = declaration.exec(line);
    if (match) {
      const rest = lines.slice(index).join("\n");
      const head =
        match[0].length + rest.slice(match[0].length).search(/[{;(=]/);
      const end =
        rest[head] === "{" || rest[head] === "("
          ? balanced(rest, head)
          : rest.indexOf(";") + 1;
      const { docs, attrs } = readMember(pending.join("\n"));
      found.set(match[2], {
        file,
        name: match[2],
        kind: match[1],
        generics: match[3],
        docs,
        attrs,
        body: rest.slice(head, end),
      });
    }
    if (line.trim() !== "") pending = [];
  }
  return found;
}

const texts = await Promise.all(
  SOURCES.map((file) => readFile(resolve(root, file), "utf8")),
);
const sources = SOURCES.map((file, index) => items(file, texts[index]));

const limits = (
  await Promise.all(
    LIMITS.map(async ([file, name]) => {
      const text = await readFile(resolve(root, file), "utf8");
      const value = new RegExp(
        `^(?:pub )?const ${name}: (?:usize|u32|f64) = ([\\d_. *]+);$`,
        "m",
      ).exec(text)?.[1];
      if (!value) fail(`${file} has no number constant ${name}`);
      const product = value
        .split("*")
        .reduce(
          (total, part) => total * Number(part.trim().replaceAll("_", "")),
          1,
        );
      return `export const ${name} = ${product};`;
    }),
  )
).join("\n");

/** The client name of the type `name` used in `file`. */
function clientName(file, name) {
  return RENAMES[file]?.[name] ?? name;
}

function lookup(file, name) {
  const own = sources[SOURCES.indexOf(file)].get(name);
  if (own) return own;
  const all = sources.flatMap((source) =>
    source.has(name) ? [source.get(name)] : [],
  );
  if (all.length > 1)
    fail(`${name} in ${file} could be any of ${all.map((i) => i.file)}`);
  return all[0];
}

const declared = new Map();
for (const source of sources)
  for (const item of source.values()) {
    const name = clientName(item.file, item.name);
    declared.set(name, [...(declared.get(name) ?? []), item]);
  }

function byClientName(name) {
  const found = declared.get(name) ?? [];
  if (found.length !== 1)
    fail(
      `${name} is declared ${found.length} times: ${found.map((item) => item.file)}`,
    );
  return found[0];
}

const PRIMITIVES = {
  String: "string",
  PathBuf: "string",
  str: "string",
  bool: "bool",
  f32: "float",
  f64: "float",
  Value: "json",
  ...Object.fromEntries(
    "u8 u16 u32 u64 usize i8 i16 i32 i64 isize"
      .split(" ")
      .map((name) => [name, "int"]),
  ),
};

/** A Rust type as `{ kind, ... }`: primitive, option, list, map, tuple or named. */
function parseType(text, file) {
  text = text.trim().replace(/^&(?:'\w+\s+)?/, "");
  if (/^\(.*\)$/s.test(text))
    return {
      kind: "tuple",
      of: splitTop(text.slice(1, -1)).map((part) => parseType(part, file)),
    };
  const generic = /^([\w:]+)\s*<(.*)>$/s.exec(text);
  const path = (generic?.[1] ?? text).split("::").at(-1);
  const args = generic ? splitTop(generic[2]) : [];
  const one = () => {
    if (args.length !== 1) fail(`${text} needs one type argument`);
    return parseType(args[0], file);
  };
  if (path === "Option") return { kind: "option", of: one() };
  if (path === "Vec") return { kind: "list", of: one() };
  if (path === "Box") return one();
  if (path === "BTreeMap" || path === "HashMap") {
    if (args.length !== 2 || parseType(args[0], file).kind !== "string")
      fail(`${text} is not keyed by strings`);
    return { kind: "map", of: parseType(args[1], file) };
  }
  if (generic) fail(`${text} is generic`);
  if (PRIMITIVES[path]) return { kind: PRIMITIVES[path] };
  const item = lookup(file, path);
  if (!item) fail(`${text} in ${file} is not declared in any source`);
  if (item.kind === "type") {
    const alias = /^=\s*(.*);$/s.exec(item.body);
    return parseType(alias[1], item.file);
  }
  return { kind: "named", name: clientName(item.file, item.name) };
}

const words = (name) =>
  name
    .replace(/^r#/, "")
    .split("_")
    .flatMap((part) => part.split(/(?=[A-Z])/))
    .filter(Boolean)
    .map((word) => word.toLowerCase());

/** What serde's `rename_all` makes of a Rust name. */
function renameAll(name, rule, where) {
  const parts = words(name);
  const cap = (word) => word[0].toUpperCase() + word.slice(1);
  switch (rule) {
    case undefined:
      return name.replace(/^r#/, "");
    case "camelCase":
      return parts[0] + parts.slice(1).map(cap).join("");
    case "snake_case":
      return parts.join("_");
    case "kebab-case":
      return parts.join("-");
    case "lowercase":
      return parts.join("");
    default:
      fail(`${where} renames all to ${rule}`);
  }
}

/** A struct's or struct variant's fields. */
function parseFields(body, file, rule, where) {
  return splitTop(body.slice(1, -1)).flatMap((member) => {
    const { docs, attrs, rest } = readMember(member);
    if (!rest) return [];
    const match = /^(?:pub(?:\([^)]*\))?\s+)?([\w#]+)\s*:\s*(.+)$/s.exec(rest);
    if (!match) fail(`${where} has field ${rest}`);
    const settings = serde(attrs, `${where}.${match[1]}`);
    only(settings, KNOWN.field, `${where}.${match[1]}`);
    if (settings.skip) return [];
    const type = parseType(match[2], file);
    return {
      name: settings.rename ?? renameAll(match[1], rule, where),
      rust: match[1],
      docs,
      type,
      mayBeAbsent: settings.skip_serializing_if !== undefined,
      defaulted: settings.default !== undefined,
    };
  });
}

/** A declaration as the clients see its JSON. */
function parse(item) {
  const where = `${item.name} in ${item.file}`;
  if (item.generics) fail(`${where} is generic`);
  const settings = serde(item.attrs, where);
  only(settings, KNOWN.container, where);
  const name = clientName(item.file, item.name);
  if (item.kind === "struct") {
    if (!item.body.startsWith("{"))
      fail(`${where} is not a struct with named fields`);
    const fields = parseFields(
      item.body,
      item.file,
      settings.rename_all,
      where,
    );
    for (const field of fields) {
      const json = JSON_FIELDS[`${name}.${field.name}`];
      if (json) {
        if (field.type.kind !== "json")
          fail(`${name}.${field.name} is not JSON`);
        field.type = { kind: "named", name: json };
      }
    }
    return { name, docs: item.docs, kind: "struct", fields };
  }
  if (item.kind !== "enum") fail(`${where} is a ${item.kind}`);
  if (settings.tag) {
    const variants = splitTop(item.body.slice(1, -1)).map((member) => {
      const { docs, attrs, rest } = readMember(member);
      const match = /^(\w+)\s*(\{[\s\S]*\})?$/.exec(rest);
      if (!match) fail(`${where}::${rest} is not a unit or struct variant`);
      const variant = serde(attrs, `${where}::${match[1]}`);
      only(variant, KNOWN.variant, `${where}::${match[1]}`);
      return {
        name: variant.rename ?? renameAll(match[1], settings.rename_all, where),
        docs,
        fields: match[2]
          ? parseFields(
              match[2],
              item.file,
              settings.rename_all_fields,
              `${where}::${match[1]}`,
            )
          : [],
      };
    });
    return {
      name,
      docs: item.docs,
      kind: "tagged",
      tag: settings.tag,
      variants,
    };
  }
  if (settings.rename_all_fields)
    fail(`${where} renames fields but has no tag`);
  const variants = splitTop(item.body.slice(1, -1)).map((member) => {
    const { docs, attrs, rest } = readMember(member);
    if (!/^\w+$/.test(rest)) fail(`${where}::${rest} carries data`);
    const variant = serde(attrs, `${where}::${rest}`);
    only(variant, KNOWN.variant, `${where}::${rest}`);
    return {
      name: variant.rename ?? renameAll(rest, settings.rename_all, where),
      rust: rest,
      docs,
      other: variant.other === true,
      isDefault: attrs.includes("default"),
    };
  });
  return { name, docs: item.docs, kind: "strings", variants };
}

function named(type) {
  if (type.kind === "named") return [type.name];
  if (type.kind === "tuple") return type.of.flatMap(named);
  if (type.of) return named(type.of);
  return [];
}

/** The declarations `roots` need, each before the first that uses it. */
function closure(roots, fieldsOf = () => undefined) {
  const order = [];
  const seen = new Set();
  const visit = (name) => {
    if (seen.has(name)) return;
    seen.add(name);
    const declaration = parse(byClientName(name));
    const wanted = fieldsOf(name);
    if (declaration.kind === "struct" && wanted) {
      const missing = wanted.filter(
        (field) => !declaration.fields.some((f) => f.name === field),
      );
      if (missing.length) fail(`${name} has no ${missing.join(", ")}`);
      declaration.fields = declaration.fields.filter((field) =>
        wanted.includes(field.name),
      );
    }
    order.push(declaration);
    [
      ...(declaration.fields ?? []),
      ...(declaration.kind === "tagged"
        ? declaration.variants.flatMap((variant) => variant.fields)
        : []),
    ]
      .flatMap((field) => named(field.type))
      .forEach(visit);
  };
  roots.forEach(visit);
  return order;
}

function tsDocs(lines, indent = "") {
  if (lines.length === 0) return "";
  if (lines.length === 1) return `${indent}/** ${lines[0]} */\n`;
  return `${indent}/**\n${lines.map((line) => `${indent} * ${line}`.trimEnd()).join("\n")}\n${indent} */\n`;
}

/**
 * TypeScript for one client. The app reads the core's JSON as it is, so a
 * `None` is `null`; the phone drops nulls as it reads, so there they are absent.
 * The phone also hears hosts from older releases, which leave out what serde
 * fills in by default.
 */
function typescript(phone, roots, preamble) {
  const tsType = (type) => {
    switch (type.kind) {
      case "string":
        return "string";
      case "bool":
        return "boolean";
      case "int":
      case "float":
        return "number";
      case "json":
        return "unknown";
      case "named":
        return type.name;
      case "list": {
        const of = tsType(type.of);
        return /[ |]/.test(of) ? `(${of})[]` : `${of}[]`;
      }
      case "map":
        return `Record<string, ${tsType(type.of)}>`;
      case "tuple":
        return `[${type.of.map(tsType).join(", ")}]`;
      case "option":
        return phone ? tsType(type.of) : `${tsType(type.of)} | null`;
    }
  };
  const field = (f) => {
    const optional =
      f.mayBeAbsent || (phone && (f.type.kind === "option" || f.defaulted));
    const type = f.mayBeAbsent && f.type.kind === "option" ? f.type.of : f.type;
    return `${tsDocs(f.docs, "  ")}  ${f.name}${optional ? "?" : ""}: ${tsType(type)};`;
  };
  const tagged = (d) =>
    `${tsDocs(d.docs)}export type ${d.name} =\n${d.variants
      .map(
        (v) =>
          `${tsDocs(v.docs, "  ")}  | {\n  ${d.tag}: ${JSON.stringify(v.name)};\n${v.fields.map(field).join("\n")}\n}`,
      )
      .join("\n")};`;
  const declare = (d) =>
    d.kind === "struct"
      ? `${tsDocs(d.docs)}export interface ${d.name} {\n${d.fields.map(field).join("\n")}\n}`
      : d.kind === "tagged"
        ? tagged(d)
        : `${tsDocs(d.docs)}export type ${d.name} = ${d.variants
            .filter((v) => !v.other)
            .map((v) => JSON.stringify(v.name))
            .join(" | ")};`;
  return `// Generated by scripts/generate-wire-types.mjs from the core's Rust. Do not edit by hand.\n\n${preamble}${closure(roots).map(declare).join("\n\n")}\n`;
}

const swiftCase = (name) => renameAll(name.replaceAll("-", "_"), "camelCase");

/**
 * Swift for the notch. It decodes only the fields it reads, and a value the
 * core added after the notch was built reads as the enum's unknown case.
 */
function swift() {
  const swiftType = (type) => {
    switch (type.kind) {
      case "string":
        return "String";
      case "bool":
        return "Bool";
      case "int":
        return "Int";
      case "float":
        return "Double";
      case "named":
        return type.name;
      case "list":
        return `[${swiftType(type.of)}]`;
      case "map":
        return `[String: ${swiftType(type.of)}]`;
      case "option":
        return `${swiftType(type.of)}?`;
      default:
        fail(`the notch cannot read ${type.kind}`);
    }
  };
  const docs = (lines, indent) =>
    lines.map((line) => `${indent}/// ${line}`.trimEnd() + "\n").join("");
  const declare = (d) => {
    if (d.kind === "struct") {
      const fields = d.fields.map((f) => {
        if (!/^[a-z]\w*$/.test(f.name))
          fail(`${d.name}.${f.name} is no Swift name`);
        const type = swiftType(f.type);
        const optional = f.mayBeAbsent && !type.endsWith("?") ? "?" : "";
        return `${docs(f.docs, "    ")}    let ${f.name}: ${type}${optional}`;
      });
      return `${docs(d.docs, "")}struct ${d.name}: Codable, Hashable {\n${fields.join("\n")}\n}`;
    }
    if (d.kind !== "strings") fail(`the notch cannot read ${d.name}`);
    const fallback = d.variants.find((v) => v.other);
    const cases = d.variants
      .filter((v) => !v.other)
      .map((v) => {
        const name = swiftCase(v.name);
        const raw = name === v.name ? "" : ` = ${JSON.stringify(v.name)}`;
        return `${docs(v.docs, "    ")}    case ${name}${raw}`;
      });
    const unknown = fallback ? swiftCase(fallback.name) : "unknown";
    return `${docs(d.docs, "")}enum ${d.name}: String, Codable, Hashable {
${cases.join("\n")}
    /// One added after this build.
    case ${unknown}

    init(from decoder: Decoder) throws {
        self = Self(rawValue: try decoder.singleValueContainer().decode(String.self)) ?? .${unknown}
    }
}`;
  };
  return `// Generated by scripts/generate-wire-types.mjs from the core's Rust. Do not edit by hand.

import Foundation

${closure(SWIFT_ROOTS, (name) => SWIFT_FIELDS[name]?.split(" "))
  .map(declare)
  .join("\n\n")}
`;
}

/** The text of `impl Default for name`'s `Self { .. }`, if the file has one. */
function defaultBody(item) {
  const text = texts[SOURCES.indexOf(item.file)];
  const at = text.search(new RegExp(`impl Default for ${item.name}\\s*\\{`));
  if (at < 0) return null;
  const impl = text.slice(at, balanced(text, text.indexOf("{", at)));
  const fn = impl.search(/fn default\(\)\s*->\s*Self\s*\{/);
  const body = impl.slice(impl.indexOf("{", fn) + 1);
  const self = fn < 0 ? -1 : body.search(/\bSelf\s*\{/);
  if (self < 0) fail(`${item.name}'s Default does not build Self { .. }`);
  const open = body.indexOf("{", self);
  return body.slice(open, balanced(body, open));
}

/** A constant's value, `const NAME: T = value;` in `file`. */
function constant(file, name) {
  const text = texts[SOURCES.indexOf(file)];
  const value = new RegExp(
    `^(?:pub )?const ${name}:[^=]+=\\s*([\\s\\S]*?);$`,
    "m",
  ).exec(text)?.[1];
  if (value === undefined) fail(`${file} has no constant ${name}`);
  return value;
}

/** What a type is when nothing was said: the JSON of its `Default`. */
function defaultOfType(type, file) {
  switch (type.kind) {
    case "bool":
      return false;
    case "int":
    case "float":
      return 0;
    case "string":
      return "";
    case "option":
      return null;
    case "list":
      return [];
    case "map":
      return {};
    case "named":
      return defaultOf(byClientName(type.name));
  }
  fail(`${JSON.stringify(type)} in ${file} has no default`);
}

/** The JSON `expr` serializes to as a value of `type`, for the plain expressions defaults are written in. */
function evaluate(expr, type, file) {
  expr = expr.trim();
  const built = /^(.*)\.(?:to_owned|to_string|into)\(\)$/s.exec(expr);
  if (built) return evaluate(built[1], type, file);
  if (expr === "true" || expr === "false") return expr === "true";
  if (/^-?[\d_]+(\.[\d_]+)?$/.test(expr))
    return Number(expr.replaceAll("_", ""));
  if (/^"(?:[^"\\]|\\.)*"$/.test(expr)) return JSON.parse(expr);
  if (expr === "None") return null;
  const some = /^Some\((.*)\)$/s.exec(expr);
  if (some && type.kind === "option") return evaluate(some[1], type.of, file);
  if (/^(?:String|Vec|BTreeMap|HashMap)::new\(\)$/.test(expr))
    return defaultOfType(type, file);
  if (/^(?:[\w:]+::)?(?:default|Default::default)\(\)$/.test(expr))
    return defaultOfType(type, file);
  const indexed = /^([A-Z][A-Z0-9_]*)\[(\d+)\]$/.exec(expr);
  if (indexed) {
    const list = constant(file, indexed[1]);
    if (!list.startsWith("[")) fail(`${indexed[1]} in ${file} is not a list`);
    return evaluate(
      splitTop(list.slice(1, -1))[Number(indexed[2])],
      type,
      file,
    );
  }
  if (/^[A-Z][A-Z0-9_]*$/.test(expr))
    return evaluate(constant(file, expr), type, file);
  const variant = /^(?:[\w:]+::)?(\w+)::(\w+)$/.exec(expr);
  if (variant && type.kind === "named") {
    const declared = parse(byClientName(type.name));
    const found =
      declared.kind === "strings" &&
      declared.variants.find((v) => v.rust === variant[2]);
    if (!found) fail(`${expr} in ${file} is not a variant of ${type.name}`);
    return found.name;
  }
  fail(`the default ${expr} in ${file} is not a plain value`);
}

function defaultOf(item) {
  const declared = parse(item);
  if (declared.kind === "strings") {
    const chosen = declared.variants.find((v) => v.isDefault);
    if (!chosen) fail(`${item.name} has no #[default] variant`);
    return chosen.name;
  }
  if (declared.kind !== "struct")
    fail(`${item.name} has no default the app can copy`);
  const body = defaultBody(item);
  const given = new Map(
    (body ? splitTop(body.slice(1, -1)) : []).map((member) => {
      const match = /^(\w+)\s*:\s*([\s\S]+)$/.exec(member.trim());
      if (!match) fail(`${item.name}'s Default sets ${member}`);
      return [match[1], match[2]];
    }),
  );
  return Object.fromEntries(
    declared.fields.map((field) => {
      const expr = given.get(field.rust);
      if (body && expr === undefined)
        fail(`${item.name}'s Default leaves out ${field.rust}`);
      return [
        field.name,
        expr === undefined
          ? defaultOfType(field.type, item.file)
          : evaluate(expr, field.type, item.file),
      ];
    }),
  );
}

const defaults = DEFAULTS.map(
  (name) =>
    `/** What the core's \`${name}::default()\` serializes to. */\nexport const DEFAULT_${words(name).join("_").toUpperCase()}: ${name} = ${JSON.stringify(defaultOf(byClientName(name)))};`,
).join("\n\n");

async function emit(path, contents) {
  const full = resolve(root, path);
  if (!path.endsWith(".swift"))
    contents = await format(contents, {
      ...(await resolveConfig(full)),
      filepath: full,
    });
  let actual = null;
  try {
    actual = await readFile(full, "utf8");
  } catch (error) {
    if (error?.code !== "ENOENT") throw error;
  }
  if (actual === contents) return;
  if (check) fail(`${path} is stale: run pnpm wire:generate`);
  await mkdir(dirname(full), { recursive: true });
  await writeFile(full, contents);
}

await emit(
  OUTPUTS.mac,
  `${typescript(false, TS_ROOTS, `${limits}\n\n`)}\n${defaults}\n`,
);
await emit(OUTPUTS.codehost, typescript(false, CODEHOST_ROOTS, ""));
await emit(OUTPUTS.jira, typescript(false, JIRA_ROOTS, ""));
await emit(OUTPUTS.rundeck, typescript(false, RUNDECK_ROOTS, ""));
await emit(OUTPUTS.aws, typescript(false, AWS_ROOTS, ""));
await emit(OUTPUTS.linear, typescript(false, LINEAR_ROOTS, ""));
await emit(OUTPUTS.database, typescript(false, DATABASE_ROOTS, ""));
await emit(OUTPUTS.bruno, typescript(false, BRUNO_ROOTS, ""));
await emit(OUTPUTS.slack, typescript(false, SLACK_ROOTS, ""));
await emit(OUTPUTS.vercel, typescript(false, VERCEL_ROOTS, ""));
await emit(OUTPUTS.signoz, typescript(false, SIGNOZ_ROOTS, ""));
await emit(OUTPUTS.phone, typescript(true, TS_ROOTS, `${limits}\n\n`));
await emit(OUTPUTS.notch, swift());
