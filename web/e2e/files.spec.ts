import { expect, test } from "@playwright/test";
import { accountsSession, BASE, runAccounts, sameOrigin, signIn, signInOnHostedPages, watchErrors } from "./support";

test("files: create, upload, range, preview, share, public link, version, bin and restore", async ({ browser }) => {
 const context = await accountsSession(browser,"carbon"); const page=await context.newPage(); const errors=watchErrors(page);
 await signIn(page,runAccounts().carbon.email,"/files");
 const folder=`Project ${Date.now()}`;
 await page.getByRole("button",{name:"New folder",exact:true}).click();
 await page.getByRole("textbox",{name:"Folder name"}).fill(folder);
 await page.getByRole("button",{name:"Create folder",exact:true}).click();
 await expect(page.getByRole("dialog")).toHaveCount(0);
 await page.getByRole("button",{name:new RegExp(`^${folder}`)}).click();
 await expect(page.getByRole("heading",{name:folder,exact:true})).toBeVisible();
 await page.locator('input[type="file"]').setInputFiles({name:"notes.txt",mimeType:"text/plain",buffer:Buffer.from("Briefcase storage proof: original bytes.")});
 await expect(page.getByText("Uploaded",{exact:true})).toBeVisible({timeout:30000});
 await page.getByRole("button",{name:/^notes.txt/}).click();
 const dialog=page.getByRole("dialog");
 await expect(dialog.getByText("Briefcase storage proof: original bytes.",{exact:true})).toBeVisible();
 const entries=await context.request.get(`${BASE}/api/v1/entries?path=${encodeURIComponent(folder)}`);expect(entries.status()).toBe(200);const entry=(await entries.json()).items[0];
 const range=await context.request.get(`${BASE}/api/v1/entries/${entry.id}/content`,{headers:{Range:"bytes=0-8"}});expect(range.status()).toBe(206);expect(await range.text()).toBe("Briefcase");expect(range.headers()["content-range"]).toContain("bytes 0-8/");
 const streamed=await context.request.get(`${BASE}/api/v1/entries/${entry.id}/content`);expect(streamed.headers()["content-security-policy"]).toContain("sandbox");
 await dialog.getByRole("tab",{name:"Sharing",exact:true}).click();
 await dialog.getByRole("textbox",{name:"Account id"}).fill(runAccounts().friend.id);
 await dialog.getByRole("button",{name:"Share access",exact:true}).click();
 await expect(dialog.getByText(runAccounts().friend.id,{exact:true})).toBeVisible();
 await dialog.getByRole("button",{name:"Enable link access",exact:true}).click();
 const address=await dialog.getByRole("textbox",{name:"Shareable address"}).inputValue();
 const anonymous=await browser.newContext();const publicPage=await anonymous.newPage();
 await publicPage.goto(`${BASE}${new URL(address).pathname}`);
 await expect(publicPage.getByRole("heading",{name:"notes.txt",exact:true})).toBeVisible();
 await expect(publicPage.getByText("Briefcase storage proof: original bytes.",{exact:true})).toBeVisible();
 await anonymous.close();
 const friend=await accountsSession(browser,"friend");const friendPage=await friend.newPage();await signIn(friendPage,runAccounts().friend.email,"/shared");
 await expect(friendPage.getByRole("button",{name:/^notes.txt/})).toBeVisible();await friend.close();
 await dialog.getByRole("tab",{name:"Versions",exact:true}).click();
 await expect(dialog.getByRole("button",{name:"Restore version"}).first()).toBeVisible();
 await dialog.getByRole("tab",{name:"Details",exact:true}).click();
 await dialog.getByRole("textbox",{name:"Name",exact:true}).fill("notes-renamed.txt");
 await dialog.getByRole("button",{name:"Rename",exact:true}).click();
 await expect(dialog.getByRole("heading",{name:"notes-renamed.txt",exact:true})).toBeVisible();
 await dialog.getByRole("button",{name:"Close dialog"}).click();
 const deleted=await context.request.delete(`${BASE}/api/v1/entries/${entry.id}`,{headers:sameOrigin({"Idempotency-Key":crypto.randomUUID()})});expect(deleted.ok()).toBe(true);
 await page.goto("/bin");await page.getByRole("button",{name:/^notes-renamed.txt/}).click();await page.getByRole("button",{name:"Restore from bin",exact:true}).click();
 await expect(page.getByRole("dialog")).toHaveCount(0);const restored=await context.request.get(`${BASE}/api/v1/entries/${entry.id}`);expect(restored.status()).toBe(200);expect((await restored.json()).deleted_at).toBeNull();
 expect(errors).toEqual([]);await context.close();
});

