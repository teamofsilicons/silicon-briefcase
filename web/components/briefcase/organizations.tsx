"use client";
import Link from "next/link";
import { useRef, useState, type FormEvent } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { ArrowLeftRight, ArrowUpRight, Building2, FolderOpen, Plus, Settings, ShieldCheck, Users, UsersRound } from "lucide-react";
import { Page, PageHeader } from "@/components/foundation/layout/layout";
import { ButtonLink } from "@/components/foundation/button-link";
import { ErrorAlert } from "@/components/foundation/feedback/error-alert";
import { Button } from "@/components/silicon-ui/button/button";
import { Input } from "@/components/silicon-ui/input/input";
import { Select } from "@/components/silicon-ui/select/select";
import { Checkbox } from "@/components/silicon-ui/checkbox/checkbox";
import { Dialog, DialogContent } from "@/components/silicon-ui/dialog/dialog";
import { HoldToConfirm } from "@/components/silicon-ui/hold-to-confirm/hold-to-confirm";
import { api } from "@/lib/client/api";
import { useSessionView } from "@/lib/client/session";
import { notify } from "@/lib/notify";
import { message } from "@/lib/briefcase/types";
import { canManageAdmin, canManageTeam, memberRole, type OrganizationDetail, type Team } from "@/lib/briefcase/organizations";
import { useOrganizations } from "./organizations-query";
import styles from "./organizations.module.css";

const switchAccount = "/auth/sign-in?prompt=select_account&return_to=%2Forganizations";

export function OrganizationsHome() {
  const query = useOrganizations();
  const session = useSessionView();
  return <Page><PageHeader title="Organizations" description="Your people, teams, and shared work, together in Briefcase." actions={<ButtonLink href={switchAccount} external variant="secondary"><ArrowLeftRight size={16} />Switch account</ButtonLink>} />
    <div className={styles.intro}><span className={styles.icon}><Building2 size={24} /></span><div><strong>{session.account?.display_name || "Your workspace"}</strong><p>Personal files stay with your personal account. Sign in with a work account to manage its organization.</p></div></div>
    {query.error ? <ErrorAlert error={query.error} action={<Button onClick={() => void query.refetch()}>Try again</Button>} /> : query.isPending ? <p role="status">Loading your organizations…</p> : query.data?.organizations.length ? <div className={styles.cards}>{query.data.organizations.map(org => <Link key={org.org_id} href={`/organizations/${encodeURIComponent(org.org_id)}`} className={styles.card}><div className={styles.cardTop}><span className={styles.icon}><Building2 size={22} /></span><span className={styles.badge}>{org.capabilities.owner ? "Owner" : org.capabilities.admin ? "Admin" : "Member"}</span></div><h2>{org.name}</h2><p>@{org.org_id}</p><div className={styles.meta}><span>{org.member_count} visible members</span><span>{org.team_count} teams</span></div><span className={styles.actions}>Open workspace <ArrowUpRight size={16} /></span></Link>)}</div> : <div className={styles.empty}><UsersRound size={32} aria-hidden="true" /><h2>Bring your work account</h2><p>This account has no organization membership. Choose a linked work account in Silicon Accounts to see its people, teams, and sharing controls here.</p><ButtonLink href={switchAccount} external>Choose a work account</ButtonLink><ButtonLink href="https://organizations.teamofsilicons.com/" external variant="ghost">Create or join an organization <ArrowUpRight size={15} /></ButtonLink></div>}
  </Page>;
}

