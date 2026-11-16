// Loopback-only UI fixtures. No tokens or external services are used.
import { createServer } from "node:http";
import { randomUUID } from "node:crypto";
const owner = { uuid: "10000000-0000-4000-8000-000000000001", id: "c:ada.example", kind: "carbon", display_name: "Ada Lovelace", owner: true, admin: false, manage_admins: false };
const colleague = { uuid: "10000000-0000-4000-8000-000000000002", id: "c:grace.example", kind: "carbon", display_name: "Grace Hopper", owner: false, admin: false, manage_admins: false };
const policy = { mode: "everywhere", apps: [], extra_apps: [], excluded_apps: [] };
const caps = { owner: true, admin: true, manage_admins: true, team_admins: [], team_structure: true, team_members: true, team_admins_manage: true, team_settings: true };
const teamId = "10000000-0000-4000-8000-000000000003";
const file = { id: "10000000-0000-4000-8000-000000000004", org_id: owner.uuid, name: "Product-roadmap-and-organization-team-sharing-2026-final", path: "Product-roadmap-and-organization-team-sharing-2026-final", parent_id: "10000000-0000-4000-8000-000000000005", type: "folder", size: null, content_type: null, visibility: "private", effective_access: ["read", "write", "update", "delete"], owner: { type: "carbon", id: owner.uuid }, permanent_url: "http://127.0.0.1:4190/c:ada.example/Product%20roadmap", created_at: "2026-11-16T00:00:00Z", updated_at: "2026-11-16T00:00:00Z", deleted_at: null };
let detail, grants, requests, mode;
function reset() { mode = "owner"; grants = []; requests = []; detail = { org: { org_id: "example", name: "Example Studio", logo_url: null, settings: { ...policy }, admin_team_permissions: { enabled: true, structure: true, members: true, admins: true, settings: true } }, members: [{ ...owner }, { ...colleague }], teams: [{ id: teamId, name: "Design", description: "Plan thoughtful products together.", members: [owner.uuid], admins: [owner.uuid], admin_team: false, settings: { ...policy } }], invites: [], capabilities: { ...caps } }; }
reset();
createServer(async (req, res) => {
  const url = new URL(req.url, "http://127.0.0.1:4191");
  let bytes = ""; for await (const chunk of req) bytes += chunk;
  const body = bytes ? JSON.parse(bytes) : {};
  const reply = (value, status = 200) => { res.writeHead(status, { "Content-Type": "application/json" }); res.end(JSON.stringify(value)); };
  if (url.pathname === "/health") return reply({ ok: true });
  if (url.pathname === "/__reset") { reset(); return reply({ ok: true }); }
  if (url.pathname === "/__mode") { mode = body.mode; return reply({ ok: true }); }
  if (url.pathname === "/__requests") return reply(requests);
  if (!req.headers.authorization?.startsWith("Bearer local-ui-")) return reply({ error: { message: "No test session" } }, 401);
  const route = url.pathname.replace(/^\/api\/v1/, "");
  const targets = [{ type: "organization", id: "example", name: "Example Studio", org_id: "example" }, ...detail.teams.map(t => ({ type: "team", id: `example/${t.id}`, name: t.name, org_id: "example" }))];
  if (route === "/organizations") {
    if (mode === "outage") return reply({ error: { code: "organizations_unavailable", message: "Organization access could not be verified." } }, 503);
    return reply({ account_uuid: owner.uuid, organizations: mode === "personal" ? [] : [{ ...detail.org, capabilities: detail.capabilities, member_count: detail.members.length, team_count: detail.teams.length }], groups: [], share_targets: mode === "personal" ? [] : targets });
  }
  if (route === "/organizations/example") return reply({ ...detail, capabilities: mode === "member" ? { owner: false, admin: false, manage_admins: false, team_admins: [], team_structure: false, team_members: false, team_admins_manage: false, team_settings: false } : detail.capabilities });
  if (route === "/organizations/example/actions") {
    if (!req.headers["idempotency-key"]) return reply({ error: { message: "An action needs an idempotency key" } }, 400);
    requests.push({ body, key: req.headers["idempotency-key"] });
    const team = detail.teams.find(team => team.id === body.team);
    if (body.action === "team_create") detail.teams.push({ id: randomUUID(), name: body.name, description: body.description, members: [], admins: [], admin_team: false, settings: { ...policy } });
    if (body.action === "team_member_add") team.members.push(body.member);
    if (body.action === "team_member_remove") team.members = team.members.filter(id => id !== body.member);
    if (body.action === "team_admin_set") team.admins = body.admin ? [...team.admins, body.member] : team.admins.filter(id => id !== body.member);
    if (body.action === "admin_set") Object.assign(detail.members.find(member => member.uuid === body.member), { admin: body.admin, ...(body.manage_admins !== undefined ? { manage_admins: body.manage_admins } : {}) });
    if (body.action === "team_update") Object.assign(team, Object.fromEntries(Object.entries(body).filter(([key]) => !["action", "team"].includes(key))));
    return reply({ ok: true });
  }
  if (route === "/drives") return reply({ items: [{ owner, relation: "own", root_id: file.parent_id, permanent_url: "http://127.0.0.1:4190/c:ada.example" }] });
  if (route === "/me") return reply(owner);
  if (route === "/entries") return reply({ items: [file], next_cursor: null });
  if (route === `/entries/${file.id}`) return reply(file);
  if (route.endsWith("/link-access")) return reply({ enabled: false, effective: false, can_manage: true, inherited_from: null, expires_at: null, url: null });
  if (route.endsWith("/invitations")) {
    if (req.method === "POST") {
      requests.push({ body, key: req.headers["idempotency-key"] });
      const target = targets.find(target => target.type === body.principal.type && target.id === body.principal.id);
      grants.push({ id: randomUUID(), ...body, principal: { ...body.principal, ...(target ? { name: target.name } : {}) }, expires_at: null });
      return reply(grants.at(-1), 201);
    }
    return reply({ items: grants, next_cursor: null });
  }
  return reply({ items: [], next_cursor: null });
}).listen(4191, "127.0.0.1");
