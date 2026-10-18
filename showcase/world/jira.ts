type Params = Record<string, unknown>;

const SITE = "acme-labs.atlassian.net";

const iso = (hoursAgo: number) =>
  new Date(Date.now() - hoursAgo * 3_600_000).toISOString();

const AVATAR_HUES: Record<string, number> = {
  "u-edon": 262,
  "u-maya": 18,
  "u-sam": 150,
  "u-lena": 330,
  "u-jonas": 200,
  "u-ana": 45,
};

const person = (accountId: string, name: string) => ({
  accountId,
  name,
  avatarUrl:
    accountId in AVATAR_HUES ? `https://avatars.acme.test/${accountId}` : null,
});

const svgAddress = (svg: string) => `data:image/svg+xml;base64,${btoa(svg)}`;

/** A drawn stand-in for a person's photo: a soft gradient with a head and shoulders. */
const faceOf = (url: string) => {
  const hue = AVATAR_HUES[url.split("/").pop() ?? ""] ?? 220;
  return svgAddress(
    `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 48 48"><defs><linearGradient id="g" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="hsl(${hue} 55% 62%)"/><stop offset="1" stop-color="hsl(${hue + 30} 50% 38%)"/></linearGradient></defs><rect width="48" height="48" fill="url(#g)"/><circle cx="24" cy="19" r="8.5" fill="hsl(${hue} 30% 92%)"/><path d="M8 48c2-11 9-16 16-16s14 5 16 16z" fill="hsl(${hue} 30% 92%)"/></svg>`,
  );
};

const PROJECT_TILES: Record<string, [number, string]> = {
  "10401": [215, "M14 30l10-14 10 14z"],
  "10402": [150, "M12 18h24v14H12z"],
  "10403": [28, "M24 12a12 12 0 1 0 0.01 0z"],
  "10404": [290, "M12 24l12-12 12 12-12 12z"],
};

/** A drawn project picture: a coloured tile with a simple white shape. */
const tileOf = (url: string) => {
  const id = /avatar\/(\d+)/.exec(url)?.[1] ?? "";
  const [hue, shape] = PROJECT_TILES[id] ?? [220, "M14 14h20v20H14z"];
  return svgAddress(
    `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 48 48"><rect width="48" height="48" rx="8" fill="hsl(${hue} 60% 50%)"/><path d="${shape}" fill="white" fill-opacity="0.9"/></svg>`,
  );
};

const projectAvatar = (id: string) =>
  `https://${SITE}/rest/api/3/universal_avatar/view/type/project/avatar/${id}?size=medium`;

const EDON = person("u-edon", "Edon Ramos");
const MAYA = person("u-maya", "Maya Chen");
const SAM = person("u-sam", "Sam Okafor");
const PRIYA = person("u-priya", "Priya Nair");
const LENA = person("u-lena", "Lena Fischer");
const JONAS = person("u-jonas", "Jonas Weber");
const THEO = person("u-theo", "Theo Laurent");
const ANA = person("u-ana", "Ana Silva");
const PEOPLE = [EDON, MAYA, SAM, PRIYA, LENA, JONAS, THEO, ANA];

const COLUMNS = [
  { name: "To Do", category: "new", statusIds: ["1"] },
  { name: "In Progress", category: "indeterminate", statusIds: ["3"] },
  { name: "In Review", category: "indeterminate", statusIds: ["4"] },
  { name: "Done", category: "done", statusIds: ["5"] },
  { name: "Deployed", category: "done", statusIds: ["6"] },
] as const;

const SPRINT = "Orbit Sprint 24";

const EPICS = {
  offline: { key: "ORB-150", summary: "Offline itineraries", color: "color_4" },
  checkout: {
    key: "ORB-151",
    summary: "Checkout for every market",
    color: "color_12",
  },
  alerts: {
    key: "ORB-152",
    summary: "Price alerts and reminders",
    color: "color_6",
  },
  accounts: { key: "ORB-153", summary: "Traveller accounts", color: "color_8" },
  polish: { key: "ORB-154", summary: "Release polish", color: null },
} as const;

type Epic = (typeof EPICS)[keyof typeof EPICS];

