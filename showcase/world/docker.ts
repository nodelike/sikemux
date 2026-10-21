type Params = Record<string, unknown>;

const now = () => Math.floor(Date.now() / 1000);
const minutesAgo = (minutes: number) => now() - minutes * 60;
const iso = (secondsAgo: number) =>
  new Date(Date.now() - secondsAgo * 1000).toISOString();

const SHOP_DIR = "/Users/edon/code/shop";
const compose = (service: string) => ({
  project: "shop",
  service,
  number: 1,
  workingDir: SHOP_DIR,
  configFiles: [`${SHOP_DIR}/compose.yaml`],
});

const CONTAINERS = [
  {
    id: "4f1c2a9b7e03",
    name: "shop-web-1",
    image: "shop-web:dev",
    command: "npm run dev",
    created: minutesAgo(180),
    state: "running",
    status: "Up 3 hours",
    ports: [{ private: 3000, public: 3000, ip: "0.0.0.0", protocol: "tcp" }],
    compose: compose("web"),
  },
  {
    id: "8b2d6e0f1a44",
    name: "shop-api-1",
    image: "shop-api:dev",
    command: "uvicorn app.main:app --reload",
    created: minutesAgo(180),
    state: "running",
    status: "Up 3 hours (healthy)",
    ports: [{ private: 8000, public: 8000, ip: "0.0.0.0", protocol: "tcp" }],
    compose: compose("api"),
  },
  {
    id: "c93a5d7e2b18",
    name: "shop-db-1",
    image: "postgres:16-alpine",
    command: "docker-entrypoint.sh postgres",
    created: minutesAgo(180),
    state: "running",
    status: "Up 3 hours (healthy)",
    ports: [{ private: 5432, public: 5432, ip: "127.0.0.1", protocol: "tcp" }],
    compose: compose("db"),
  },
  {
    id: "1e7f0c4b9d62",
    name: "shop-worker-1",
    image: "shop-api:dev",
    command: "celery -A app.worker worker",
    created: minutesAgo(180),
    state: "exited",
    status: "Exited (1) 4 minutes ago",
    ports: [],
    compose: compose("worker"),
  },
  {
    id: "a05b8c3d6f91",
    name: "redis",
    image: "redis:7-alpine",
    command: "redis-server",
    created: minutesAgo(2_900),
    state: "running",
    status: "Up 2 days",
    ports: [{ private: 6379, public: 6379, ip: "127.0.0.1", protocol: "tcp" }],
    compose: null,
  },
];

const ENV: Record<string, string[]> = {
  "shop-api-1": ["DATABASE_URL", "REDIS_URL", "STRIPE_SECRET_KEY", "PATH"],
  "shop-worker-1": ["DATABASE_URL", "REDIS_URL", "PATH"],
  "shop-db-1": ["POSTGRES_PASSWORD", "POSTGRES_DB", "PGDATA", "PATH"],
};

function detail(id: string) {
  const container =
    CONTAINERS.find((each) => each.id === id || each.name === id) ??
    CONTAINERS[0]!;
  const crashed = container.state === "exited";
  return {
    id: container.id,
    name: container.name,
    image: container.image,
    imageId: `sha256:${container.id}aa`,
    created: iso(10_800),
    command: container.command.split(" "),
    entrypoint: [],
    workingDir: "/app",
    state: {
      status: container.state,
      running: !crashed,
      exitCode: crashed ? 1 : 0,
      error: null,
      oomKilled: false,
      startedAt: iso(crashed ? 400 : 10_800),
      finishedAt: crashed ? iso(240) : null,
      health: container.status.includes("healthy")
        ? { status: "healthy", failingStreak: 0, lastOutput: ["ok"] }
        : null,
    },
    restartCount: crashed ? 3 : 0,
    restartPolicy: "unless-stopped",
    env: ENV[container.name] ?? ["PATH"],
    mounts:
      container.name === "shop-db-1"
        ? [
            {
              kind: "volume",
              source: "/var/lib/docker/volumes/shop_pgdata/_data",
              destination: "/var/lib/postgresql/data",
              writable: true,
              volume: "shop_pgdata",
            },
          ]
        : [
            {
              kind: "bind",
              source: SHOP_DIR,
              destination: "/app",
              writable: true,
              volume: null,
            },
          ],
    networks: [
      {
        name: "shop_default",
        ip: crashed ? null : "172.18.0.4",
        aliases: [container.compose?.service ?? container.name],
      },
    ],
    ports: container.ports,
    labels: container.compose
      ? {
          "com.docker.compose.project": "shop",
          "com.docker.compose.service": container.compose.service,
        }
      : {},
    compose: container.compose,
  };
}

