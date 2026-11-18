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
    const uploaded = await sender.request.post(`${BASE}/api/v1/uploads`, {
      headers: sameOrigin({ "Idempotency-Key": randomUUID() }),
      multipart: { file: { name: `review-${randomUUID()}.txt`, mimeType: "text/plain", buffer: Buffer.from("Approved file contents") } },
    });
    expect(uploaded.status()).toBe(201);
    const firstFile = await uploaded.json();
    const first = await sender.request.post(`${BASE}/api/v1/entries/${firstFile.id}/invitations`, {
      headers: sameOrigin({ "Idempotency-Key": randomUUID() }),
      data: { principal: { type: "carbon", id: accounts.friend.id }, access: ["read"], inherit: false },
    });
    expect(first.status()).toBe(202);
    const second = await shareNewFolder();
    expect(second.status()).toBe(201);
    const secondFolder = await second.json();
    const read = (id: string) => context.request.get(`${BASE}/api/v1/entries/${id}`);
    const download = () => context.request.get(`${BASE}/api/v1/entries/${firstFile.id}/download`);
    expect((await download()).status()).toBe(404);
    await page.getByRole("link", { name: /Review incoming shares/ }).click();
    const pending = page.getByRole("region", { name: /Pending shares/ });
    const firstRow = pending.getByRole("listitem").filter({ hasText: firstFile.name });
    await firstRow.getByRole("button", { name: "Accept File" }).click();
    await expect(firstRow).toHaveCount(0);
    expect(await (await download()).text()).toBe("Approved file contents");
    expect((await read(secondFolder.id)).status()).toBe(404);
    const third = await (await shareNewFolder()).json();
    await pending.getByRole("listitem").filter({ hasText: secondFolder.name }).getByRole("button", { name: "Allow Sender" }).click();
    await expect(pending.getByText("No shares waiting for review")).toBeVisible();
    expect((await read(third.id)).status()).toBe(200);
    const direct = await (await shareNewFolder()).json();
    expect((await read(direct.id)).status()).toBe(200);
    const senders = page.getByRole("region", { name: "Allowed senders" });
    await senders.getByRole("button", { name: "Remove" }).click();
    await expect(senders.getByRole("button", { name: "Remove" })).toHaveCount(0);
    const final = await (await shareNewFolder()).json();
    expect((await read(final.id)).status()).toBe(404);
    await senders.getByRole("textbox", { name: "Sender account ID" }).fill(accounts.carbon.id);
    await senders.getByRole("button", { name: "Add Sender" }).click();
    await expect(senders.getByRole("button", { name: "Remove" })).toBeVisible();
    expect((await read(final.id)).status()).toBe(200);
    await senders.getByRole("button", { name: "Remove" }).click();
    await expect(senders.getByRole("button", { name: "Remove" })).toHaveCount(0);

    const connected = await context.request.put(`${BASE}/api/v1/contacts/connection`, {
      headers: sameOrigin(), data: { enabled: true },
    });
    expect(connected.ok()).toBeTruthy();
    const contact = await context.request.post(`${BASE}/api/v1/contacts`, {
      headers: sameOrigin(), data: { id: accounts.carbon.id, name: "Incoming share sender", trust: 1 },
    });
    expect(contact.ok()).toBeTruthy();
    const disconnected = await context.request.put(`${BASE}/api/v1/contacts/connection`, {
      headers: sameOrigin(), data: { enabled: false },
    });
    expect(disconnected.ok()).toBeTruthy();
    const signedOut = await context.request.post(`${BASE}/auth/sign-out`, { headers: sameOrigin() });
    expect(signedOut.ok()).toBeTruthy();
    expect((await shareNewFolder()).status()).toBe(201);

    await signIn(page, accounts.friend.email, "/settings");
    await expect(allow).not.toBeChecked();
    await allow.click();
    await expect(allow).toBeChecked();
    await page.reload();
    await expect(allow).toBeChecked();
  } finally {
    await Promise.all([context.close(), sender.close()]);
  }
});
