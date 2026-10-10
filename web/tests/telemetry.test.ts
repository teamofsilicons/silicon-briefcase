import test from "node:test";
import assert from "node:assert/strict";
import { safeBrowserEvent } from "../lib/telemetry";
test("browser diagnostics exclude content, identifiers and credentials",()=>{
 const event=safeBrowserEvent({id:crypto.randomUUID(),type:"network",data:{url:"https://private.example/secret.txt?token=private",status:503,duration_ms:42,message:"private filename",percent:120}});
 assert.equal(event.stage,"failed");assert.equal(event.duration_ms,42);assert.equal(event.progress,null);assert.equal(event.environment_id,null);assert.equal(event.testing,false);
 assert.doesNotMatch(JSON.stringify(event),/private|secret|https|token/);
 assert.equal(safeBrowserEvent({type:"secret",data:{duration_ms:Infinity}}).operation,"web_unknown");
});
