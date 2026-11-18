import { execFileSync, spawn, spawnSync } from "node:child_process";
import { mkdirSync, realpathSync, writeFileSync } from "node:fs";
import { createServer } from "node:net";
import { dirname, join } from "node:path";
import { createInterface } from "node:readline";
import { fileURLToPath } from "node:url";
import pg from "pg";

import joinVector from "../../protocol/vectors/join.json" with { type: "json" };

/**
 * The accounts API a film's cores sign in to: a throwaway Postgres, the stand-in sign-in and the
 * real API, all in the folder it is given and gone when its input closes. It prints the API's
 * address, then answers lines on its input: `approve CODE [PERSON]` approves that host sign-in as
 * the person or another stand-in person, `member PERSON ORG` has the person bring PERSON into an
 * org they own as a member, `role ROLE ORG` has the owner of an org the person doesn't own give them ROLE in it,
 * `plan active|lapsed ORG` sets the org's plan, which hosts read at their next role change, and
 * `policy every_member|admins_approve ORG` has the org's owner set who may add its rules.
 *
 *   node dev/film-account.ts FOLDER '{"person":"ines","orgs":[...]}'
 *
 * Each org is `{ name, role: "admin" | "user", plan: "active" | "lapsed" }`; the person owns the
 * ones they administer, and another stand-in person owns and invites them to the rest.
 */

type Org = { name: string; role: "admin" | "user"; plan: "active" | "lapsed" };
type Account = { person?: string; orgs?: Org[] };

const API = dirname(dirname(fileURLToPath(import.meta.url)));
const [folderArg, given] = process.argv.slice(2);
if (!folderArg) throw new Error("usage: film-account.ts FOLDER [ACCOUNT_JSON]");
const folder: string = folderArg;
const account: Account = given ? JSON.parse(given) : {};
const person = account.person ?? "ines";
const other = person === "tomas" ? "priya" : "tomas";
mkdirSync(folder, { recursive: true });

const children: ReturnType<typeof spawn>[] = [];
let postgresData: string | null = null;
let pgCtl = "";

function stop() {
  for (const child of children) child.kill("SIGTERM");
  if (postgresData)
    spawnSync(pgCtl, ["-D", postgresData, "-m", "immediate", "stop"], {
      stdio: "ignore",
    });
}
process.on("exit", stop);
for (const signal of ["SIGINT", "SIGTERM"] as const)
  process.on(signal, () => process.exit(1));

function freePort(): Promise<number> {
  return new Promise((resolve, reject) => {
    const server = createServer();
    server.once("error", reject);
    server.listen(0, "127.0.0.1", () => {
      const address = server.address();
      server.close(() =>
        typeof address === "object" && address
          ? resolve(address.port)
          : reject(new Error("no free port")),
      );
    });
  });
}

function startPostgres(port: number): string {
  const which = spawnSync("which", ["postgres"], { encoding: "utf8" });
  if (which.status !== 0)
    throw new Error("a signed-in film needs Postgres 16 or newer on PATH");
  const bin = dirname(realpathSync(which.stdout.trim()));
  pgCtl = join(bin, "pg_ctl");
  const data = join(folder, "postgres");
  execFileSync(
    join(bin, "initdb"),
    ["-D", data, "-U", "postgres", "-A", "trust", "--no-sync", "-E", "UTF8"],
    { stdio: "ignore" },
  );
  execFileSync(
    pgCtl,
    [
      "-D",
      data,
      "-l",
      join(folder, "postgres.log"),
      "-w",
      "-o",
      `-p ${port} -c listen_addresses=127.0.0.1 -c unix_socket_directories= -c fsync=off`,
      "start",
    ],
    { stdio: "ignore" },
  );
  postgresData = data;
  return `postgresql://postgres@127.0.0.1:${port}/postgres`;
}

function startNode(args: string[], env: Record<string, string>, log: string) {
  const out = join(folder, log);
  writeFileSync(out, "");
  const child = spawn(process.execPath, args, {
    cwd: API,
    env: { PATH: process.env.PATH ?? "", HOME: folder, ...env },
    stdio: ["ignore", "pipe", "pipe"],
  });
  const append = (chunk: Buffer) => writeFileSync(out, chunk, { flag: "a" });
  child.stdout?.on("data", append);
  child.stderr?.on("data", append);
  children.push(child);
  return child;
}

