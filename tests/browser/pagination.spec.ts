import { test, expect, type Page, type Route } from "@playwright/test";

const event = (id: number, revision = id, status = "started") => ({
  id,
  revision,
  status,
  kind: "tool",
  title: `操作 ${id}`,
  input: "检查设备",
  output: status === "returned" ? "已收到最新结果" : "",
  timestamp: "2026-10-05T10:00:00Z",
  turn_id: null,
  call_id: `call-${id}`,
  generation: 0,
});
const respond = (
  route: Route,
  events: ReturnType<typeof event>[],
  cursor: number,
  more = false,
) =>
  route.fulfill({ json: { events, cursor, has_more: more, hidden_ids: [] } });

async function openTimeline(page: Page) {
  await page.goto("/");
  await expect(page.locator('.event[data-id="200"]')).toBeVisible();
  await expect(page.locator("#refresh")).toBeEnabled();
}

test("late updates to older events do not move the history pagination boundary", async ({
  page,
}) => {
  let sendUpdate = false;
  let requestedBefore: string | null = null;
  await page.route("**/api/sessions/*/events*", async (route) => {
    const query = new URL(route.request().url()).searchParams;
    if (query.has("before")) {
      requestedBefore = query.get("before");
      await respond(
        route,
        Number(requestedBefore) > 100 ? [event(100)] : [],
        201,
      );
    } else if (query.has("after")) {
      await respond(
        route,
        sendUpdate && Number(query.get("after")) < 201
          ? [event(50, 201, "returned")]
          : [],
        sendUpdate ? 201 : 200,
      );
    } else {
      await respond(route, [event(200)], 200, true);
    }
  });
  await openTimeline(page);
  sendUpdate = true;
  await page.locator("#refresh").click();
  await expect(page.locator('.event[data-id="50"] .badge')).toHaveText(
    "已收到返回",
  );
  await page.locator("#older").click();
  await expect.poll(() => requestedBefore).toBe("200");
  await expect(page.locator('.event[data-id="100"]')).toBeVisible();
});

test("a delayed history response cannot overwrite a newer tool result", async ({
  page,
}) => {
  let sendUpdate = false;
  let olderRequested = false;
  let releaseOlder = () => {};
  const gate = new Promise<void>((resolve) => {
    releaseOlder = resolve;
  });
  await page.route("**/api/sessions/*/events*", async (route) => {
    const query = new URL(route.request().url()).searchParams;
    if (query.has("before")) {
      olderRequested = true;
      await gate;
      await respond(route, [event(50), event(100)], 200);
    } else if (query.has("after")) {
      await respond(
        route,
        sendUpdate && Number(query.get("after")) < 201
          ? [event(50, 201, "returned")]
          : [],
        sendUpdate ? 201 : 200,
      );
    } else {
      await respond(route, [event(200)], 200, true);
    }
  });
  await openTimeline(page);
  await page.locator("#older").click();
  await expect.poll(() => olderRequested).toBeTruthy();
  sendUpdate = true;
  await page.locator("#refresh").click();
  await expect(page.locator('.event[data-id="50"] .badge')).toHaveText(
    "已收到返回",
  );
  releaseOlder();
  await expect(page.locator('.event[data-id="100"]')).toBeVisible();
  await expect(page.locator('.event[data-id="50"] .badge')).toHaveText(
    "已收到返回",
  );
});