type Action = (body: Record<string, unknown>, success?: string) => Promise<boolean>;
export function OrganizationWorkspace({ organization }: { organization: string }) {
  const session = useSessionView();
  const cache = useQueryClient();
  const endpoint = `/v1/organizations/${encodeURIComponent(organization)}`;
  const query = useQuery({ queryKey: ["organization", session.account?.uuid, organization], queryFn: () => api.get<OrganizationDetail>(endpoint), refetchInterval: 30_000 });
  const [tab, setTab] = useState("people");
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState("");
  const pending = useRef<{ fingerprint: string; key: string } | null>(null);
  const action: Action = async (body, success = "Organization updated") => {
    if (busy) return false;
    const fingerprint = JSON.stringify(body);
    if (pending.current?.fingerprint !== fingerprint) pending.current = { fingerprint, key: crypto.randomUUID() };
    setBusy(true); setFailure("");
    try {
      await api.post(endpoint + "/actions", body, { idempotencyKey: pending.current.key });
      pending.current = null;
      await Promise.all([cache.invalidateQueries({ queryKey: ["organization"] }), cache.invalidateQueries({ queryKey: ["organizations"] }), cache.invalidateQueries({ queryKey: ["files"] })]);
      notify.success(success); return true;
    } catch (error) { setFailure(message(error)); notify.error(message(error)); return false; }
    finally { setBusy(false); }
  };
  if (query.error) return <Page><PageHeader title="Organization unavailable" back={{ href: "/organizations", label: "Organizations" }} /><ErrorAlert error={query.error} action={<Button onClick={() => void query.refetch()}>Try again</Button>} /><ButtonLink href={switchAccount} external variant="secondary">Choose another account</ButtonLink></Page>;
  if (!query.data) return <Page><p role="status">Opening your organization…</p></Page>;
  const detail = query.data;
  return <Page><PageHeader title={detail.org.name} description={`@${detail.org.org_id} · ${detail.capabilities.owner ? "Owner" : detail.capabilities.admin ? "Administrator" : "Member"}`} back={{ href: "/organizations", label: "Organizations" }} actions={<ButtonLink href="/files" variant="secondary"><FolderOpen size={17} />Your work files</ButtonLink>} />
    <div className={styles.tabs} aria-label="Organization sections">{[{ id: "people", label: "People", icon: Users }, { id: "teams", label: "Teams", icon: UsersRound }, { id: "settings", label: "Settings", icon: Settings }].map(item => <button key={item.id} aria-current={tab === item.id ? "page" : undefined} onClick={() => { setTab(item.id); setFailure(""); }}><item.icon size={17} />{item.label}</button>)}</div>
    {failure && <p role="alert" className={styles.error}>{failure}</p>}
    {tab === "people" ? <People detail={detail} action={action} busy={busy} /> : tab === "teams" ? <Teams detail={detail} action={action} busy={busy} /> : <OrganizationSettings key={detail.org.org_id} detail={detail} action={action} busy={busy} />}
  </Page>;
}

