import { fireEvent, render, screen } from "@testing-library/react";
import { BreakGlassModal } from "../components/admin/BreakGlassModal";
import type { BreakGlassGrant, FleetEntry } from "../lib/adminApi";

const deployment: FleetEntry = {
  id: 7,
  name: "tenant-prod",
  deploymentType: "prod",
  intendedState: "running",
  tier: "S16",
  url: "http://tenant-prod.localhost",
  creationTime: 0,
  teamId: 1,
  teamSlug: "acme",
  projectId: 2,
  projectSlug: "app",
  actualState: "running",
  drifted: false,
};

describe("BreakGlassModal", () => {
  it("requires a reason before access can be granted", () => {
    const onConfirm = jest.fn();
    render(
      <BreakGlassModal
        deployment={deployment}
        grant={null}
        onConfirm={onConfirm}
        onClose={jest.fn()}
      />,
    );

    const grant = screen.getByRole("button", { name: /grant access/i });
    expect(grant).toBeDisabled();

    // Whitespace is not a reason.
    fireEvent.change(screen.getByLabelText(/reason for access/i), {
      target: { value: "   " },
    });
    expect(grant).toBeDisabled();

    fireEvent.change(screen.getByLabelText(/reason for access/i), {
      target: { value: "investigating ticket 4711" },
    });
    expect(grant).toBeEnabled();
    fireEvent.click(grant);
    expect(onConfirm).toHaveBeenCalledWith("investigating ticket 4711");
  });

  it("warns that the tenant will see this, before the operator commits", () => {
    render(
      <BreakGlassModal
        deployment={deployment}
        grant={null}
        onConfirm={jest.fn()}
        onClose={jest.fn()}
      />,
    );
    // The whole accountability design rests on the operator knowing this
    // going in rather than discovering it afterwards.
    expect(
      screen.getByText(/see this in their own audit log/i),
    ).toBeInTheDocument();
    expect(screen.getByText(/acme/)).toBeInTheDocument();
  });

  it("shows the key once, with a countdown, after access is granted", () => {
    const grant: BreakGlassGrant = {
      deployment: "tenant-prod",
      url: "http://tenant-prod.localhost",
      adminKey: "prod:tenant-prod|secret-value",
      expiresAt: Date.now() + 15 * 60_000,
      tenantNotified: true,
    };
    render(
      <BreakGlassModal
        deployment={deployment}
        grant={grant}
        onConfirm={jest.fn()}
        onClose={jest.fn()}
      />,
    );

    expect(
      screen.getByText("prod:tenant-prod|secret-value"),
    ).toBeInTheDocument();
    // A countdown, so an operator with a dashboard open is not surprised
    // when the key lapses mid-session.
    expect(screen.getByText(/^1[0-5]:\d{2}$/)).toBeInTheDocument();
    // The reason field is gone — this state is display-only.
    expect(
      screen.queryByLabelText(/reason for access/i),
    ).not.toBeInTheDocument();
  });
});
