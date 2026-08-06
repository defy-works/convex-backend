import { getAllowedDeploymentSettingsPages } from "./settingsSidebarPages";

describe("getAllowedDeploymentSettingsPages", () => {
  test("keeps the Components tab visible while component metadata is loading", () => {
    expect(
      getAllowedDeploymentSettingsPages({
        nents: undefined,
        showAdminKeys: true,
      }),
    ).toContain("components");
  });

  test("hides the Components tab when loaded component metadata is empty", () => {
    expect(
      getAllowedDeploymentSettingsPages({
        nents: [],
        showAdminKeys: true,
      }),
    ).not.toContain("components");
  });

  test("hides the Admin Keys tab when the deployment backend does not own admin keys", () => {
    expect(
      getAllowedDeploymentSettingsPages({
        nents: [{ name: "_App" }],
        showAdminKeys: false,
      }),
    ).not.toContain("admin-keys");
  });

  test("shows the Admin Keys tab when the deployment backend owns admin keys", () => {
    expect(
      getAllowedDeploymentSettingsPages({
        nents: [{ name: "_App" }],
        showAdminKeys: true,
      }),
    ).toContain("admin-keys");
  });

  // Usage Limits used to be feature-flagged off via `usageLimitsEnabled`.
  // Upstream removed that flag from DeploymentInfo when the feature shipped,
  // so the page is now unconditionally available.
  test("shows the Usage Limits tab", () => {
    expect(
      getAllowedDeploymentSettingsPages({
        nents: [{ name: "_App" }],
        showAdminKeys: true,
      }),
    ).toContain("usage-limits");
  });
});
