import Link from "next/link";
import { useRouter } from "next/router";

export type NavBarItem = {
  label: string;
  href: string;
};

/**
 * Which item, if any, a path selects.
 *
 * Most specific wins. Items here nest — "Home" is `/t/<team>` and "Team
 * Settings" is `/t/<team>/settings` — so a plain prefix test lights up both on
 * every settings route, which is what left the Home underline showing while
 * Team Settings was open. Matching on the longest href that the path is under
 * makes exactly one item active.
 *
 * Exported for tests: the bug is in the predicate, not the markup.
 */
export function activeNavHref(
  path: string,
  items: NavBarItem[],
): string | undefined {
  // Strip query/hash: `asPath` carries them and they never affect which
  // section is open.
  const pathname = path.split(/[?#]/)[0].replace(/\/+$/, "") || "/";
  let best: string | undefined;
  for (const { href } of items) {
    const base = href.replace(/\/+$/, "") || "/";
    const matches = pathname === base || pathname.startsWith(`${base}/`);
    if (matches && (best === undefined || base.length > best.length)) {
      best = base;
    }
  }
  return best;
}

export function NavBar({ items }: { items: NavBarItem[] }) {
  const router = useRouter();
  const selected = activeNavHref(router.asPath, items);
  return (
    <nav className="flex h-full items-center" aria-label="Section navigation">
      {items.map((item) => {
        const active = selected === item.href.replace(/\/+$/, "");
        return (
          <Link
            key={item.href}
            href={item.href}
            aria-current={active ? "page" : undefined}
            className={`relative flex h-full items-center px-3 text-sm transition-colors ${
              active
                ? "font-medium text-content-primary"
                : "text-content-secondary hover:text-content-primary"
            }`}
          >
            {item.label}
            {active && (
              <span
                aria-hidden
                className="absolute inset-x-1.5 bottom-0 h-0.5 bg-content-primary"
              />
            )}
          </Link>
        );
      })}
    </nav>
  );
}
