import { spawn, execFile } from "node:child_process";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { resolve, join } from "node:path";
import { promisify } from "node:util";
import { expect, test } from "@playwright/test";
import { accountsSession } from "./support";
const run=promisify(execFile);
test("native CLI device sign-in, real file roundtrip and revocation",async({browser})=>{
 const home=await mkdtemp(join(tmpdir(),"briefcase-cli-live-"));
 const binary=process.env.BRIEFCASE_E2E_CLI || resolve("../target/mig-client/debug/briefcase");
 const env: NodeJS.ProcessEnv={...process.env,SILICON_HOME:home,BRIEFCASE_TELEMETRY:"off",BRIEFCASE_URL:"http://127.0.0.1:4101/api/v1/",ACCOUNTS_URL:"http://localhost:9590"};
 delete env.BRIEFCASE_TOKEN;delete env.BRIEFCASE_PROFILE;
 const child=spawn(binary,["login","--json","--label","Briefcase browser integration test"],{env,stdio:["ignore","pipe","pipe"]});
 let stdout="",stderr="";child.stdout.on("data",chunk=>stdout+=chunk);child.stderr.on("data",chunk=>stderr+=chunk);
 const completion=new Promise<number|null>((resolve,reject)=>{child.once("error",reject);child.once("close",resolve);});
 const context=await accountsSession(browser,"carbon");
 try {
  await expect.poll(()=>stdout.includes('"event":"device_code"')).toBe(true);
  const event=stdout.split("\n").map(line=>{try{return JSON.parse(line)}catch{return null}}).find(value=>value?.event==="device_code");
  const page=await context.newPage();await page.goto(event.verification_uri_complete);
  await page.getByRole("button",{name:"Approve sign-in",exact:true}).click();
  expect(await completion,stderr).toBe(0);
  const cli=async(args:string[])=>await run(binary,args,{env,timeout:30000});
  const status=JSON.parse((await cli(["login","status","--json"])).stdout);expect(status.authenticated).toBe(true);
  const directory=`cli-${Date.now()}`;await cli(["mkdir",directory]);
  const source=join(home,"proof.txt");await writeFile(source,"Live native Briefcase bytes");await cli(["put",source,directory]);
  expect((await cli(["cat",`${directory}/proof.txt`])).stdout).toBe("Live native Briefcase bytes");
  await cli(["rm",directory]);await cli(["logout"]);
  expect(JSON.parse((await cli(["login","status","--json"])).stdout).authenticated).toBe(false);
 }finally{child.kill();await context.close();await rm(home,{recursive:true,force:true});}
});
