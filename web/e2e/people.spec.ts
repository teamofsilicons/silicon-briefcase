import { test, expect } from "@playwright/test";
import { seal } from "../lib/server/seal";
const secret = "briefcase-organizations-local-ui-test-secret-0000000000000000";
const fixture = "http://127.0.0.1:4191";
test.beforeEach(async ({ context, request }) => {
  await request.post(fixture + "/__reset");
  await context.addCookies([{ name: "briefcase_session", url: "http://127.0.0.1:4190", httpOnly: true, sameSite: "Lax", value: seal({ v: 1, at: "local-ui-access", rt: "local-ui-refresh", ae: Date.now() + 3_600_000, re: Date.now() + 7_200_000, scope: "email", acct: { uuid: "10000000-0000-4000-8000-000000000001", id: "c:ada.example", kind: "carbon", display_name: "Ada Lovelace", pfp_url: null, custodian: null } }, "session", secret) }]);
});

test("ranked fuzzy suggestions, keyboard selection, saved photo and UUID invitation", async ({ page, request }) => {
  await page.goto("/files?entry=10000000-0000-4000-8000-000000000004");
  await page.getByRole("tab", { name: "Sharing", exact: true }).click();
  const input = page.getByRole("combobox", { name: "Account ID or name" });
  await input.focus();
  await expect(page.getByRole("option").first()).toContainText("My Grace");
  await input.fill("garce");
  await expect(page.getByRole("option")).toHaveCount(1);
  await input.press("ArrowDown"); await input.press("Enter");
  const selected = page.getByLabel("Selected recipient");
  await expect(selected).toContainText("My Grace");
  await expect(selected.locator("img")).toHaveAttribute("src", /contacts\/photos\//);
  await expect(selected.locator("img")).toHaveJSProperty("naturalWidth", 1);
  await page.getByRole("button", { name: "Share access", exact: true }).click();
  await expect(page.getByRole("button", { name: "Remove", exact: true })).toHaveCount(1);
  const requests = await (await request.get(fixture + "/__requests")).json();
  expect(requests.at(-1).body.principal).toEqual({ type: "carbon", id: "10000000-0000-4000-8000-000000000002" });
  await page.screenshot({ path: "test-results/contacts-sharing-desktop.png", fullPage: true });
  await page.setViewportSize({ width: 390, height: 844 });
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  await page.screenshot({ path: "test-results/contacts-sharing-mobile.png", fullPage: true });
  // Editing text clears the former selection immediately; a failed lookup cannot share it.
  await input.fill("si:nobody");
  await expect(page.getByRole("button", { name: "Share access", exact: true })).toBeDisabled();
});

test("exact Silicon preview and inline contact saving do not submit an invitation", async ({ page, request }) => {
  await page.goto("/files?entry=10000000-0000-4000-8000-000000000004");
  await page.getByRole("tab", { name: "Sharing", exact: true }).click();
  await page.getByRole("combobox", { name: "Account ID or name" }).fill("si:scout");
  await expect(page.getByLabel("Selected recipient")).toContainText("Scout");
  await page.getByRole("button", { name: "Add contact", exact: true }).click();
  await page.getByLabel("Saved name", { exact: true }).fill("Research partner");
  await page.getByLabel("Trust level", { exact: true }).fill("255");
  await page.getByRole("button", { name: "Save contact", exact: true }).click();
  await expect(page.getByLabel("Selected recipient")).toContainText("Research partner");
  const requests = await (await request.get(fixture + "/__requests")).json();
  expect(requests).toHaveLength(1); expect(requests[0].route).toBe("/contacts"); expect(requests[0].body.trust).toBe(255);
});

test("Contacts add/edit, consent, saved details and accessible mobile layout", async ({ page }) => {
  await page.goto("/contacts");
  await expect(page.getByText("My Grace", { exact: true })).toBeVisible();
  await page.getByRole("button", { name: "Add contact", exact: true }).click();
  await page.getByRole("combobox", { name: "Account ID or name" }).fill("si:scout");
  await expect(page.getByLabel("Selected recipient")).toBeVisible();
  await page.getByLabel("Saved name", { exact: true }).fill("Scout at work");
  await page.getByLabel("Trust level", { exact: true }).fill("20");
  await page.getByRole("button", { name: "Save contact", exact: true }).click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await expect(page.locator("main li").first()).toContainText("Scout at work");
  await page.setViewportSize({ width: 390, height: 844 });
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  await page.addScriptTag({ path: "node_modules/axe-core/axe.min.js" });
  const violations = await page.evaluate(async () => (await (window as unknown as { axe: { run: (context: Document, options: object) => Promise<{ violations: unknown[] }> } }).axe.run(document, { runOnly: { type: "tag", values: ["wcag2a", "wcag2aa", "wcag22aa"] } })).violations);
  expect(violations).toEqual([]);
  await page.screenshot({ path: "test-results/contacts-mobile.png", fullPage: true });
  await page.getByRole("button", { name: "Disconnect Contacts" }).click();
  await expect(page.getByRole("button", { name: "Connect Silicon Contacts" })).toBeVisible();
  await expect(page.getByText("Scout at work", { exact: true })).toHaveCount(0);
  await page.getByRole("button", { name: "Connect Silicon Contacts" }).click();
  await expect(page.getByText("Scout at work", { exact: true })).toBeVisible();
});

test("shared filters reach the server and search remains in shared files", async ({ page, request }) => {
  await page.goto("/shared?contacts=saved");
  await expect(page.getByText("My Grace", { exact: true })).toBeVisible();
  await page.getByLabel("Minimum trust", { exact: true }).fill("8");
  await page.getByRole("combobox", { name: "Shared by", exact: true }).click();
  await page.getByRole("option", { name: /My Grace/ }).click();
  await page.getByLabel("Search files", { exact: true }).fill("missing");
  await page.getByLabel("Search files", { exact: true }).press("Enter");
  await expect(page.getByRole("heading", { name: "No matching files" })).toBeVisible();
  const requests = await (await request.get(fixture + "/__requests")).json();
  expect(requests.at(-1).query).toMatchObject({ q: "missing", contacts: "saved", min_trust: "8", from: "10000000-0000-4000-8000-000000000002" });
});

test("Contacts outage keeps invitation history and shows a retry", async ({ page, request }) => {
  await request.post(fixture + "/__mode", { data: { mode: "contacts-outage" } });
  await page.goto("/files?entry=10000000-0000-4000-8000-000000000004");
  await page.getByRole("tab", { name: "Sharing", exact: true }).click();
  await page.getByRole("combobox", { name: "Account ID or name" }).focus();
  await expect(page.getByRole("option").first()).toContainText("Grace Hopper");
  await expect(page.getByRole("button", { name: "Retry contacts" })).toBeVisible();
});
