import release from "@sikemux/protocol/release.json";

const { production, dev } = release.accounts;

/** Where the web app finds the API and Clerk. Both are public; a dev server talks to the API on this machine. */
export const config = import.meta.env.DEV
  ? {
      apiUrl: `http://127.0.0.1:${dev.apiPort}`,
      clerkPublishableKey: dev.clerkPublishableKey,
    }
  : production;
