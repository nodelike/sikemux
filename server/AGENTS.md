# Accounts backend

The root `AGENTS.md` applies here too.

- The accounts backend is in `server/`: the API (`server/api`), the web app at
  app.sikemux.com (`server/app`), the shared protocol (`server/protocol`) and what runs
  them on citadel (`server/deploy`). It is its own pnpm workspace: never add it to the
  root install, scripts or checks. Run `pnpm check` in `server/`.
- The protocol is generated from `server/protocol/schema`. Change the schema and run
  `pnpm protocol:generate`; the TypeScript, the OpenAPI document and the Rust in
  `sikemux-wire` and `sikemux-corekit` (each one's `accounts/protocol.rs`, with the core's other
  generated modules beside it) are build output that CI compares against it. Within `/v1` the
  schema only grows: never remove or rename a field, route or enum value a released app
  reads.
- Migrations in `server/api/migrations` are never edited once merged. A change that
  removes something comes in two steps: stop using it in one release, drop it in a later
  one, so a rollback always finds the schema it expects.
- `server/api/schema.dbml` draws the schema the migrations build, for dbdiagram.io. After
  adding a migration run `pnpm schema:generate` in `server/` and commit it alongside; the
  API tests fail while it is stale.
- Merging to `main` deploys the API and the web app to production. What `server/deploy`
  holds reaches citadel only through `setup-citadel.sh`, and citadel's watch says on Discord
  when the two differ. You may push it,
  run `server/deploy` scripts against citadel, and set up its config without asking,
  after `agentctl guard` passes. Never print secrets, and ask first before anything
  destructive or irreversible, such as dropping data or deleting accounts.
- A Paddle account answers to one API. A subscription a buyer checked out on their own, with
  no checkout the API opened, is cancelled and refunded. One paid on a checkout opened with the
  account's key that the API has no record of is left alone: its events are refused, for Paddle
  to send again, with an error in the log. A second API on the account would put every sale of
  its own there. Production holds the sandbox account whose key is in citadel's `api.env`: never
  start another API on that key or run `paddle-setup` against that account with another origin.
