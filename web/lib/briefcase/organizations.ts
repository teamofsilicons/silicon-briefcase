export type Capabilities = { owner: boolean; admin: boolean; manage_admins: boolean; team_admins: string[]; team_structure: boolean; team_members: boolean; team_admins_manage: boolean; team_settings: boolean };
export type AppPolicy = { mode: "everywhere" | "only" | "except"; apps: string[]; extra_apps: string[]; excluded_apps: string[] };
export type TeamPermissions = { enabled: boolean; structure: boolean; members: boolean; admins: boolean; settings: boolean };
export type OrganizationSummary = { org_id: string; name: string; logo_url: string | null; capabilities: Capabilities; member_count: number; team_count: number };
export type ShareTarget = { type: "organization" | "team"; id: string; name: string; org_id: string };
export type Organizations = { account_uuid: string; organizations: OrganizationSummary[]; groups: string[]; share_targets: ShareTarget[] };
export type Member = { uuid: string; id: string; display_name: string; kind: "carbon" | "silicon"; owner: boolean; admin: boolean; effective_admin?: boolean; manage_admins: boolean; removed_at?: number | null };
export type Team = { id: string; name: string; description: string; members: string[]; admins: string[]; admin_team: boolean; settings: AppPolicy };
export type Invitation = { id: string; target: string; handle: string; status: string; expires_at: number; teams: string[] };
export type OrganizationDetail = { org: { org_id: string; name: string; logo_url: string | null; settings: AppPolicy; admin_team_permissions: TeamPermissions }; members: Member[]; teams: Team[]; invites: Invitation[]; capabilities: Capabilities };

export function canManageAdmin(caps: Capabilities, member: Member): boolean {
  return !member.owner && (caps.owner || (caps.manage_admins && !member.manage_admins));
}
export function canManageTeam(caps: Capabilities, team: Team, part: "members" | "admins" | "settings"): boolean {
  return caps.team_admins.includes(team.id) || (part === "members" ? caps.team_members : part === "admins" ? caps.team_admins_manage : caps.team_settings);
}
export function memberRole(member: Member): string {
  return member.owner ? "Owner" : member.manage_admins ? "Admin · manages admins" : member.admin ? "Admin" : member.effective_admin ? "Admin via team" : "Member";
}
export function groupShareBody(target: ShareTarget, access: string[], folder: boolean, minutes: string) {
  const lifetime = minutes ? Number(minutes) : null;
  if (lifetime !== null && (!Number.isInteger(lifetime) || lifetime < 1 || lifetime > 43200)) throw new Error("Choose a whole number of minutes from 1 to 43,200.");
  return { principal: { type: target.type, id: target.id }, access: lifetime ? ["read"] : access.filter(right => folder || right !== "write"), inherit: folder, ...(lifetime ? { expires_in_minutes: lifetime } : {}) };
}