function People({ detail, action, busy }: { detail: OrganizationDetail; action: Action; busy: boolean }) {
  const [search, setSearch] = useState("");
  const [inviting, setInviting] = useState(false);
  const [target, setTarget] = useState("");
  const [handle, setHandle] = useState("");
  const [team, setTeam] = useState("none");
  const caps = detail.capabilities;
  const members = detail.members.filter(member => !member.removed_at && `${member.id} ${member.display_name}`.toLowerCase().includes(search.toLowerCase()));
  const invites = detail.invites.filter(invite => invite.status === "pending");
  async function invite(event: FormEvent) {
    event.preventDefault();
    if (await action({ action: "invite", target: target.trim(), handle: handle.trim(), teams: team === "none" ? [] : [team] }, "Invitation sent")) { setInviting(false); setTarget(""); setHandle(""); }
  }
  return <section className={styles.section}>
    <div className={styles.sectionHeader}><div><h2>People &amp; access</h2><p>Manage organization roles and see who you work with.</p></div>{caps.admin && <Button onClick={() => setInviting(true)}><Plus size={16} />Invite a member</Button>}</div>
    <Input label="Find a member" placeholder="Search by name or account ID" value={search} onChange={event => setSearch(event.target.value)} />
    <ul className={styles.list}>{members.map(member => <li key={member.uuid}><span className={styles.avatar}>{(member.display_name || member.id).slice(0, 2).toUpperCase()}</span><div className={styles.person}><strong>{member.display_name || member.id}</strong><small>{member.id}</small></div><span className={styles.badge}>{(member.owner || member.effective_admin || member.admin) && <ShieldCheck size={13} />}{memberRole(member)}</span><div className={styles.actions}>
      {canManageAdmin(caps, member) && <Button size="sm" variant="ghost" disabled={busy} onClick={() => void action({ action: "admin_set", member: member.uuid, admin: !member.admin }, member.admin ? "Admin appointment removed" : "Admin appointed")}>{member.admin ? "Remove admin" : "Make admin"}</Button>}
      {caps.owner && member.admin && !member.owner && <Button size="sm" variant="ghost" disabled={busy} onClick={() => void action({ action: "admin_set", member: member.uuid, admin: true, manage_admins: !member.manage_admins }, "Admin permissions updated")}>{member.manage_admins ? "Restrict admin management" : "Allow managing admins"}</Button>}
      {caps.admin && !member.owner && (!(member.admin || member.effective_admin) || canManageAdmin(caps, member)) && <HoldToConfirm label={`Remove ${member.display_name || member.id}`} tone="danger" disabled={busy} onConfirm={() => { void action({ action: "member_remove", member: member.uuid }, "Member removed"); }} />}
    </div></li>)}</ul>
    {!members.length && <p className={styles.muted}>No members match that search.</p>}
    <p className={styles.notice}>Owners manage all organization settings. Admins can manage people and teams within their permissions. An admin team also gives its members organization admin access.</p>
    {invites.length > 0 && <><h2>Pending invitations</h2><ul className={styles.list}>{invites.map(invite => <li key={invite.id}><div className={styles.person}><strong>{invite.target}</strong><small>{invite.handle} · expires {new Date(invite.expires_at * 1000).toLocaleDateString()}</small></div>{caps.admin && <div className={styles.actions}><Button size="sm" variant="ghost" disabled={busy} onClick={() => void action({ action: "invite_resend", invite: invite.id }, "Invitation resent")}>Resend</Button><Button size="sm" variant="ghost" disabled={busy} onClick={() => void action({ action: "invite_cancel", invite: invite.id }, "Invitation cancelled")}>Cancel</Button></div>}</li>)}</ul></>}
    <Dialog open={inviting} onOpenChange={setInviting}><DialogContent title="Invite a member" description="They’ll receive an invitation to create a work account in this organization."><form className={styles.form} onSubmit={invite}><Input label="Email or Carbon account ID" value={target} onChange={event => setTarget(event.target.value)} required /><Input label="Work handle" description={`Their account will use this handle in ${detail.org.org_id}.`} value={handle} onChange={event => setHandle(event.target.value)} pattern="[a-z0-9_-]+" required /><Select label="Add to a team" value={team} onValueChange={setTeam} options={[{ value: "none", label: "No team yet" }, ...detail.teams.map(team => ({ value: team.id, label: team.name }))]} /><Button type="submit" loading={busy}>Send invitation</Button></form></DialogContent></Dialog>
  </section>;
}

function Teams({ detail, action, busy }: { detail: OrganizationDetail; action: Action; busy: boolean }) {
  const [creating, setCreating] = useState(false);
  const [selected, setSelected] = useState<string | null>(null);
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const team = detail.teams.find(team => team.id === selected);
  async function create(event: FormEvent) { event.preventDefault(); if (await action({ action: "team_create", name: name.trim(), description: description.trim() }, "Team created")) { setCreating(false); setName(""); setDescription(""); } }
  return <section className={styles.section}><div className={styles.sectionHeader}><div><h2>Teams</h2><p>Share a file once. Access follows the team’s current membership.</p></div>{detail.capabilities.team_structure && <Button onClick={() => setCreating(true)}><Plus size={16} />Create a team</Button>}</div>
    <div className={styles.cards}>{detail.teams.map(team => <div className={styles.card} key={team.id}><div className={styles.cardTop}><span className={styles.icon}><UsersRound size={22} /></span>{team.admin_team && <span className={styles.badge}><ShieldCheck size={13} />Admin team</span>}</div><h3>{team.name}</h3><p>{team.description || "A shared space for the people working together."}</p><div className={styles.meta}><span>{team.members.length} members</span><span>{team.admins.length} team admins</span></div><Button variant="secondary" onClick={() => setSelected(team.id)}>Open team</Button></div>)}</div>
    {!detail.teams.length && <div className={styles.empty}><UsersRound size={30} /><h2>A place for every team</h2><p>{detail.capabilities.team_structure ? "Create a team, add people, then choose it when sharing files or folders." : "Your teams will appear here when you’re added to one."}</p></div>}
    <Dialog open={creating} onOpenChange={setCreating}><DialogContent title="Create a team" description="Give people a common place in your organization."><form className={styles.form} onSubmit={create}><Input label="Team name" placeholder="Design, Engineering, Operations…" value={name} onChange={event => setName(event.target.value)} required maxLength={160} /><Input label="Description" value={description} onChange={event => setDescription(event.target.value)} maxLength={4000} /><Button type="submit" loading={busy}>Create team</Button></form></DialogContent></Dialog>
    {team && <Dialog open onOpenChange={open => { if (!open) setSelected(null); }}><DialogContent title={team.name} description="Members, team admins, and team settings."><TeamEditor key={team.id} team={team} detail={detail} action={action} busy={busy} onDeleted={() => setSelected(null)} /></DialogContent></Dialog>}
  </section>;
}

