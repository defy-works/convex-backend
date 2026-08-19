// A token created on Team Settings > Access Tokens has to be readable back by
// the same page.
//
// `POST /v1/create_personal_access_token` writes a MEMBER-scoped row:
// `member_id = <caller>`, `team_id = NULL`
// (crates/orchestrator/src/routes/management/tokens.rs). The page used to read
// the list back from `GET /api/dashboard/teams/{team_id}/access_tokens`, whose
// SQL is `WHERE team_id = $1` (SELECT_TOKENS_BY_TEAM in
// crates/orchestrator/src/storage/access_tokens.rs). A member-scoped row can
// never match a team-scoped filter, so every token the user created was
// written successfully and then stayed invisible — indistinguishable from
// "the PAT does not save".
//
// The fake orchestrator below reproduces both scoping rules verbatim, so these
// tests fail for any client that pairs the member-scoped writer with the
// team-scoped reader.

import fs from "fs";
import path from "path";
import {
  createPersonalAccessToken,
  deletePersonalAccessToken,
  listPersonalAccessTokens,
} from "../lib/orchestratorApi";

const BASE = "http://orchestrator.test";
const SESSION = "pat_session";
const MEMBER_ID = 1;
const TEAM_ID = 42;

/** Mirrors the columns of the orchestrator's `access_tokens` table. */
type Row = {
  publicId: string;
  kind: string;
  memberId: number | null;
  teamId: number | null;
  name: string;
  secretSuffix: string;
  creationTime: number;
  revokedTime: number | null;
};

/** The subset of `Response` that `request()` in orchestratorApi.ts touches. */
type FakeResponse = {
  ok: boolean;
  status: number;
  statusText: string;
  text: () => Promise<string>;
  json: () => Promise<unknown>;
};

function jsonResponse(body: unknown): FakeResponse {
  return {
    ok: true,
    status: 200,
    statusText: "",
    text: async () => JSON.stringify(body),
    json: async () => body,
  };
}

/** A 200 with no body, which is what the revoke endpoint answers. */
function emptyResponse(): FakeResponse {
  return {
    ok: true,
    status: 200,
    statusText: "",
    text: async () => "",
    json: async () => {
      throw new SyntaxError("Unexpected end of JSON input");
    },
  };
}

/**
 * Minimal stand-in for the orchestrator's token routes. Only the scoping rules
 * matter here, and each is annotated with the Rust it mirrors.
 */
function fakeOrchestrator() {
  const rows: Row[] = [];
  let seq = 0;

  const fetchMock = jest.fn(async (url: string, init?: RequestInit) => {
    const path = url.slice(BASE.length);

    // management/tokens.rs::create_personal_access_token — member_id: Some,
    // team_id: None.
    if (path === "/v1/create_personal_access_token") {
      const { name } = JSON.parse(String(init?.body ?? "{}")) as {
        name: string;
      };
      seq += 1;
      const publicId = `tok_${seq}`;
      rows.push({
        publicId,
        kind: "pat",
        memberId: MEMBER_ID,
        teamId: null,
        name,
        secretSuffix: `sfx${seq}`,
        creationTime: seq,
        revokedTime: null,
      });
      return jsonResponse({
        accessToken: `pat_${publicId}|secret`,
        id: publicId,
        name,
        creationTime: seq,
      });
    }

    // management/tokens.rs::list_personal_access_tokens — SELECT_TOKENS_BY_MEMBER
    // plus a `kind == Pat` filter.
    if (path === "/v1/list_personal_access_tokens") {
      const tokens = rows
        .filter(
          (r) =>
            r.memberId === MEMBER_ID &&
            r.revokedTime === null &&
            r.kind === "pat",
        )
        .map((r) => ({
          id: r.publicId,
          name: r.name,
          creationTime: r.creationTime,
          keySuffix: r.secretSuffix,
        }));
      return jsonResponse({ tokens, cursor: null });
    }

    // management/tokens.rs::delete_personal_access_token — revoke by public_id.
    if (path === "/v1/delete_personal_access_token") {
      const { id } = JSON.parse(String(init?.body ?? "{}")) as { id: string };
      const row = rows.find((r) => r.publicId === id);
      if (row) row.revokedTime = 1;
      return emptyResponse();
    }

    // dashboard/access_tokens.rs::list_team_tokens — SELECT_TOKENS_BY_TEAM.
    if (path === `/api/dashboard/teams/${TEAM_ID}/access_tokens`) {
      return jsonResponse(
        rows
          .filter((r) => r.teamId === TEAM_ID && r.revokedTime === null)
          .map((r) => ({
            id: r.publicId,
            kind: r.kind,
            name: r.name,
            creationTime: r.creationTime,
            keySuffix: r.secretSuffix,
          })),
      );
    }

    throw new Error(`unexpected request to ${path}`);
  });

  global.fetch = fetchMock as unknown as typeof fetch;
  return { rows, fetchMock };
}

test("a token that was just created is listed back", async () => {
  fakeOrchestrator();

  const created = await createPersonalAccessToken(BASE, SESSION, "CI deploy");
  expect(created.accessToken).toContain("|");

  const tokens = await listPersonalAccessTokens(BASE, SESSION);
  expect(tokens.map((t) => t.name)).toEqual(["CI deploy"]);
  expect(tokens[0].id).toEqual(created.id);
});

test("every created token is listed, not just the newest", async () => {
  fakeOrchestrator();

  await createPersonalAccessToken(BASE, SESSION, "first");
  await createPersonalAccessToken(BASE, SESSION, "second");

  const tokens = await listPersonalAccessTokens(BASE, SESSION);
  expect(tokens.map((t) => t.name).sort()).toEqual(["first", "second"]);
});

test("a revoked token drops out of the list", async () => {
  fakeOrchestrator();

  const created = await createPersonalAccessToken(BASE, SESSION, "throwaway");
  await deletePersonalAccessToken(BASE, SESSION, created.id);

  await expect(listPersonalAccessTokens(BASE, SESSION)).resolves.toEqual([]);
});

// The root cause, pinned so nobody re-points the page at the team endpoint:
// the team-scoped reader structurally cannot see a personal access token.
test("the team-scoped endpoint cannot see a personal access token", async () => {
  const { fetchMock } = fakeOrchestrator();

  await createPersonalAccessToken(BASE, SESSION, "CI deploy");

  const res = await (fetchMock as unknown as typeof fetch)(
    `${BASE}/api/dashboard/teams/${TEAM_ID}/access_tokens`,
  );
  await expect(res.json()).resolves.toEqual([]);
});

test("the Access Tokens page reads tokens back from the member-scoped list", () => {
  const source = fs.readFileSync(
    path.join(
      __dirname,
      "..",
      "pages",
      "t",
      "[team]",
      "settings",
      "access-tokens.tsx",
    ),
    "utf8",
  );
  expect(source).toContain("listPersonalAccessTokens");
  // The team-scoped reader is what made created tokens invisible.
  expect(source).not.toContain("access_tokens");
});
