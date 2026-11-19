import { connectApi } from "@shared/account/api";
import { useEffect, useState } from "react";

import { config } from "./config.ts";

/** The name of the org an invite link joins, read without signing in; null while unread, or when the link would not join. */
export function useInviteOrg(token: string): string | null {
  const [name, setName] = useState<string | null>(null);
  useEffect(() => {
    if (!token) return;
    let current = true;
    connectApi({
      url: config.apiUrl,
      offline: "Can't reach Sikemux.",
    })
      .previewOrgInvite({ token })
      .then(
        ({ orgName }) => current && setName(orgName),
        () => undefined,
      );
    return () => {
      current = false;
    };
  }, [token]);
  return name;
}
