// Response handling in the shared `request()` helper.
//
// Several orchestrator endpoints answer with a status but no body — delete
// returns 200 and retry returns 202, both empty. Parsing those as JSON throws,
// and because the dashboard awaits the call before revalidating its SWR cache,
// the throw is what stops a removed domain from disappearing until a manual
// reload.

import {
  deleteCustomDomain,
  listCustomDomains,
  retryCustomDomain,
} from "../lib/orchestratorApi";

function mockFetchOnce(init: { status: number; body?: string }) {
  const fetchMock = jest.fn().mockResolvedValue({
    ok: init.status >= 200 && init.status < 300,
    status: init.status,
    statusText: "",
    text: async () => init.body ?? "",
    json: async () => {
      const text = init.body ?? "";
      if (!text) throw new SyntaxError("Unexpected end of JSON input");
      return JSON.parse(text);
    },
  });
  global.fetch = fetchMock as unknown as typeof fetch;
  return fetchMock;
}

test("delete resolves on an empty 200 rather than choking on the body", async () => {
  mockFetchOnce({ status: 200 });
  await expect(
    deleteCustomDomain("http://orchestrator.test", "pat", 7, "a.example.com"),
  ).resolves.toBeUndefined();
});

test("retry resolves on an empty 202", async () => {
  mockFetchOnce({ status: 202 });
  await expect(
    retryCustomDomain("http://orchestrator.test", "pat", 7, "a.example.com"),
  ).resolves.toBeUndefined();
});

test("a 204 with no body still resolves", async () => {
  mockFetchOnce({ status: 204 });
  await expect(
    deleteCustomDomain("http://orchestrator.test", "pat", 7, "a.example.com"),
  ).resolves.toBeUndefined();
});

// Adding or removing a custom domain rewrites Traefik's dynamic config, and
// Traefik reloads when it lands. Dashboard traffic returns through that same
// Traefik, so the reload can drop the list refetch the mutation triggers —
// which surfaced as "Could not load custom domains: NetworkError when
// attempting to fetch resource" for an operation that had already succeeded.
test("a read dropped at the transport layer is retried", async () => {
  let calls = 0;
  global.fetch = jest.fn(async () => {
    calls += 1;
    if (calls === 1) {
      throw new TypeError("NetworkError when attempting to fetch resource.");
    }
    return {
      ok: true,
      status: 200,
      statusText: "",
      text: async () =>
        JSON.stringify({
          domains: [],
          targetHost: "convex.example.com",
          routingEnabled: true,
        }),
      json: async () => ({}),
    };
  }) as unknown as typeof fetch;

  await expect(
    listCustomDomains("http://orchestrator.test", "pat", 7),
  ).resolves.toMatchObject({ targetHost: "convex.example.com" });
  expect(calls).toBe(2);
});

test("a dropped mutation is never replayed", async () => {
  // The write may already have been applied; retrying could double-apply it.
  let calls = 0;
  global.fetch = jest.fn(async () => {
    calls += 1;
    throw new TypeError("NetworkError when attempting to fetch resource.");
  }) as unknown as typeof fetch;

  const started = Date.now();
  await expect(
    deleteCustomDomain("http://orchestrator.test", "pat", 7, "a.example.com"),
  ).rejects.toThrow(/NetworkError/);
  expect(calls).toBe(1);
  // And fails straight away: a request that is never retried must not pay the
  // inter-attempt delay before surfacing the error.
  expect(Date.now() - started).toBeLessThan(200);
});

test("an HTTP error status is surfaced, not retried away", async () => {
  let calls = 0;
  global.fetch = jest.fn(async () => {
    calls += 1;
    return {
      ok: false,
      status: 500,
      statusText: "Internal Server Error",
      text: async () => "",
      json: async () => ({ message: "boom" }),
    };
  }) as unknown as typeof fetch;

  await expect(
    listCustomDomains("http://orchestrator.test", "pat", 7),
  ).rejects.toMatchObject({ status: 500 });
  expect(calls).toBe(1);
});

test("responses that do carry JSON are still parsed", async () => {
  mockFetchOnce({
    status: 200,
    body: JSON.stringify({
      domains: [],
      targetHost: "convex.example.com",
      routingEnabled: true,
    }),
  });
  await expect(
    listCustomDomains("http://orchestrator.test", "pat", 7),
  ).resolves.toMatchObject({ targetHost: "convex.example.com" });
});
