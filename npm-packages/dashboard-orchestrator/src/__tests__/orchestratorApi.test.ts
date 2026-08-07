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
