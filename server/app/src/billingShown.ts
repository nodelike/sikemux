import { ApiProblem } from "@shared/account/api";
import { useEffect, useState } from "react";

import { useClient } from "./client.ts";

/** Whether billing answers this account: it does for staff, and for everyone once it is live. */
export function useBillingShown(
  ready: boolean,
  version: number,
): boolean | undefined {
  const client = useClient();
  const [shown, setShown] = useState<boolean>();
  useEffect(() => {
    if (!ready) return;
    let current = true;
    void client("see your plan")
      .then((routes) => routes.getBilling())
      .then(
        () => current && setShown(true),
        (error: unknown) =>
          current &&
          setShown((last) =>
            error instanceof ApiProblem && error.status === 404
              ? false
              : (last ?? false),
          ),
      );
    return () => {
      current = false;
    };
  }, [ready, client, version]);
  return shown;
}
