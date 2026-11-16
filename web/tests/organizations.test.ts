import { test } from "node:test";
import assert from "node:assert/strict";
import { canManageAdmin, canManageTeam, groupShareBody, type Capabilities, type Member, type Team } from "../lib/briefcase/organizations";
const caps: Capabilities = { owner: false, admin: true, manage_admins: false, team_admins: [], team_structure: false, team_members: false, team_admins_manage: false, team_settings: false };
const member: Member = { uuid: "member", id: "c:ada.example", display_name: "Ada", kind: "carbon", owner: false, admin: false, manage_admins: false };
test("admin appointment hints respect protected owners and Manage admins", () => {
  assert.equal(canManageAdmin(caps, member), false);
  assert.equal(canManageAdmin({ ...caps, manage_admins: true }, member), true);
  assert.equal(canManageAdmin({ ...caps, manage_admins: true }, { ...member, manage_admins: true }), false);
  assert.equal(canManageAdmin({ ...caps, owner: true }, { ...member, owner: true }), false);
});
test("a team admin's controls are scoped to their team", () => {
  const team = { id: "design" } as Team;
  assert.equal(canManageTeam({ ...caps, team_admins: ["design"] }, team, "members"), true);
  assert.equal(canManageTeam({ ...caps, team_admins: ["engineering"] }, team, "members"), false);
});
test("expiring group shares are read-only, valid for at most thirty days", () => {
  const target = { type: "team" as const, id: "example/design", name: "Design", org_id: "example" };
  assert.deepEqual(groupShareBody(target, ["read", "update"], true, "60"), { principal: { type: "team", id: "example/design" }, access: ["read"], inherit: true, expires_in_minutes: 60 });
  assert.deepEqual(groupShareBody(target, ["read", "write", "update"], false, "").access, ["read", "update"]);
  for (const invalid of ["0", "-1", "0.5", "NaN", "43201"]) assert.throws(() => groupShareBody(target, ["read"], true, invalid));
});