const WORKER_LOG: [string, string][] = [
  ["stdout", " -------------- celery@4f1c v5.4.0 (opalescent)"],
  ["stdout", "[tasks]"],
  ["stdout", "  . app.worker.send_receipt"],
  ["stdout", "  . app.worker.sync_inventory"],
  [
    "stderr",
    "[2026-10-18 10:21:04,118: INFO/MainProcess] Connected to redis://redis:6379/0",
  ],
  [
    "stderr",
    "[2026-10-18 10:21:05,402: ERROR/MainProcess] consumer: Cannot connect to postgresql://shop-db:5432/shop",
  ],
  [
    "stderr",
    'psycopg.OperationalError: connection to server at "shop-db" (172.18.0.3), port 5432 failed: FATAL:  password authentication failed for user "shop"',
  ],
  ["stderr", "Traceback (most recent call last):"],
  ["stderr", '  File "/app/app/worker.py", line 18, in <module>'],
  ["stderr", "    engine = create_engine(settings.database_url)"],
  ["stderr", "celery.exceptions.WorkerShutdown: database unreachable"],
];

const API_LOG: [string, string][] = [
  ["stdout", "INFO:     Will watch for changes in these directories: ['/app']"],
  [
    "stdout",
    "INFO:     Uvicorn running on http://0.0.0.0:8000 (Press CTRL+C to quit)",
  ],
  ["stdout", "INFO:     Started reloader process [1] using WatchFiles"],
  ["stdout", "INFO:     Application startup complete."],
  [
    "stdout",
    'INFO:     172.18.0.1:51322 - "GET /api/products HTTP/1.1" 200 OK',
  ],
  ["stdout", 'INFO:     172.18.0.1:51322 - "GET /api/cart HTTP/1.1" 200 OK'],
  ["stderr", "WARNING:  /api/checkout took 1.84s"],
  [
    "stdout",
    'INFO:     172.18.0.1:51340 - "POST /api/checkout HTTP/1.1" 201 Created',
  ],
];

export function dockerLogLines(id: string) {
  const name = detail(id).name;
  const lines = name === "shop-worker-1" ? WORKER_LOG : API_LOG;
  return {
    lines: lines.map(([stream, text], index) => ({
      at: iso(lines.length - index),
      stream,
      text,
    })),
    ended: name === "shop-worker-1",
  };
}

const IMAGES = [
  {
    id: "9d1e4f7a2c30",
    tags: ["shop-web:dev"],
    created: minutesAgo(190),
    size: 612_000_000,
    containers: 1,
    dangling: false,
  },
  {
    id: "6c2b8e1f0d57",
    tags: ["shop-api:dev"],
    created: minutesAgo(190),
    size: 431_000_000,
    containers: 2,
    dangling: false,
  },
  {
    id: "e4a7d2c9b1f8",
    tags: [],
    created: minutesAgo(1_400),
    size: 428_000_000,
    containers: 0,
    dangling: true,
  },
  {
    id: "2f9c0b6e8a13",
    tags: ["postgres:16-alpine"],
    created: minutesAgo(20_000),
    size: 243_000_000,
    containers: 1,
    dangling: false,
  },
  {
    id: "7b3e5a1d9c04",
    tags: ["redis:7-alpine"],
    created: minutesAgo(30_000),
    size: 41_000_000,
    containers: 1,
    dangling: false,
  },
];

