import { mkdirSync } from "node:fs";
import { expect, test } from "@playwright/test";
import { accountsSession, appConfig, runAccounts, signIn } from "./support";
for (const theme of ["light", "dark"] as const) for (const mobile of [false, true]) {
 test(`Briefcase screens ${theme} ${mobile ? "phone" : "desktop"}`, async ({browser}) => {
  mkdirSync("screens",{recursive:true});
  const context=await accountsSession(browser,"carbon",{colorScheme:theme,reducedMotion:"reduce",viewport:mobile?{width:390,height:844}:{width:1440,height:900}});
  const page=await context.newPage();await signIn(page,runAccounts().carbon.email,"/files");
  await page.locator('input[type="file"]').setInputFiles({name:"Welcome to Briefcase.txt",mimeType:"text/plain",buffer:Buffer.from("Your files have a home. Share precisely, keep their history, and work with your Silicons.")});
  await expect(page.getByText("Uploaded",{exact:true})).toBeVisible();
  for(const item of appConfig.nav){
   await page.goto(item.href);await expect(page.getByRole("heading",{level:1})).toBeVisible();await page.waitForLoadState("networkidle");
   await page.screenshot({path:`screens/${item.href.slice(1)}-${theme}-${mobile?"phone":"desktop"}.png`,fullPage:true});
  }
  await page.goto("/files");await page.getByRole("button",{name:/^Welcome to Briefcase/}).click();
  await expect(page.getByRole("dialog")).toBeVisible();
  await page.screenshot({path:`screens/preview-${theme}-${mobile?"phone":"desktop"}.png`,fullPage:true});
  await page.getByRole("tab",{name:"Sharing",exact:true}).click();
  await page.screenshot({path:`screens/sharing-${theme}-${mobile?"phone":"desktop"}.png`,fullPage:true});
  expect(await page.evaluate(()=>document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
  await context.close();
 });
}
