// Canonical URL selection.
//
// The card's two jobs after a mutation are to stop lying about state: saving
// must clear "unsaved", and restarting must clear "not live yet". Neither
// should need a page reload — the restart response says nothing about
// canonical URLs, so the card has to re-read.

import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { SWRConfig } from "swr";
import { CanonicalUrlsCard } from "../components/CanonicalUrlsCard";

const mockGet = jest.fn();
const mockSet = jest.fn();
const mockRestart = jest.fn();
const mockListDomains = jest.fn();

jest.mock("../lib/config", () => ({
  orchestratorUrl: () => "http://orchestrator.test",
}));

jest.mock("../lib/useOrchestratorToken", () => ({
  useAccessToken: () => "pat_test",
}));

jest.mock("../lib/orchestratorApi", () => ({
  getCanonicalUrls: (...a: unknown[]) => mockGet(...a),
  setCanonicalUrls: (...a: unknown[]) => mockSet(...a),
  restartDeployment: (...a: unknown[]) => mockRestart(...a),
  listCustomDomains: (...a: unknown[]) => mockListDomains(...a),
  createCustomDomain: jest.fn(),
  deleteCustomDomain: jest.fn(),
  verifyCustomDomain: jest.fn(),
  retryCustomDomain: jest.fn(),
}));

const DEFAULTS = {
  currentUrl: "https://shiny-ibis.defyhost.com",
  currentSiteUrl: "https://shiny-ibis-site.defyhost.com",
  desiredUrl: null,
  desiredSiteUrl: null,
  defaultUrl: "https://shiny-ibis.defyhost.com",
  defaultSiteUrl: "https://shiny-ibis-site.defyhost.com",
  restartPending: false,
};

function renderCard() {
  return render(
    <SWRConfig value={{ provider: () => new Map(), dedupingInterval: 0 }}>
      <CanonicalUrlsCard deploymentId={7} deploymentName="shiny-ibis" />
    </SWRConfig>,
  );
}

beforeEach(() => {
  mockGet.mockReset();
  mockSet.mockReset();
  mockRestart.mockReset();
  mockListDomains.mockReset();
  mockGet.mockResolvedValue({ ...DEFAULTS });
  mockListDomains.mockResolvedValue({
    domains: [
      {
        id: 1,
        deploymentId: 7,
        domain: "backend.dayqwest.app",
        certState: "active",
        createdAt: 0,
        kind: "api",
        tlsMode: "acme",
        lastError: null,
      },
    ],
    targetHost: "defyhost.com",
    routingEnabled: true,
  });
});

test("offers attached domains of the matching surface alongside the default", async () => {
  renderCard();
  await waitFor(() =>
    expect(screen.getByLabelText("Database (Convex API)")).toBeInTheDocument(),
  );
  const select = screen.getByLabelText(
    "Database (Convex API)",
  ) as HTMLSelectElement;
  const values = Array.from(select.options).map((o) => o.value);
  expect(values).toContain("https://shiny-ibis.defyhost.com");
  expect(values).toContain("https://backend.dayqwest.app");
  // A `site` domain must not be selectable as the database URL.
  expect(values).not.toContain("https://api.dayqwest.app");
});

test("a canonical URL whose domain was deleted collapses to the default and can be saved", async () => {
  // The saved canonical names a domain that no longer exists, so it is not
  // among the options. The control has to show the default *and* agree that
  // this differs from what's stored — otherwise Save stays greyed out and the
  // dangling value can't be cleared from the UI.
  mockGet.mockResolvedValue({
    ...DEFAULTS,
    desiredUrl: "https://deleted.dayqwest.app",
    restartPending: true,
  });
  mockListDomains.mockResolvedValue({
    domains: [],
    targetHost: "defyhost.com",
    routingEnabled: true,
  });

  renderCard();
  await waitFor(() =>
    expect(screen.getByLabelText("Database (Convex API)")).toBeInTheDocument(),
  );

  const select = screen.getByLabelText(
    "Database (Convex API)",
  ) as HTMLSelectElement;
  expect(select.value).toBe("https://shiny-ibis.defyhost.com");
  expect(screen.getByRole("button", { name: "Save" })).toBeEnabled();
});

test("saving reflects the new state without a reload", async () => {
  const user = userEvent.setup();
  mockSet.mockResolvedValue({
    ...DEFAULTS,
    desiredUrl: "https://backend.dayqwest.app",
    restartPending: true,
  });
  renderCard();
  await waitFor(() => expect(mockGet).toHaveBeenCalled());

  await user.selectOptions(
    screen.getByLabelText("Database (Convex API)"),
    "https://backend.dayqwest.app",
  );
  await user.click(screen.getByRole("button", { name: "Save" }));

  // The pending-restart notice appears straight from the save response.
  await waitFor(() =>
    expect(
      screen.getByRole("button", { name: "Restart deployment" }),
    ).toBeInTheDocument(),
  );
});

test("restarting re-reads, so the pending notice clears on its own", async () => {
  const user = userEvent.setup();
  mockGet.mockResolvedValueOnce({
    ...DEFAULTS,
    desiredUrl: "https://backend.dayqwest.app",
    restartPending: true,
  });
  mockRestart.mockResolvedValue({});
  // After the restart the backend really is serving the new URL.
  mockGet.mockResolvedValue({
    ...DEFAULTS,
    currentUrl: "https://backend.dayqwest.app",
    desiredUrl: "https://backend.dayqwest.app",
    restartPending: false,
  });

  renderCard();
  await waitFor(() =>
    expect(
      screen.getByRole("button", { name: "Restart deployment" }),
    ).toBeInTheDocument(),
  );

  await user.click(screen.getByRole("button", { name: "Restart deployment" }));
  await user.click(
    screen.getAllByRole("button", { name: "Restart deployment" }).pop()!,
  );

  await waitFor(() => expect(mockRestart).toHaveBeenCalled());
  // Re-read rather than left stale — this is what a reload used to be for.
  await waitFor(() => expect(mockGet).toHaveBeenCalledTimes(2));
  await waitFor(() =>
    expect(
      screen.queryByRole("button", { name: "Restart deployment" }),
    ).toBeNull(),
  );
});
