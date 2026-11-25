import { lazy, type ComponentType } from "react";

const org = () => import("./orgPages.ts");
const moneyed = () => import("./moneyPages.ts");

const named = <Props>(pages: Promise<ComponentType<Props>>) =>
  pages.then((component) => ({ default: component }));

export const AuditLogPage = lazy(() =>
  named(org().then((all) => all.AuditLogPage)),
);
export const DetailsPage = lazy(() =>
  named(org().then((all) => all.DetailsPage)),
);
export const DomainsPage = lazy(() =>
  named(org().then((all) => all.DomainsPage)),
);
export const GroupsPage = lazy(() =>
  named(org().then((all) => all.GroupsPage)),
);
export const OrgBillingPage = lazy(() =>
  named(org().then((all) => all.OrgBillingPage)),
);
export const PeoplePage = lazy(() =>
  named(org().then((all) => all.PeoplePage)),
);
export const PolicyPage = lazy(() =>
  named(org().then((all) => all.PolicyPage)),
);
export const BillingPage = lazy(() =>
  named(moneyed().then((all) => all.BillingPage)),
);
export const PricingPage = lazy(() =>
  named(moneyed().then((all) => all.PricingPage)),
);
export const ShedNotHost = lazy(() =>
  named(moneyed().then((all) => all.ShedNotHost)),
);
export const StartPage = lazy(() =>
  named(moneyed().then((all) => all.StartPage)),
);

/** Fetched ahead, so a page opens at once when it's picked. */
export function loadPages() {
  const load = () => {
    org().catch(() => {});
    moneyed().catch(() => {});
  };
  if (!("requestIdleCallback" in window)) {
    const timer = setTimeout(load, 1500);
    return () => clearTimeout(timer);
  }
  const idle = requestIdleCallback(load, { timeout: 3000 });
  return () => cancelIdleCallback(idle);
}
