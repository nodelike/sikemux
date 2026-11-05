import type { Services } from "../app.ts";
import { Hono } from "hono";

import { requireIdentity, requireRecentSignIn, type AuthEnv } from "../auth.ts";
import { toDevice } from "../devices/register.ts";
import { readBody } from "../http.ts";
import { callerAddress, clientAddress, limit, perUser } from "../limits.ts";
import {
  approveHost,
  finishSignIn,
  pendingHost,
  pollSignIn,
  refuseHost,
  startSignIn,
} from "./codes.ts";

/**
 * Signing in a host that has no browser, as in OAuth's device grant: the host shows a user code,
 * the person types it on their phone or the web app and approves it, and the host then registers
 * itself with the same signed challenge as any device. The host keeps no sign-in token afterwards.
 */
export function hostSignInRoutes({
  database: { db },
  verifier,
  limiter,
  appOrigin,
  places,
}: Pick<
  Services,
  "database" | "verifier" | "limiter" | "appOrigin" | "places"
>) {
  const perAddress = (name: string, perMinute: number) =>
    limit<AuthEnv>(limiter, name, perMinute, clientAddress);
  const signedIn = requireIdentity(verifier, db);
  const perHostSignIn = perUser(limiter, "host-sign-in", 30);

  const fromHost = requireIdentity(verifier, db, { accept: ["host"] });

  return (
    new Hono<AuthEnv>()
      .get("/:userCode", signedIn, perHostSignIn, async (c) =>
        c.json(
          await pendingHost(
            db,
            c.get("identity").userId,
            c.req.param("userCode"),
            places,
          ),
        ),
      )
      .post(
        "/:userCode/approve",
        signedIn,
        requireRecentSignIn(),
        perHostSignIn,
        async (c) => {
          const host = await approveHost(
            db,
            c.get("identity"),
            c.req.param("userCode"),
          );
          c.get("log").info({ host: host.key.slice(0, 8) }, "approved a host");
          return c.body(null, 204);
        },
      )
      // "Install Sikemux here" in the desktop app: it runs the installer on a server over the
      // person's own SSH, reads the code `sikemux login` prints there, and has its host approve that
      // code. A host signs only for the person in its own app, never for an agent or any other
      // program, so this trusts the host's signature as the person's click (ADR 0018).
      .post(
        "/:userCode/approve-from-host",
        fromHost,
        perHostSignIn,
        async (c) => {
          const host = await approveHost(
            db,
            c.get("identity"),
            c.req.param("userCode"),
          );
          c.get("log").info({ host: host.key.slice(0, 8) }, "approved a host");
          return c.json(host);
        },
      )
      .delete("/:userCode", signedIn, perHostSignIn, async (c) => {
        await refuseHost(db, c.get("identity"), c.req.param("userCode"));
        return c.body(null, 204);
      })
      .post("/", perAddress("host-sign-in-start", 10), async (c) => {
        const start = await readBody(c, "HostSignInStart");
        const requestedFrom = callerAddress(c)?.slice(0, 64) ?? null;
        return c.json(
          await startSignIn(db, start, requestedFrom, appOrigin),
          201,
        );
      })
      .post("/poll", perAddress("host-sign-in-poll", 60), async (c) => {
        const { code } = await readBody(c, "HostSignInPoll");
        return c.json(await pollSignIn(db, code));
      })
      .post("/finish", perAddress("host-sign-in-finish", 10), async (c) => {
        const finish = await readBody(c, "HostSignInFinish");
        const { row: device, added } = await finishSignIn(db, finish);
        c.get("log").info(
          { key: device.key, added },
          "signed in a host by code",
        );
        return c.json(toDevice(device), added ? 201 : 200);
      })
  );
}
