// Shared chrome for the instance admin console.
//
// The guard here is presentation only. Every /api/admin route is gated by
// the SuperAdmin extractor server-side; hiding the UI keeps a non-operator
// from staring at a page of 403s, it is not the security boundary.

import Link from "next/link";
import { useRouter } from "next/router";
import type { ReactNode } from "react";
import {
  useIsSuperAdmin,
  useOrchestratorSession,
} from "../../lib/useOrchestratorToken";

const TABS = [
  { href: "/admin", label: "Overview" },
  { href: "/admin/deployments", label: "Deployments" },
  { href: "/admin/members", label: "Members" },
];

export function AdminLayout({
  title,
  children,
}: {
  title: string;
  children: ReactNode;
}) {
  const isSuperAdmin = useIsSuperAdmin();
  const { isLoading } = useOrchestratorSession();
  const router = useRouter();

  // While the session is in flight we know nothing. Rendering the denial
  // here would flash "not available" at operators on every hard navigation.
  if (isLoading) return null;

  if (!isSuperAdmin) {
    return (
      <div className="mx-auto max-w-2xl px-6 py-16 text-center">
        <h1 className="text-xl font-semibold">Not available</h1>
        <p className="mt-2 text-content-secondary">
          This area is limited to instance operators. Ask an existing operator
          to grant you access.
        </p>
        <Link href="/" className="mt-6 inline-block underline">
          Back to your teams
        </Link>
      </div>
    );
  }

  return (
    <div className="mx-auto w-full max-w-7xl px-6 py-8">
      <header className="mb-6">
        <p className="text-xs uppercase tracking-wide text-content-secondary">
          Instance admin
        </p>
        <h1 className="text-2xl font-semibold">{title}</h1>
      </header>
      <nav className="mb-6 flex gap-4 border-b" aria-label="Instance admin">
        {TABS.map((tab) => (
          <Link
            key={tab.href}
            href={tab.href}
            aria-current={router.pathname === tab.href ? "page" : undefined}
            className={
              router.pathname === tab.href
                ? "border-b-2 border-content-primary pb-2 font-medium"
                : "pb-2 text-content-secondary hover:text-content-primary"
            }
          >
            {tab.label}
          </Link>
        ))}
      </nav>
      {children}
    </div>
  );
}
