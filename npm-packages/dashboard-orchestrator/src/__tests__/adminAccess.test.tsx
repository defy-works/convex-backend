import { render, screen } from "@testing-library/react";
import { AdminLayout } from "../components/admin/AdminLayout";

// `jest.spyOn` cannot redefine these exports — SWC compiles them to
// non-configurable getters — so mock the module with a factory that reads
// mutable state, matching the pattern in `loginSessionCache.test.tsx`.
let mockIsSuperAdmin = false;
let mockIsLoading = false;

jest.mock("next/router", () => ({
  useRouter: () => ({ pathname: "/admin", push: jest.fn() }),
}));

jest.mock("../lib/useOrchestratorToken", () => ({
  useIsSuperAdmin: () => mockIsSuperAdmin,
  useOrchestratorSession: () => ({ isLoading: mockIsLoading }),
}));

function mockSession({
  isSuperAdmin,
  isLoading,
}: {
  isSuperAdmin: boolean;
  isLoading: boolean;
}) {
  mockIsSuperAdmin = isSuperAdmin;
  mockIsLoading = isLoading;
}

describe("AdminLayout", () => {
  afterEach(() => mockSession({ isSuperAdmin: false, isLoading: false }));

  it("renders children for an operator", () => {
    mockSession({ isSuperAdmin: true, isLoading: false });

    render(
      <AdminLayout title="Overview">
        <p>admin content</p>
      </AdminLayout>,
    );
    expect(screen.getByText("admin content")).toBeInTheDocument();
    // The nav is what makes the section navigable; without it the layout is
    // just a heading.
    expect(
      screen.getByRole("navigation", { name: /instance admin/i }),
    ).toBeInTheDocument();
  });

  it("refuses to render children for a non-operator", () => {
    mockSession({ isSuperAdmin: false, isLoading: false });

    render(
      <AdminLayout title="Overview">
        <p>admin content</p>
      </AdminLayout>,
    );
    expect(screen.queryByText("admin content")).not.toBeInTheDocument();
    expect(screen.getByText(/instance operators/i)).toBeInTheDocument();
  });

  it("shows nothing rather than a denial while the session loads", () => {
    // The denial must not flash at an operator mid-navigation, so an
    // unknown session renders neither branch.
    mockSession({ isSuperAdmin: false, isLoading: true });

    render(
      <AdminLayout title="Overview">
        <p>admin content</p>
      </AdminLayout>,
    );
    expect(screen.queryByText("admin content")).not.toBeInTheDocument();
    expect(screen.queryByText(/instance operators/i)).not.toBeInTheDocument();
  });
});
