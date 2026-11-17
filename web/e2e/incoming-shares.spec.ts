import { randomUUID } from "node:crypto";
import { expect, test } from "@playwright/test";
import { accountsSession, BASE, runAccounts, sameOrigin, signIn } from "./support";

test("incoming shares are open by default and the recipient can restrict and reopen them", async ({ browser }) => {
  const context = await accountsSession(browser, "friend");
  const sender = await accountsSession(browser, "carbon");
  try {
    const accounts = runAccounts();
    const page = await context.newPage();
    await signIn(page, accounts.friend.email, "/settings");
    await signIn(await sender.newPage(), accounts.carbon.email, "/files");
    const allow = page.getByRole("switch", { name: "Allow shares from anyone" });
    await expect(allow).toBeChecked();
    const shareNewFolder = () => sender.request.post(`${BASE}/api/v1/entries`, {
      headers: sameOrigin({ "Idempotency-Key": randomUUID() }),
      data: {
        name: `incoming-${randomUUID()}`,
        invitees: [{ principal: { type: "carbon", id: accounts.friend.id }, access: ["read"], inherit: true }],
      },
    });
    expect((await shareNewFolder()).status()).toBe(201);
    await allow.click();
    await expect(allow).not.toBeChecked();
    await page.reload();
    await expect(allow).not.toBeChecked();
    const refused = await shareNewFolder();
    expect(refused.status()).toBe(403);
    expect((await refused.json()).error.code).toBe("incoming_shares_restricted");

    const connected = await context.request.put(`${BASE}/api/v1/contacts/connection`, {
      headers: sameOrigin(), data: { enabled: true },
    });
    expect(connected.ok()).toBeTruthy();
    const contact = await context.request.post(`${BASE}/api/v1/contacts`, {
      headers: sameOrigin(), data: { id: accounts.carbon.id, name: "Incoming share sender", trust: 1 },
    });
    expect(contact.ok()).toBeTruthy();
    expect((await shareNewFolder()).status()).toBe(201);
    await allow.click();
    await expect(allow).toBeChecked();
    await page.reload();
    await expect(allow).toBeChecked();
  } finally {
    await Promise.all([context.close(), sender.close()]);
  }
});
