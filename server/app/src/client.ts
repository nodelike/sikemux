import { useAuth } from "@clerk/react";
import { useCallback } from "react";

import { api, signedInToken } from "./api.ts";

export function useClient() {
  const { getToken } = useAuth();
  return useCallback(
    async (what: string) => api(await signedInToken(getToken, what)),
    [getToken],
  );
}
