import { test, expect } from "@playwright/test";
import { seal } from "../lib/server/seal";
const secret = "briefcase-organizations-local-ui-test-secret-0000000000000000";
const fixture = "http://127.0.0.1:4191";
test.beforeEach(async ({ context, request }) => {
  await request.post(fixture + "/__reset");
  await context.addCookies([{ name: "briefcase_session", url: "http://127.0.0.1:4190", httpOnly: true, sameSite: "Lax", value: seal({ v: 1, at: "local-ui-access", rt: "local-ui-refresh", ae: Date.now() + 3_600_000, re: Date.now() + 7_200_000, scope: "email", acct: { uuid: "10000000-0000-4000-8000-000000000001", id: "c:ada.example", kind: "carbon", display_name: "Ada Lovelace", pfp_url: null, custodian: null } }, "session", secret) }]);
});

test("manage teams, team admins and organization admins within Briefcase", async ({ page, request }) => {
  await page.goto("/organizations/example");
  await expect(page.getByRole("heading", { name: "Example Studio" })).toBeVisible();
  await page.getByRole("button", { name: "Make admin", exact: true }).click();
  await expect(page.getByRole("button", { name: "Remove admin", exact: true })).toBeVisible();
  await page.screenshot({ path: "test-results/organizations-admin.png", fullPage: true });
  await page.getByRole("button", { name: "Teams", exact: true }).click();
  await page.getByRole("button", { name: "Create a team", exact: true }).click();
  await page.getByLabel("Team name", { exact: true }).fill("Engineering");
  await page.getByLabel("Description", { exact: true }).fill("Build together");
  await page.getByRole("button", { name: "Create team", exact: true }).click();
  await expect(page.getByRole("heading", { name: "Engineering", exact: true })).toBeVisible();
  await page.getByRole("button", { name: "Open team" }).first().click();
  await page.getByRole("combobox", { name: "Add an organization member" }).click();
  await page.getByRole("option", { name: /Grace Hopper/ }).click();
  await page.getByRole("button", { name: "Add to team", exact: true }).click();
  await expect(page.getByRole("dialog").getByText("Grace Hopper", { exact: true })).toBeVisible();
  await page.getByRole("button", { name: "Make team admin", exact: true }).click();
  await expect(page.getByRole("button", { name: "Remove team admin", exact: true })).toHaveCount(2);
  const requests = await (await request.get(fixture + "/__requests")).json();
  expect(requests.map((r: { body: { action: string } }) => r.body.action)).toEqual(["admin_set", "team_create", "team_member_add", "team_admin_set"]);
  expect(requests.every((r: { key: string }) => r.key?.length > 10)).toBeTruthy();
  await page.setViewportSize({ width: 390, height: 844 });
  await page.screenshot({ path: "test-results/team-management-mobile.png" });
});

test("share a folder with a particular team and the whole organization", async ({ page, request }) => {
  await page.goto("/files?entry=10000000-0000-4000-8000-000000000004");
  await page.getByRole("tab", { name: "Sharing", exact: true }).click();
  await page.getByRole("button", { name: "Team", exact: true }).click();
  await page.getByRole("combobox", { name: "Team", exact: true }).click();
  await page.getByRole("option", { name: "Design · example", exact: true }).click();
  await page.getByRole("button", { name: "Share with team", exact: true }).click();
  await expect(page.getByRole("button", { name: "Remove", exact: true })).toHaveCount(1);
  await expect(page.getByRole("dialog").getByText("Design", { exact: true })).toBeVisible();
  await page.getByRole("button", { name: "Organization", exact: true }).click();
  await page.getByRole("combobox", { name: "Organization", exact: true }).click();
  await page.getByRole("option", { name: "Example Studio", exact: true }).click();
  await page.getByRole("button", { name: "Share with organization", exact: true }).click();
  await expect(page.getByRole("button", { name: "Remove", exact: true })).toHaveCount(2);
  await expect(page.getByRole("dialog").locator("strong").filter({ hasText: "Example Studio" })).toBeVisible();
  const requests = await (await request.get(fixture + "/__requests")).json();
  expect(requests.map((r: { body: { principal: { type: string } } }) => r.body.principal.type)).toEqual(["team", "organization"]);
  await page.screenshot({ path: "test-results/team-sharing.png", fullPage: true });
  await page.setViewportSize({ width: 390, height: 844 });
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
  await page.getByRole("button", { name: "Close dialog", exact: true }).click();
  await page.setViewportSize({ width: 320, height: 700 });
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
  expect(requests[0].body.inherit).toBe(true);
  expect(requests[0].body.principal.id).toBe("example/10000000-0000-4000-8000-000000000003");
});

test("personal accounts and service outages have actionable states", async ({ page, request }) => {
  await request.post(fixture + "/__mode", { data: { mode: "personal" } });
  await page.goto("/organizations");
  await expect(page.getByRole("heading", { name: "Bring your work account" })).toBeVisible();
  await expect(page.getByRole("link", { name: "Choose a work account" })).toHaveAttribute("href", /prompt=select_account/);
  await request.post(fixture + "/__mode", { data: { mode: "outage" } });
  await page.reload();
  await expect(page.getByRole("button", { name: "Try again", exact: true })).toBeVisible();
});

test("ordinary members cannot see admin controls; mobile layout stays within viewport", async ({ page, request }) => {
  await request.post(fixture + "/__mode", { data: { mode: "member" } });
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto("/organizations/example");
  await expect(page.getByRole("heading", { name: "People & access" })).toBeVisible();
  await expect(page.getByRole("button", { name: "Make admin", exact: true })).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Invite a member" })).toHaveCount(0);
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
  await page.screenshot({ path: "test-results/organizations-mobile.png", fullPage: true });
  await page.setViewportSize({ width: 1440, height: 1000 });
  await page.screenshot({ path: "test-results/organizations-desktop.png", fullPage: true });
});