function TeamEditor({ team, detail, action, busy, onDeleted }: { team: Team; detail: OrganizationDetail; action: Action; busy: boolean; onDeleted: () => void }) {
  const [member, setMember] = useState("");
  const [name, setName] = useState(team.name);
  const [description, setDescription] = useState(team.description || "");
  const [extraApps, setExtraApps] = useState(team.settings.extra_apps.join(", "));
  const [excludedApps, setExcludedApps] = useState(team.settings.excluded_apps.join(", "));
  const caps = detail.capabilities;
  const canMembers = canManageTeam(caps, team, "members") && (!team.admin_team || caps.manage_admins);
  const canAdmins = canManageTeam(caps, team, "admins");
  const available = detail.members.filter(member => !member.removed_at && !team.members.includes(member.uuid) && (!team.admin_team || canManageAdmin(caps, member)));
  return <div className={styles.form}>
    {caps.team_structure && <form className={styles.form} onSubmit={event => { event.preventDefault(); void action({ action: "team_update", team: team.id, name: name.trim(), description: description.trim() }, "Team updated"); }}><div className={styles.fields}><Input label="Team name" value={name} onChange={event => setName(event.target.value)} required /><Input label="Description" value={description} onChange={event => setDescription(event.target.value)} /></div><Button type="submit" variant="secondary" loading={busy}>Save team details</Button></form>}
    <ul className={styles.list}>{detail.members.filter(member => !member.removed_at && team.members.includes(member.uuid)).map(member => <li key={member.uuid}><div className={styles.person}><strong>{member.display_name || member.id}</strong><small>{member.id}{team.admins.includes(member.uuid) ? " · Team admin" : ""}</small></div><div className={styles.actions}>{canAdmins && <Button size="sm" variant="ghost" disabled={busy} onClick={() => void action({ action: "team_admin_set", team: team.id, member: member.uuid, admin: !team.admins.includes(member.uuid) }, "Team admin updated")}>{team.admins.includes(member.uuid) ? "Remove team admin" : "Make team admin"}</Button>}{canMembers && (!team.admin_team || canManageAdmin(caps, member)) && <Button size="sm" variant="ghost" disabled={busy} onClick={() => void action({ action: "team_member_remove", team: team.id, member: member.uuid }, "Removed from team")}>Remove</Button>}</div></li>)}</ul>
    {canMembers && available.length > 0 && <form className={styles.form} onSubmit={event => { event.preventDefault(); if (member) void action({ action: "team_member_add", team: team.id, member }, "Added to team").then(ok => { if (ok) setMember(""); }); }}><Select label="Add an organization member" value={member} onValueChange={setMember} options={available.map(member => ({ value: member.uuid, label: `${member.display_name || member.id} · ${member.id}` }))} /><Button type="submit" disabled={!member} loading={busy}>Add to team</Button></form>}
    {caps.team_settings && caps.manage_admins && <Checkbox label="Admin team" description="Every member of this team becomes an organization admin. Changing membership changes that authority." checked={team.admin_team} disabled={busy} onCheckedChange={value => void action({ action: "team_update", team: team.id, admin_team: value === true }, "Admin team setting updated")} />}
    {canManageTeam(caps, team, "settings") && <form className={styles.form} onSubmit={event => { event.preventDefault(); void action({ action: "policy_update", scope: "team", team: team.id, settings: { ...team.settings, extra_apps: caps.team_settings ? extraApps.split(/[\s,]+/).filter(Boolean) : team.settings.extra_apps, excluded_apps: excludedApps.split(/[\s,]+/).filter(Boolean) } }, "Team app access updated"); }}><h3>Team app access</h3><p className={styles.muted}>Exclusions take priority over permissions from other teams. {caps.team_settings ? "Use app IDs, separated by commas." : "Team admins can add exclusions. An organization admin must remove exclusions or allow extra apps."}</p>{caps.team_settings && <Input label="Extra apps allowed for this team" value={extraApps} onChange={event => setExtraApps(event.target.value)} placeholder="briefcase, write" />}<Input label="Apps excluded for this team" value={excludedApps} onChange={event => setExcludedApps(event.target.value)} placeholder="App IDs, separated by commas" /><Button type="submit" variant="secondary" loading={busy}>Save team app access</Button></form>}
    <p className={styles.notice}>Removing someone from the team removes access granted through this team. Any separate file shares still apply.</p>
    {caps.team_structure && (!team.admin_team || caps.manage_admins) && <HoldToConfirm label="Hold to delete team" tone="danger" disabled={busy} onConfirm={() => { void action({ action: "team_delete", team: team.id }, "Team deleted").then(ok => { if (ok) onDeleted(); }); }} />}
  </div>;
}