test("archives and office previews use real uploaded bytes",async({browser})=>{
 const context=await accountsSession(browser,"carbon");const page=await context.newPage();await signIn(page,runAccounts().carbon.email,"/files");
 const {zipSync,strToU8}=await import("fflate");
 const docx=zipSync({"word/document.xml":strToU8('<w:document xmlns:w="test"><w:body><w:p><w:r><w:t>Verified document text</w:t></w:r></w:p></w:body></w:document>')});
 const archive=zipSync({"hello.txt":strToU8("hello from archive")});
 for(const [name,data] of [["preview.docx",docx],["bundle.zip",archive]] as const){
  await page.locator('input[type="file"]').setInputFiles({name,mimeType:"application/octet-stream",buffer:Buffer.from(data)});
  await expect(page.getByRole("button",{name:new RegExp(`^${name}`)})).toBeVisible({timeout:30000});
  await page.getByRole("button",{name:new RegExp(`^${name}`)}).click();
  if(name.endsWith("docx"))await expect(page.frameLocator('iframe[title="Preview of preview.docx"]').getByText("Verified document text")).toBeVisible();
  else await expect(page.getByRole("cell",{name:"hello.txt",exact:true})).toBeVisible();
  await page.getByRole("button",{name:"Close dialog"}).click();
 }
 await context.close();
});

test("version restore, folder move, self-destruct keep and encoded public paths",async({browser})=>{
 const context=await accountsSession(browser,"carbon");const page=await context.newPage();await signIn(page,runAccounts().carbon.email,"/files");
 const name=`100% ready ${Date.now()}.txt`;
 const upload=async(text:string,minutes?:string)=>{const response=await context.request.post(`${BASE}/api/v1/uploads`,{headers:sameOrigin({"Idempotency-Key":crypto.randomUUID()}),multipart:{file:{name,mimeType:"text/plain",buffer:Buffer.from(text)},...(minutes?{self_destruct_minutes:minutes}:{})}});expect(response.ok()).toBe(true);return response.json();};
 const original=await upload("Version one", "10");
 await page.goto(`/files?entry=${original.id}`);await page.getByRole("tab",{name:"Details",exact:true}).click();await page.getByRole("button",{name:"Keep this file",exact:true}).click();
 await expect(page.getByRole("button",{name:"Keep this file",exact:true})).toHaveCount(0);
 await upload("Version two");
 await page.getByRole("tab",{name:"Versions",exact:true}).click();await expect(page.getByRole("button",{name:"Restore version"})).toHaveCount(2);await page.getByRole("button",{name:"Restore version"}).last().click();
 await expect.poll(async()=>await (await context.request.get(`${BASE}/api/v1/entries/${original.id}/content`)).text()).toBe("Version one");
 await page.getByRole("tab",{name:"Sharing",exact:true}).click();await page.getByRole("button",{name:"Enable link access",exact:true}).click();const address=await page.getByRole("textbox",{name:"Shareable address"}).inputValue();
 const guest=await browser.newContext();const publicPage=await guest.newPage();await publicPage.goto(`${BASE}${new URL(address).pathname}`);await expect(publicPage.getByRole("heading",{name,exact:true})).toBeVisible();await expect(publicPage.getByText("Version one",{exact:true})).toBeVisible();await guest.close();
 await page.getByRole("tab",{name:"Details",exact:true}).click();await page.getByRole("button",{name:"Move to folder",exact:true}).click();await page.getByRole("button",{name:"Move here",exact:true}).click();await expect(page.getByRole("button",{name:"Move here",exact:true})).toHaveCount(0);
 const entry=await (await context.request.get(`${BASE}/api/v1/entries/${original.id}`)).json();expect(entry.self_destruct_at).toBeNull();expect(entry.name).toBe(name);await context.close();
});


test("private permanent links sign in and retain canonical UUID and encoded paths",async({browser})=>{
 const owner=await accountsSession(browser,"carbon");const ownerPage=await owner.newPage();await signIn(ownerPage,runAccounts().carbon.email,"/files");
 const name=`Private 100% ${Date.now()}.txt`;const text="Shared through a private permanent address.";
 const upload=await owner.request.post(`${BASE}/api/v1/uploads`,{headers:sameOrigin({"Idempotency-Key":crypto.randomUUID()}),multipart:{file:{name,mimeType:"text/plain",buffer:Buffer.from(text)}}});expect(upload.status()).toBe(201);const entry=await upload.json();
 expect(entry.org_id).toMatch(/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/);
 const share=await owner.request.post(`${BASE}/api/v1/entries/${entry.id}/invitations`,{headers:sameOrigin({"Idempotency-Key":crypto.randomUUID()}),data:{principal:{type:"carbon",id:runAccounts().friend.id},access:["read"],inherit:true}});expect(share.status()).toBe(201);
 const friend=await accountsSession(browser,"friend");const page=await friend.newPage();const path=entry.path.split("/").map(encodeURIComponent).join("/");
 await page.goto(`${BASE}/org/${entry.org_id}/${path}`);await signInOnHostedPages(page,runAccounts().friend.email);
 await expect(page).toHaveURL(new RegExp(`entry=${entry.id}`));await expect(page.getByRole("dialog").getByText(text,{exact:true})).toBeVisible();
 await page.goto(`${BASE}/${entry.org_id}/${path}`);await expect(page).toHaveURL(new RegExp(`entry=${entry.id}`));await expect(page.getByRole("dialog").getByText(text,{exact:true})).toBeVisible();
 await friend.close();await owner.close();
});
