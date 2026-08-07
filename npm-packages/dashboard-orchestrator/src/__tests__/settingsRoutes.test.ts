// Every settings page the sidebar links to must have a route here.
//
// The sidebar is shared code (`dashboard-common`), but the routes are per
// dashboard. Nothing connects the two, so a page added upstream — or one
// removed here — silently produces a link that 404s. That is exactly how
// "Usage Limits" ended up broken. This test is the connection.

import fs from "fs";
import path from "path";
import {
  DEPLOYMENT_SETTINGS_PAGES_AND_NAMES,
  getAllowedDeploymentSettingsPages,
} from "@common/layouts/settingsSidebarPages";

const SETTINGS_DIR = path.join(
  __dirname,
  "..",
  "pages",
  "t",
  "[team]",
  "[project]",
  "[deploymentName]",
  "settings",
);

// The widest set the sidebar can render: a deployment with components, and a
// backend that owns its admin keys (which is every orchestrator deployment).
const reachablePages = getAllowedDeploymentSettingsPages({
  nents: [{}],
  showAdminKeys: true,
});

// `general` is the settings index, not a named file.
const routeFileFor = (page: string) =>
  page === "general" ? "index.tsx" : `${page}.tsx`;

test("every sidebar-reachable settings page has a route", () => {
  const missing = reachablePages.filter(
    (page) => !fs.existsSync(path.join(SETTINGS_DIR, routeFileFor(page))),
  );
  expect(missing).toEqual([]);
});

test("snapshots stays out of the sidebar, since we have no route for it", () => {
  // Guard the other direction: if upstream ever stops filtering `snapshots`
  // out, this fails instead of shipping a dead link.
  expect(reachablePages).not.toContain("snapshots");
  expect(Object.keys(DEPLOYMENT_SETTINGS_PAGES_AND_NAMES)).toContain(
    "snapshots",
  );
});