function OrganizationSettings({ detail, action, busy }: { detail: OrganizationDetail; action: Action; busy: boolean }) {
  const [name, setName] = useState(detail.org.name);
  const [mode, setMode] = useState(detail.org.settings.mode);
  const [apps, setApps] = useState(detail.org.settings.apps.join(", "));
  const caps = detail.capabilities;
  return <section className={styles.section}><div className={styles.sectionHeader}><div><h2>Organization settings</h2><p>Your permissions decide which settings you can change.</p></div></div>
    <div className={styles.cards}><form className={styles.card} onSubmit={event => { event.preventDefault(); void action({ action: "update", name: name.trim() }, "Organization renamed"); }}><h3>Identity</h3><Input label="Organization name" value={name} onChange={event => setName(event.target.value)} required disabled={!caps.admin} /><p className={styles.muted}>Permanent ID: @{detail.org.org_id}</p>{caps.admin && <Button type="submit" loading={busy}>Save name</Button>}</form>
    <div className={styles.card}><h3>Admins can manage teams</h3><p>Owners control which team actions organization admins can take. Team admins keep their own scoped permissions.</p>{(["enabled", "structure", "members", "admins", "settings"] as const).map(key => <Checkbox key={key} label={{ enabled: "Allow team management", structure: "Create, rename, and delete teams", members: "Add and remove team members", admins: "Appoint team admins", settings: "Change team settings" }[key]} checked={detail.org.admin_team_permissions[key]} disabled={busy || !caps.owner || (key !== "enabled" && !detail.org.admin_team_permissions.enabled)} onCheckedChange={value => void action({ action: "update", admin_team_permissions: { ...detail.org.admin_team_permissions, [key]: value === true } }, "Team permissions updated")} />)}</div></div>
    {caps.admin && <form className={styles.card} onSubmit={event => { event.preventDefault(); void action({ action: "policy_update", scope: "org", settings: { ...detail.org.settings, mode, apps: apps.split(/[\s,]+/).filter(Boolean) } }, "App access updated"); }}><h3>App access</h3><p>Set the organization’s base policy. Member and team exclusions still apply.</p><div className={styles.fields}><Select label="Organization policy" value={mode} onValueChange={value => setMode(value as typeof mode)} options={[{ value: "everywhere", label: "Allow all apps" }, { value: "only", label: "Allow only listed apps" }, { value: "except", label: "Allow all except listed apps" }]} /><Input label="App IDs, separated by commas" placeholder="briefcase, silicon-write" value={apps} disabled={mode === "everywhere"} onChange={event => setApps(event.target.value)} /></div><Button type="submit" loading={busy}>Save app access</Button></form>}
  </section>;
}
