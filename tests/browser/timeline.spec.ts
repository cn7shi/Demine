import { test, expect } from "@playwright/test";
import { appendFileSync, readFileSync } from "node:fs";
import { resolve } from "node:path";

test("automatically discovers a session and opens exact evidence", async ({
  page,
}) => {
  await page.goto("/");
  await expect(
    page.getByRole("status").filter({ hasText: "本地观察中" }),
  ).toBeVisible();
  await expect(page.locator("#sessions .session-button")).toHaveCount(1);
  await expect(page.locator("#session-title")).toHaveText(
    "检查小屏设备上的布局偏移",
  );
  await expect(page.locator("#stat-failures")).toHaveText("1");
  await page
    .getByRole("button", { name: "执行命令 · 退出码 1，查看证据" })
    .click();
  await expect(page.locator("#evidence pre")).toContainText(
    "horizontal overflow",
  );
  await expect(page.locator(".evidence-location")).toContainText("第 4 行");
  await page.getByRole("button", { name: "需留意", exact: true }).click();
  await expect(page.locator("#timeline .event")).toHaveCount(2);
  await page.getByRole("button", { name: "全部", exact: true }).click();
  await page.screenshot({
    path: "test-results/demine-desktop.png",
    fullPage: true,
  });
});

test("follows appended records without reload and renders untrusted text safely", async ({
  page,
}) => {
  await page.goto("/");
  await expect(page.locator("#timeline .event")).not.toHaveCount(0);
  const control = JSON.parse(
    readFileSync(resolve("target/browser-control.json"), "utf8"),
  );
  const unique = `live-${Date.now()}`;
  const marker = `<img src=x onerror="window.__demineInjected=true"> ${unique}`;
  appendFileSync(
    control.log,
    JSON.stringify({
      timestamp: "2026-10-05T11:31:00Z",
      type: "event_msg",
      payload: {
        type: "item_completed",
        turn_id: "turn-next",
        item: { type: "AgentMessage", id: unique, content: marker },
      },
    }) + "\n",
  );
  await expect(
    page.locator(".event-preview").filter({ hasText: unique }),
  ).toBeVisible({ timeout: 10_000 });
  expect(
    await page.evaluate(
      () =>
        (window as unknown as { __demineInjected?: boolean }).__demineInjected,
    ),
  ).toBeUndefined();
  await expect(page.locator("#timeline img")).toHaveCount(0);
  await page.getByRole("searchbox").fill(unique);
  await expect(page.locator("#timeline .event")).toHaveCount(1);
});

test("updates a pending call with its eventual result instead of duplicating it", async ({
  page,
}) => {
  await page.goto("/");
  const control = JSON.parse(
    readFileSync(resolve("target/browser-control.json"), "utf8"),
  );
  const id = `pending-${Date.now()}`;
  appendFileSync(
    control.log,
    JSON.stringify({
      type: "response_item",
      payload: {
        type: "function_call",
        name: id,
        call_id: id,
        arguments: "check the new device",
      },
    }) + "\n",
  );
  const card = page.locator(".event").filter({ hasText: id });
  await expect(card).toHaveCount(1, { timeout: 10_000 });
  await expect(card.locator(".badge")).toContainText("进行中");
  appendFileSync(
    control.log,
    JSON.stringify({
      type: "response_item",
      payload: {
        type: "function_call_output",
        call_id: id,
        output: "new device observation received",
      },
    }) + "\n",
  );
  await expect(card.locator(".badge")).toContainText("已收到返回", {
    timeout: 10_000,
  });
  await expect(card).toHaveCount(1);
  await card.locator("summary").click();
  await expect(card.locator("pre").last()).toContainText(
    "new device observation received",
  );
});

test("remains usable on a narrow screen and reports a disconnected service", async ({
  page,
}) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto("/");
  await expect(page.locator("#session-title")).toHaveText(
    "检查小屏设备上的布局偏移",
  );
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= window.innerWidth,
    ),
  ).toBeTruthy();
  await page.screenshot({
    path: "test-results/demine-mobile.png",
    fullPage: true,
  });
  await page.route("**/api/**", (route) => route.abort());
  await expect(page.locator("#connection")).toHaveText("连接已断开", {
    timeout: 10_000,
  });
  await expect(page.locator("#timeline .event")).not.toHaveCount(0);
});