const VOLUMES = [
  {
    name: "shop_pgdata",
    driver: "local",
    created: iso(900_000),
    size: 184_000_000,
    usedBy: ["shop-db-1"],
    anonymous: false,
    composeProject: "shop",
  },
  {
    name: "shop_node_modules",
    driver: "local",
    created: iso(900_000),
    size: 391_000_000,
    usedBy: ["shop-web-1"],
    anonymous: false,
    composeProject: "shop",
  },
  {
    name: "3fa91c0e7b2d",
    driver: "local",
    created: iso(2_000_000),
    size: 12_000_000,
    usedBy: [],
    anonymous: true,
    composeProject: null,
  },
];

const stats = () =>
  CONTAINERS.filter((each) => each.state === "running").map((each, index) => ({
    id: each.id,
    name: each.name,
    cpuPercent: [12.4, 3.1, 1.8, 0.4][index] ?? 0.2,
    memory:
      [318_000_000, 142_000_000, 96_000_000, 9_000_000][index] ?? 8_000_000,
    memoryLimit: 6_000_000_000,
    netRx: 18_400_000,
    netTx: 4_100_000,
    diskRead: 52_000_000,
    diskWrite: 9_800_000,
    pids: [24, 9, 7, 5][index] ?? 3,
  }));

function stacks() {
  const shop = CONTAINERS.filter((each) => each.compose);
  const services = [...new Set(shop.map((each) => each.compose!.service))]
    .sort()
    .map((name) => ({
      name,
      containers: shop.filter((each) => each.compose!.service === name),
    }));
  return [
    {
      project: "shop",
      workingDir: SHOP_DIR,
      configFiles: [`${SHOP_DIR}/compose.yaml`],
      services,
      running: shop.filter((each) => each.state === "running").length,
      total: shop.length,
    },
  ];
}

export const DOCKER: Record<string, (params: Params) => unknown> = {
  status: () => ({
    running: true,
    message: null,
    socket: "/Users/edon/.colima/default/docker.sock",
    source: "colima",
    engine: {
      name: "colima",
      version: "29.2.1",
      apiVersion: "1.53",
      os: "Ubuntu 24.04 LTS",
      arch: "aarch64",
      cpus: 4,
      memory: 6_200_000_000,
      containers: CONTAINERS.length,
      running: CONTAINERS.filter((each) => each.state === "running").length,
      paused: 0,
      stopped: 1,
      images: IMAGES.length,
    },
  }),
  containers: () => CONTAINERS,
  container: ({ id }) => detail(String(id)),
  act: ({ id }) => detail(String(id)),
  remove: () => null,
  logs: ({ id }) => ({ ...dockerLogLines(String(id)), filtered: 0 }),
  stats,
  stacks,
  actOnStack: ({ project }) => ({ project, done: [], failed: [] }),
  images: () => IMAGES,
  removeImage: () => ({ untagged: [], deleted: 1, spaceReclaimed: 0 }),
  pruneImages: () => ({
    untagged: [],
    deleted: 1,
    spaceReclaimed: 428_000_000,
  }),
  volumes: () => VOLUMES,
  removeVolume: () => null,
  pruneVolumes: () => ({
    deleted: ["3fa91c0e7b2d"],
    spaceReclaimed: 12_000_000,
  }),
  disk: () => ({
    images: {
      count: 5,
      active: 4,
      size: 1_755_000_000,
      reclaimable: 428_000_000,
    },
    containers: {
      count: 5,
      active: 4,
      size: 38_000_000,
      reclaimable: 2_000_000,
    },
    volumes: {
      count: 3,
      active: 2,
      size: 587_000_000,
      reclaimable: 12_000_000,
    },
    buildCache: {
      count: 41,
      active: 0,
      size: 2_310_000_000,
      reclaimable: 2_310_000_000,
    },
  }),
};
