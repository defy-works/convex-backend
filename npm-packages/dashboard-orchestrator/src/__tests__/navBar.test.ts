// Exactly one section tab may be active.
//
// "Home" is `/t/<team>` and "Team Settings" is `/t/<team>/settings`, so the
// settings routes are nested *under* the Home href. A plain prefix test lit
// both up, leaving the Home underline showing while Team Settings was open.

import { activeNavHref, NavBarItem } from "../components/NavBar";

const items: NavBarItem[] = [
  { label: "Home", href: "/t/self-hosted" },
  { label: "Team Settings", href: "/t/self-hosted/settings" },
];

test("Home is active on the team landing page", () => {
  expect(activeNavHref("/t/self-hosted", items)).toBe("/t/self-hosted");
});

test("Team Settings takes over on the settings index", () => {
  expect(activeNavHref("/t/self-hosted/settings", items)).toBe(
    "/t/self-hosted/settings",
  );
});

test("Team Settings stays active on its sub-pages, and Home does not", () => {
  for (const page of ["members", "usage", "access-tokens", "audit-log"]) {
    expect(activeNavHref(`/t/self-hosted/settings/${page}`, items)).toBe(
      "/t/self-hosted/settings",
    );
  }
});

test("query strings and trailing slashes don't change the section", () => {
  expect(
    activeNavHref("/t/self-hosted/settings/members?invited=1", items),
  ).toBe("/t/self-hosted/settings");
  expect(activeNavHref("/t/self-hosted/settings/", items)).toBe(
    "/t/self-hosted/settings",
  );
  expect(activeNavHref("/t/self-hosted#top", items)).toBe("/t/self-hosted");
});

test("a team whose slug prefixes another team's does not match it", () => {
  // `/t/self-hosted-2` must not count as being under `/t/self-hosted`.
  expect(activeNavHref("/t/self-hosted-2", items)).toBeUndefined();
});

test("an unrelated path selects nothing", () => {
  expect(activeNavHref("/t/other-team/settings", items)).toBeUndefined();
  expect(activeNavHref("/profile", items)).toBeUndefined();
});