async function waitFor(url: string) {
  const started = Date.now();
  while (Date.now() - started < 30_000) {
    try {
      if ((await fetch(url)).ok) return;
    } catch {
      // Not listening yet.
    }
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  throw new Error(`${url} never answered; see the logs in ${folder}`);
}

const databaseUrl = startPostgres(await freePort());
execFileSync(process.execPath, ["src/main.ts", "migrate"], {
  cwd: API,
  env: { PATH: process.env.PATH ?? "", DATABASE_URL: databaseUrl },
  stdio: "ignore",
});

const clerkPort = await freePort();
const clerk = `http://127.0.0.1:${clerkPort}`;
const apiPort = await freePort();
const api = `http://127.0.0.1:${apiPort}`;
const appOrigin = process.env.APP_ORIGIN ?? api;
startNode(
  ["dev/stand-in-clerk.ts"],
  { PORT: String(clerkPort), APP_ORIGIN: appOrigin },
  "stand-in-clerk.log",
);
const joinKey = join(folder, "join-signing-key.pem");
writeFileSync(joinKey, joinVector.privateKeyPem);
startNode(
  ["src/main.ts", "serve"],
  {
    DATABASE_URL: databaseUrl,
    CLERK_ISSUER: clerk,
    CLERK_MAC_CLIENT_ID: "film",
    // Only read by the stand-in Clerk above, which takes any key.
    CLERK_SECRET_KEY: "sk_test_standin",
    JOIN_SIGNING_KEY_FILE: joinKey,
    JOIN_SIGNING_KEY_ID: "dev-1",
    PORT: String(apiPort),
    API_ORIGIN: api,
    APP_ORIGIN: appOrigin,
    BILLING_PROVIDER: "stand-in",
    LOG_LEVEL: "warn",
  },
  "api.log",
);
await waitFor(`${clerk}/.well-known/jwks.json`);
await waitFor(`${api}/v1/health`);

const db = new pg.Client({ connectionString: databaseUrl });
await db.connect();

const people = (await (await fetch(`${clerk}/people`)).json()) as {
  id: string;
}[];
await db.query(
  "insert into users (id, staff) select unnest($1::text[]), true on conflict (id) do update set staff = true",
  [people.map((one) => one.id)],
);

async function as<Answer = unknown>(
  key: string,
  path: string,
  init: RequestInit = {},
): Promise<Answer> {
  const { token } = (await (
    await fetch(`${clerk}/token?as=${key}`)
  ).json()) as {
    token: string;
  };
  const response = await fetch(`${api}${path}`, {
    ...init,
    headers: {
      authorization: `Bearer ${token}`,
      "content-type": "application/json",
      ...init.headers,
    },
  });
  if (!response.ok)
    throw new Error(
      `${init.method ?? "GET"} ${path} as ${key}: ${response.status} ${await response.text()}`,
    );
  return (response.status === 204 ? null : response.json()) as Promise<Answer>;
}

const seeded = new Map<
  string,
  { id: string; owner: string; customer: string }
>();
for (const org of account.orgs ?? []) {
  const owner = org.role === "admin" ? person : other;
  const { id } = await as<{ id: string }>(owner, "/v1/orgs", {
    method: "POST",
    body: JSON.stringify({ name: org.name }),
  });
  const { rows } = await db.query(
    "insert into billing_customers (org_id, provider) values ($1, 'stand-in') returning id",
    [id],
  );
  const now = new Date();
  await db.query(
    `insert into subscriptions (customer_id, provider_subscription_id, product, interval, quantity,
       status, current_period_end, cancel_at_period_end, started_at, ended_at, provider_updated_at)
     values ($1, $2, 'team', 'month', 5, 'active', $3, false, $4, null, $4)`,
    [
      rows[0].id,
      `sub_film_${id}`,
      new Date(now.getTime() + 30 * 86_400_000),
      now,
    ],
  );
  if (owner !== person) {
    const { token } = await as<{ token: string }>(
      other,
      `/v1/orgs/${id}/invites`,
      {
        method: "POST",
        body: JSON.stringify({ role: org.role }),
      },
    );
    await as(person, "/v1/orgs/join", {
      method: "POST",
      body: JSON.stringify({ token }),
    });
  }
  seeded.set(org.name, { id, owner, customer: rows[0].id });
  if (org.plan === "lapsed")
    await db.query(
      "update subscriptions set status = 'ended', ended_at = now() where customer_id = $1",
      [rows[0].id],
    );
}

console.log(JSON.stringify({ api }));

async function setRole(name: string, role: string) {
  const org = seeded.get(name);
  if (!org || org.owner === person)
    throw new Error(`${name} is not an org another person owns`);
  const { rows } = await db.query(
    "select id from org_users where org_id = $1 and role <> 'owner'",
    [org.id],
  );
  await as(org.owner, `/v1/orgs/${org.id}/members/${rows[0].id}/role`, {
    method: "PUT",
    body: JSON.stringify({ role }),
  });
}

async function setPlan(name: string, plan: string) {
  const org = seeded.get(name);
  if (!org) throw new Error(`no org named ${name}`);
  await db.query(
    plan === "lapsed"
      ? "update subscriptions set status = 'ended', ended_at = now() where customer_id = $1"
      : "update subscriptions set status = 'active', ended_at = null where customer_id = $1",
    [org.customer],
  );
}

async function addMember(member: string, name: string) {
  const org = seeded.get(name);
  if (!org || org.owner !== person)
    throw new Error(`${name} is not an org the person owns`);
  const { token } = await as<{ token: string }>(
    person,
    `/v1/orgs/${org.id}/invites`,
    { method: "POST", body: JSON.stringify({ role: "user" }) },
  );
  await as(member, "/v1/orgs/join", {
    method: "POST",
    body: JSON.stringify({ token }),
  });
}

async function setPolicy(name: string, ruleAdding: string) {
  const org = seeded.get(name);
  if (!org) throw new Error(`no org named ${name}`);
  await as(org.owner, `/v1/orgs/${org.id}/policy`, {
    method: "PUT",
    body: JSON.stringify({ ruleAdding, asking: true, dailyQuestions: 30 }),
  });
}

for await (const line of createInterface({ input: process.stdin })) {
  const [verb, arg, ...rest] = line.trim().split(" ");
  if (!verb || !arg) continue;
  if (verb === "approve")
    await as(rest[0] ?? person, `/v1/host-sign-ins/${arg}/approve`, {
      method: "POST",
    });
  else if (verb === "member") await addMember(arg, rest.join(" "));
  else if (verb === "role") await setRole(rest.join(" "), arg);
  else if (verb === "plan") await setPlan(rest.join(" "), arg);
  else if (verb === "policy") await setPolicy(rest.join(" "), arg);
  else continue;
  console.log(`${verb === "approve" ? "approved" : "done"} ${arg}`);
}
await db.end();
process.exit(0);