/** Days from today, as Jira writes a due date. */
const dayFromNow = (days: number) => {
  const date = new Date(Date.now() + days * 86_400_000);
  const pad = (value: number) => String(value).padStart(2, "0");
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`;
};

const PLANS: Record<number, [Epic | null, number | null]> = {
  212: [EPICS.offline, -6],
  218: [EPICS.polish, 9],
  224: [EPICS.checkout, null],
  227: [EPICS.offline, -2],
  205: [EPICS.checkout, 4],
  209: [EPICS.alerts, -11],
  215: [null, 12],
  198: [EPICS.accounts, null],
  201: [EPICS.checkout, 2],
  184: [EPICS.offline, -3],
  190: [EPICS.accounts, 20],
  176: [EPICS.polish, -30],
  179: [null, null],
  231: [EPICS.offline, 6],
  233: [EPICS.alerts, null],
  236: [EPICS.accounts, -1],
  238: [EPICS.checkout, 15],
};

type Row = [
  number,
  string,
  number,
  string,
  string,
  (typeof PEOPLE)[number] | null,
  number,
];

const ROWS: Row[] = [
  [
    212,
    "Offline mode keeps the last synced itinerary readable without a connection",
    0,
    "Story",
    "High",
    EDON,
    30,
  ],
  [
    218,
    "Trip sharing link expires after seven days instead of thirty",
    0,
    "Bug",
    "Medium",
    MAYA,
    52,
  ],
  [
    224,
    "Currency picker remembers the last choice per traveller",
    0,
    "Task",
    "Low",
    null,
    70,
  ],
  [
    227,
    "Seat map loads in under a second on a cold start",
    0,
    "Story",
    "Highest",
    SAM,
    8,
  ],
  [
    205,
    "Checkout asks for the passport number only for international legs",
    1,
    "Story",
    "High",
    EDON,
    3,
  ],
  [
    209,
    "Price alerts batch into one push a day per route",
    1,
    "Task",
    "Medium",
    THEO,
    11,
  ],
  [
    215,
    "Calendar export writes the right time zone for overnight flights",
    1,
    "Bug",
    "High",
    LENA,
    19,
  ],
  [
    198,
    "Booking history paginates by month on the account page",
    2,
    "Story",
    "Medium",
    JONAS,
    6,
  ],
  [
    201,
    "Retry the payment webhook with backoff when the bank times out",
    2,
    "Task",
    "High",
    EDON,
    14,
  ],
  [
    184,
    "Search results show the baggage allowance on each fare",
    3,
    "Story",
    "Medium",
    MAYA,
    40,
  ],
  [
    190,
    "Duplicate traveller profiles merge on sign in",
    3,
    "Bug",
    "Low",
    SAM,
    60,
  ],
  [
    176,
    "Itinerary emails render the map at retina size",
    4,
    "Task",
    "Lowest",
    ANA,
    90,
  ],
  [
    179,
    "Gift cards apply before taxes on mixed carts",
    4,
    "Bug",
    "Medium",
    null,
    120,
  ],
  [
    231,
    "Boarding passes open from the lock screen",
    0,
    "Story",
    "Medium",
    EDON,
    4,
  ],
  [
    233,
    "Quiet hours hold price alerts until the morning",
    1,
    "Task",
    "Low",
    MAYA,
    26,
  ],
  [236, "Sign in with a passkey on the web", 2, "Story", "High", MAYA, 33],
  [238, "Saved cards show their expiry month", 3, "Task", "Medium", EDON, 48],
];

const issue = ([
  number,
  summary,
  column,
  issueType,
  priority,
  assignee,
  hours,
]: Row) => {
  const status = COLUMNS[column]!;
  const key = `ORB-${number}`;
  const [epic, due] = PLANS[number] ?? [null, null];
  return {
    key,
    summary,
    status: status.name,
    statusCategory: status.category,
    priority,
    assignee,
    issueType,
    sprint: SPRINT,
    updated: iso(hours),
    due: due === null ? null : dayFromNow(due),
    parent: epic,
    url: `https://${SITE}/browse/${key}`,
    column,
  };
};

const ISSUES = ROWS.map(issue);

const byPeople = (params: Params) => {
  const people = (params.people as string[] | undefined) ?? [];
  const mine = params.mine === true;
  const unassigned = params.unassigned === true;
  if (!people.length && !mine && !unassigned) return ISSUES;
  return ISSUES.filter(
    (each) =>
      (mine && each.assignee?.accountId === EDON.accountId) ||
      (unassigned && !each.assignee) ||
      (each.assignee && people.includes(each.assignee.accountId)),
  );
};

const search = ({ jql }: Params) => {
  const text = String(jql ?? "");
  const words = /text ~ "([^"]*)"/.exec(text)?.[1]?.toLowerCase();
  let found = ISSUES;
  if (words)
    found = ISSUES.filter((each) => each.summary.toLowerCase().includes(words));
  else if (/assignee = currentUser\(\)/.test(text))
    found = ISSUES.filter((each) => each.assignee === EDON);
  else if (/assignee is EMPTY/.test(text))
    found = ISSUES.filter((each) => !each.assignee && each.column < 3);
  else if (/statusCategory != Done/.test(text))
    found = ISSUES.filter((each) => each.column < 3);
  return {
    issues: [...found].sort(
      (a, b) => Date.parse(b.updated) - Date.parse(a.updated),
    ),
    next: null,
  };
};

