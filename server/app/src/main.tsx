import { ClerkProvider } from "@clerk/react";
import { lazy, StrictMode, Suspense } from "react";
import { createRoot } from "react-dom/client";

import { config } from "./config.ts";
import { isSharePath } from "./navigation.ts";

/* A shared chat draws with the Mac's styles and the account pages with their own, so each loads only its own. */
const Page = isSharePath(location.pathname)
  ? lazy(() => import("./share/SharePage.tsx"))
  : lazy(() => import("./App.tsx").then(({ App }) => ({ default: App })));

const root = document.getElementById("root");
if (!root) throw new Error("index.html has no #root");
createRoot(root).render(
  <StrictMode>
    <ClerkProvider
      publishableKey={config.clerkPublishableKey}
      prefetchUI={false}
      afterSignOutUrl="/"
    >
      <Suspense fallback={null}>
        <Page />
      </Suspense>
    </ClerkProvider>
  </StrictMode>,
);