const DESCRIPTION = `Travellers on a train or a plane open the app to check their gate and find a spinner. The itinerary they synced an hour ago should still be there.

### What done looks like

- [x] The last synced itinerary is kept on the device
- [ ] It opens without a connection, marked with when it was synced
- [ ] Boarding passes open offline too

Tickets and seats are read only while offline; changing a booking still needs a connection.`;

const COMMENTS = [
  {
    id: "c1",
    author: "Maya Chen",
    created: iso(28),
    body: "The boarding pass needs the barcode cached as an image, not the payload. The airline SDK will not render offline.",
  },
  {
    id: "c2",
    author: "Edon Ramos",
    created: iso(6),
    body: "Agreed. I will store the rendered pass beside the itinerary and drop both when the trip ends.",
  },
];

const full = (key: string) => {
  const found = ISSUES.find((each) => each.key === key) ?? ISSUES[0]!;
  const position = ISSUES.indexOf(found);
  return {
    ...found,
    project: "Orbit",
    reporter: PEOPLE[(position + 1) % PEOPLE.length],
    labels: position % 2 === 0 ? ["mobile", "offline"] : ["web"],
    created: iso(24 * 9),
    description:
      position === 0
        ? DESCRIPTION
        : `${found.summary}.\n\nSteps and acceptance are in the linked design.`,
    comments: position === 0 ? COMMENTS : COMMENTS.slice(0, 1),
    commentCount: position === 0 ? 4 : 1,
    transitions: COLUMNS.filter((_, index) => index !== found.column).map(
      (column, index) => ({
        id: String(index + 11),
        name: column.name,
        to: column.name,
        toCategory: column.category,
      }),
    ),
    subtasks: [],
    blockedBy: [],
    blocks: [],
  };
};

export const JIRA: Record<string, (params: Params) => unknown> = {
  status: () => ({
    configured: true,
    sites: [{ host: SITE, displayName: "Acme Labs", default: true }],
    ok: true,
    authFailed: false,
    message: null,
    browserSignIn: true,
  }),
  filters: () => [
    {
      id: "10021",
      name: "Bugs this sprint",
      jql: `type = Bug AND sprint in openSprints()`,
    },
    { id: "10022", name: "Waiting on review", jql: `status = "In Review"` },
  ],
  projects: () => [
    { key: "ORB", name: "Orbit", avatarUrl: projectAvatar("10401") },
    { key: "PAY", name: "Payments", avatarUrl: projectAvatar("10402") },
    { key: "ATL", name: "Atlas Web", avatarUrl: projectAvatar("10403") },
    { key: "SUP", name: "Support desk", avatarUrl: projectAvatar("10404") },
  ],
  boards: () => [
    {
      id: 7,
      name: "Orbit board",
      kind: "scrum",
      project: "ORB",
      projectKey: "ORB",
    },
    {
      id: 8,
      name: "Orbit releases",
      kind: "kanban",
      project: "ORB",
      projectKey: "ORB",
    },
    {
      id: 9,
      name: "Payments kanban",
      kind: "kanban",
      project: "PAY",
      projectKey: "PAY",
    },
    {
      id: 11,
      name: "Atlas sprint board",
      kind: "scrum",
      project: "ATL",
      projectKey: "ATL",
    },
    {
      id: 12,
      name: "Edon's board",
      kind: "kanban",
      project: "Edon Ramos",
      projectKey: null,
    },
  ],
  board: (params) => {
    const shown = byPeople(params);
    return {
      id: Number(params.id ?? 7),
      name: "Orbit board",
      kind: "scrum",
      sprint: {
        id: 24,
        name: SPRINT,
        end: new Date(Date.now() + 4 * 86_400_000).toISOString(),
        goal: "Offline itineraries ship to every traveller on iOS and Android.",
      },
      columns: COLUMNS.map((column, index) => ({
        name: column.name,
        statusIds: [...column.statusIds],
        issues: shown.filter((each) => each.column === index),
      })),
      truncated: false,
    };
  },
  search,
  issue: ({ key }) => full(String(key)),
  assignable: () => PEOPLE,
  keys: () => [],
  image: ({ url }) => faceOf(String(url)),
  siteImage: ({ url }) => tileOf(String(url)),
  moveIssue: () => null,
  transition: () => ({ status: null, transitions: [] }),
  assign: () => null,
  comment: ({ body }) => ({
    id: "c9",
    author: "Edon Ramos",
    created: iso(0),
    body,
  }),
};
