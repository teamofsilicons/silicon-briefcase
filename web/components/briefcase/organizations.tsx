"use client";
import { useRef, useState, type FormEvent } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { ArrowLeftRight, ArrowUpRight, Building2, FolderOpen, Plus, ShieldCheck, UsersRound } from "lucide-react";
import { Page } from "@/components/foundation/layout/layout";
import { ButtonLink } from "@/components/foundation/button-link";
import { PageBar } from "@/components/foundation/page/page-bar";
import { LinkItem, Section, Tile } from "@/components/foundation/page/rows";
import { ErrorAlert } from "@/components/foundation/feedback/error-alert";
import { SkeletonBlock } from "@/components/foundation/feedback/skeleton-block";
import { Alert } from "@teamofsilicons/silicon-ui/alert";
import { Badge } from "@teamofsilicons/silicon-ui/badge";
import { Button } from "@teamofsilicons/silicon-ui/button";
import { Input } from "@teamofsilicons/silicon-ui/input";
import { Select } from "@teamofsilicons/silicon-ui/select";
import { Checkbox } from "@teamofsilicons/silicon-ui/checkbox";
import { Dialog, DialogContent } from "@teamofsilicons/silicon-ui/dialog";
import { HoldToConfirm } from "@teamofsilicons/silicon-ui/hold-to-confirm";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@teamofsilicons/silicon-ui/tabs";
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
  return <>
    <PageBar title="Organizations" actions={<ButtonLink href={switchAccount} external variant="secondary" size="sm"><ArrowLeftRight size={16} strokeWidth={2} aria-hidden="true" />Switch Account</ButtonLink>} />
    <Page>
      <p className={styles.lead}>Your people, teams, and shared work, together in Briefcase. Signed in as <strong>{session.account?.display_name || session.account?.id || "you"}</strong>: personal files stay with your personal account, and a work account manages its organization.</p>
      {query.error ? <ErrorAlert error={query.error} action={<Button variant="secondary" size="sm" onClick={() => void query.refetch()}>Try Again</Button>} />
        : query.isPending ? <div role="status" aria-label="Loading your organizations"><SkeletonBlock width="100%" height="3.25rem" radius="var(--radius-container)" /></div>
        : query.data?.organizations.length ? (
          <Section id="organizations" title="Your organizations" count={query.data.organizations.length}>
            {query.data.organizations.map(org => (
              <LinkItem key={org.org_id} href={`/organizations/${encodeURIComponent(org.org_id)}`} lead={<Tile><Building2 size={16} strokeWidth={2} /></Tile>} title={org.name} detail={`@${org.org_id} · ${org.member_count} visible members · ${org.team_count} teams`} end={<Badge size="sm" tone="neutral">{org.capabilities.owner ? "Owner" : org.capabilities.admin ? "Admin" : "Member"}</Badge>} />
            ))}
          </Section>
        ) : (
          <div data-sq="surface" className={styles.empty}>
            <span data-sq="surface" className={styles.emptyIcon} aria-hidden="true"><UsersRound size={20} strokeWidth={2} /></span>
            <h2>Bring your work account</h2>
            <p>This account has no organization membership. Choose a linked work account in Silicon Accounts to see its people, teams, and sharing controls here.</p>
            <div className={styles.actions}>
              <ButtonLink href="https://organizations.teamofsilicons.com/" external variant="ghost" size="sm">Create or Join an Organization<ArrowUpRight size={16} strokeWidth={2} aria-hidden="true" /></ButtonLink>
              <ButtonLink href={switchAccount} external size="sm">Choose a Work Account</ButtonLink>
            </div>
          </div>
        )}
    </Page>
  </>;
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
  const parent = { href: "/organizations", label: "Organizations" };
  if (query.error) return <>
    <PageBar parent={parent} title="Organization unavailable" actions={<ButtonLink href={switchAccount} external variant="secondary" size="sm">Choose Another Account</ButtonLink>} />
    <Page><ErrorAlert error={query.error} action={<Button variant="secondary" size="sm" onClick={() => void query.refetch()}>Try Again</Button>} /></Page>
  </>;
  if (!query.data) return <>
    <PageBar parent={parent} title="Organization" />
    <Page><div role="status" aria-label="Opening your organization"><SkeletonBlock width="100%" height="calc(3 * 3.25rem)" radius="var(--radius-container)" /></div></Page>
  </>;
  const detail = query.data;
  return <>
    <PageBar parent={parent} title={detail.org.name} actions={<ButtonLink href="/files" variant="secondary" size="sm"><FolderOpen size={16} strokeWidth={2} aria-hidden="true" />Your Work Files</ButtonLink>} />
    <Page>
      <p className={styles.lead}>@{detail.org.org_id} · {detail.capabilities.owner ? "Owner" : detail.capabilities.admin ? "Administrator" : "Member"}</p>
      <Tabs value={tab} onValueChange={value => { setTab(value); setFailure(""); }} className={styles.tabs}>
        <TabsList aria-label="Organization sections">
          <TabsTrigger value="people">People</TabsTrigger>
          <TabsTrigger value="teams">Teams</TabsTrigger>
          <TabsTrigger value="settings">Settings</TabsTrigger>
        </TabsList>
        {failure ? <Alert tone="danger" title="That did not work" onDismiss={() => setFailure("")}>{failure}</Alert> : null}
        <TabsContent value="people" forceMount className={styles.panel}><People detail={detail} action={action} busy={busy} /></TabsContent>
        <TabsContent value="teams" forceMount className={styles.panel}><Teams detail={detail} action={action} busy={busy} /></TabsContent>
        <TabsContent value="settings" forceMount className={styles.panel}><OrganizationSettings key={detail.org.org_id} detail={detail} action={action} busy={busy} /></TabsContent>
      </Tabs>
    </Page>
  </>;
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
    <div className={styles.sectionHeader}><div><h2>People &amp; access</h2><p>Manage organization roles and see who you work with.</p></div>{caps.admin && <Button size="sm" onClick={() => setInviting(true)}><Plus size={16} strokeWidth={2} aria-hidden="true" />Invite a Member</Button>}</div>
    <Input label="Find a member" placeholder="Search by name or account ID" value={search} onChange={event => setSearch(event.target.value)} />
    <ul data-sq="surface" className={styles.list}>{members.map(member => <li key={member.uuid}><span data-sq="surface" className={styles.avatar} aria-hidden="true">{(member.display_name || member.id).slice(0, 2).toUpperCase()}</span><div className={styles.person}><strong>{member.display_name || member.id}</strong><small>{member.id}</small></div><Badge size="sm" tone="neutral" icon={member.owner || member.effective_admin || member.admin ? <ShieldCheck size={12} strokeWidth={2} /> : undefined}>{memberRole(member)}</Badge><div className={styles.actions}>
      {canManageAdmin(caps, member) && <Button size="sm" variant="ghost" disabled={busy} onClick={() => void action({ action: "admin_set", member: member.uuid, admin: !member.admin }, member.admin ? "Admin appointment removed" : "Admin appointed")}>{member.admin ? "Remove Admin" : "Make Admin"}</Button>}
      {caps.owner && member.admin && !member.owner && <Button size="sm" variant="ghost" disabled={busy} onClick={() => void action({ action: "admin_set", member: member.uuid, admin: true, manage_admins: !member.manage_admins }, "Admin permissions updated")}>{member.manage_admins ? "Restrict Admin Management" : "Allow Managing Admins"}</Button>}
      {caps.admin && !member.owner && (!(member.admin || member.effective_admin) || canManageAdmin(caps, member)) && <HoldToConfirm label={`Remove ${member.display_name || member.id}`} tone="danger" disabled={busy} onConfirm={() => { void action({ action: "member_remove", member: member.uuid }, "Member removed"); }} />}
    </div></li>)}</ul>
    {!members.length && <p className={styles.muted}>No members match that search.</p>}
    <p data-sq="surface" className={styles.notice}>Owners manage all organization settings. Admins can manage people and teams within their permissions. An admin team also gives its members organization admin access.</p>
    {invites.length > 0 && <><h2 className={styles.subTitle}>Pending invitations</h2><ul data-sq="surface" className={styles.list}>{invites.map(invite => <li key={invite.id}><div className={styles.person}><strong>{invite.target}</strong><small>{invite.handle} · expires {new Date(invite.expires_at * 1000).toLocaleDateString()}</small></div>{caps.admin && <div className={styles.actions}><Button size="sm" variant="ghost" disabled={busy} onClick={() => void action({ action: "invite_resend", invite: invite.id }, "Invitation resent")}>Resend</Button><Button size="sm" variant="ghost" disabled={busy} onClick={() => void action({ action: "invite_cancel", invite: invite.id }, "Invitation cancelled")}>Cancel</Button></div>}</li>)}</ul></>}
    <Dialog open={inviting} onOpenChange={setInviting}><DialogContent title="Invite a Member" description="They’ll receive an invitation to create a work account in this organization."><form className={styles.form} onSubmit={invite}><Input label="Email or Carbon account ID" value={target} onChange={event => setTarget(event.target.value)} required /><Input label="Work handle" description={`Their account will use this handle in ${detail.org.org_id}.`} value={handle} onChange={event => setHandle(event.target.value)} pattern="[a-z0-9_-]+" required /><Select label="Add to a team" value={team} onValueChange={setTeam} options={[{ value: "none", label: "No team yet" }, ...detail.teams.map(team => ({ value: team.id, label: team.name }))]} /><div className={styles.dialogActions}><Button type="button" variant="ghost" onClick={() => setInviting(false)}>Cancel</Button><Button type="submit" loading={busy}>Send Invitation</Button></div></form></DialogContent></Dialog>
  </section>;
}

function Teams({ detail, action, busy }: { detail: OrganizationDetail; action: Action; busy: boolean }) {
  const [creating, setCreating] = useState(false);
  const [selected, setSelected] = useState<string | null>(null);
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const team = detail.teams.find(team => team.id === selected);
  async function create(event: FormEvent) { event.preventDefault(); if (await action({ action: "team_create", name: name.trim(), description: description.trim() }, "Team created")) { setCreating(false); setName(""); setDescription(""); } }
  return <section className={styles.section}><div className={styles.sectionHeader}><div><h2>Teams</h2><p>Share a file once. Access follows the team’s current membership.</p></div>{detail.capabilities.team_structure && <Button size="sm" onClick={() => setCreating(true)}><Plus size={16} strokeWidth={2} aria-hidden="true" />Create a Team</Button>}</div>
    <div className={styles.cards}>{detail.teams.map(team => <div data-sq="surface" className={styles.card} key={team.id}><div className={styles.cardTop}><span data-sq="surface" className={styles.icon}><UsersRound size={16} strokeWidth={2} /></span>{team.admin_team && <Badge size="sm" tone="neutral" icon={<ShieldCheck size={12} strokeWidth={2} />}>Admin team</Badge>}</div><h3>{team.name}</h3><p>{team.description || "A shared space for the people working together."}</p><div className={styles.meta}><span>{team.members.length} members</span><span>{team.admins.length} team admins</span></div><Button variant="secondary" size="sm" onClick={() => setSelected(team.id)}>Open Team</Button></div>)}</div>
    {!detail.teams.length && <div data-sq="surface" className={styles.empty}><span data-sq="surface" className={styles.emptyIcon} aria-hidden="true"><UsersRound size={20} strokeWidth={2} /></span><h2>A place for every team</h2><p>{detail.capabilities.team_structure ? "Create a team, add people, then choose it when sharing files or folders." : "Your teams will appear here when you’re added to one."}</p></div>}
    <Dialog open={creating} onOpenChange={setCreating}><DialogContent title="Create a Team" description="Give people a common place in your organization."><form className={styles.form} onSubmit={create}><Input label="Team name" placeholder="Design, Engineering, Operations…" value={name} onChange={event => setName(event.target.value)} required maxLength={160} /><Input label="Description" value={description} onChange={event => setDescription(event.target.value)} maxLength={4000} /><div className={styles.dialogActions}><Button type="button" variant="ghost" onClick={() => setCreating(false)}>Cancel</Button><Button type="submit" loading={busy}>Create Team</Button></div></form></DialogContent></Dialog>
    {team && <Dialog open onOpenChange={open => { if (!open) setSelected(null); }}><DialogContent className={styles.wide} title={team.name} description="Members, team admins, and team settings."><TeamEditor key={team.id} team={team} detail={detail} action={action} busy={busy} onDeleted={() => setSelected(null)} /></DialogContent></Dialog>}
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
    {caps.team_structure && <form className={styles.form} onSubmit={event => { event.preventDefault(); void action({ action: "team_update", team: team.id, name: name.trim(), description: description.trim() }, "Team updated"); }}><div className={styles.fields}><Input label="Team name" value={name} onChange={event => setName(event.target.value)} required /><Input label="Description" value={description} onChange={event => setDescription(event.target.value)} /></div><div className={styles.formEnd}><Button type="submit" variant="secondary" size="sm" loading={busy}>Save Team Details</Button></div></form>}
    <ul data-sq="surface" className={styles.list}>{detail.members.filter(member => !member.removed_at && team.members.includes(member.uuid)).map(member => <li key={member.uuid}><div className={styles.person}><strong>{member.display_name || member.id}</strong><small>{member.id}{team.admins.includes(member.uuid) ? " · Team admin" : ""}</small></div><div className={styles.actions}>{canAdmins && <Button size="sm" variant="ghost" disabled={busy} onClick={() => void action({ action: "team_admin_set", team: team.id, member: member.uuid, admin: !team.admins.includes(member.uuid) }, "Team admin updated")}>{team.admins.includes(member.uuid) ? "Remove Team Admin" : "Make Team Admin"}</Button>}{canMembers && (!team.admin_team || canManageAdmin(caps, member)) && <Button size="sm" variant="ghost" disabled={busy} onClick={() => void action({ action: "team_member_remove", team: team.id, member: member.uuid }, "Removed from team")}>Remove</Button>}</div></li>)}</ul>
    {canMembers && available.length > 0 && <form className={styles.form} onSubmit={event => { event.preventDefault(); if (member) void action({ action: "team_member_add", team: team.id, member }, "Added to team").then(ok => { if (ok) setMember(""); }); }}><Select label="Add an organization member" value={member} onValueChange={setMember} options={available.map(member => ({ value: member.uuid, label: `${member.display_name || member.id} · ${member.id}` }))} /><div className={styles.formEnd}><Button type="submit" size="sm" disabled={!member} loading={busy}>Add to Team</Button></div></form>}
    {caps.team_settings && caps.manage_admins && <Checkbox label="Admin team" description="Every member of this team becomes an organization admin. Changing membership changes that authority." checked={team.admin_team} disabled={busy} onCheckedChange={value => void action({ action: "team_update", team: team.id, admin_team: value === true }, "Admin team setting updated")} />}
    {canManageTeam(caps, team, "settings") && <form className={styles.form} onSubmit={event => { event.preventDefault(); void action({ action: "policy_update", scope: "team", team: team.id, settings: { ...team.settings, extra_apps: caps.team_settings ? extraApps.split(/[\s,]+/).filter(Boolean) : team.settings.extra_apps, excluded_apps: excludedApps.split(/[\s,]+/).filter(Boolean) } }, "Team app access updated"); }}><h3 className={styles.subTitle}>Team app access</h3><p className={styles.muted}>Exclusions take priority over permissions from other teams. {caps.team_settings ? "Use app IDs, separated by commas." : "Team admins can add exclusions. An organization admin must remove exclusions or allow extra apps."}</p>{caps.team_settings && <Input label="Extra apps allowed for this team" value={extraApps} onChange={event => setExtraApps(event.target.value)} placeholder="briefcase, write" />}<Input label="Apps excluded for this team" value={excludedApps} onChange={event => setExcludedApps(event.target.value)} placeholder="App IDs, separated by commas" /><div className={styles.formEnd}><Button type="submit" variant="secondary" size="sm" loading={busy}>Save Team App Access</Button></div></form>}
    <p data-sq="surface" className={styles.notice}>Removing someone from the team removes access granted through this team. Any separate file shares still apply.</p>
    {caps.team_structure && (!team.admin_team || caps.manage_admins) && <HoldToConfirm label="Hold to delete team" tone="danger" disabled={busy} onConfirm={() => { void action({ action: "team_delete", team: team.id }, "Team deleted").then(ok => { if (ok) onDeleted(); }); }} />}
  </div>;
}

function OrganizationSettings({ detail, action, busy }: { detail: OrganizationDetail; action: Action; busy: boolean }) {
  const [name, setName] = useState(detail.org.name);
  const [mode, setMode] = useState(detail.org.settings.mode);
  const [apps, setApps] = useState(detail.org.settings.apps.join(", "));
  const caps = detail.capabilities;
  return <section className={styles.section}><div className={styles.sectionHeader}><div><h2>Organization settings</h2><p>Your permissions decide which settings you can change.</p></div></div>
    <div className={styles.cards}><form data-sq="surface" className={styles.card} onSubmit={event => { event.preventDefault(); void action({ action: "update", name: name.trim() }, "Organization renamed"); }}><h3>Identity</h3><Input label="Organization name" value={name} onChange={event => setName(event.target.value)} required disabled={!caps.admin} /><p className={styles.muted}>Permanent ID: @{detail.org.org_id}</p>{caps.admin && <div className={styles.formEnd}><Button type="submit" size="sm" loading={busy}>Save Name</Button></div>}</form>
    <div data-sq="surface" className={styles.card}><h3>Admins can manage teams</h3><p>Owners control which team actions organization admins can take. Team admins keep their own scoped permissions.</p>{(["enabled", "structure", "members", "admins", "settings"] as const).map(key => <Checkbox key={key} label={{ enabled: "Allow team management", structure: "Create, rename, and delete teams", members: "Add and remove team members", admins: "Appoint team admins", settings: "Change team settings" }[key]} checked={detail.org.admin_team_permissions[key]} disabled={busy || !caps.owner || (key !== "enabled" && !detail.org.admin_team_permissions.enabled)} onCheckedChange={value => void action({ action: "update", admin_team_permissions: { ...detail.org.admin_team_permissions, [key]: value === true } }, "Team permissions updated")} />)}</div></div>
    {caps.admin && <form data-sq="surface" className={styles.card} onSubmit={event => { event.preventDefault(); void action({ action: "policy_update", scope: "org", settings: { ...detail.org.settings, mode, apps: apps.split(/[\s,]+/).filter(Boolean) } }, "App access updated"); }}><h3>App access</h3><p>Set the organization’s base policy. Member and team exclusions still apply.</p><div className={styles.fields}><Select label="Organization policy" value={mode} onValueChange={value => setMode(value as typeof mode)} options={[{ value: "everywhere", label: "Allow all apps" }, { value: "only", label: "Allow only listed apps" }, { value: "except", label: "Allow all except listed apps" }]} /><Input label="App IDs, separated by commas" placeholder="briefcase, silicon-write" value={apps} disabled={mode === "everywhere"} onChange={event => setApps(event.target.value)} /></div><div className={styles.formEnd}><Button type="submit" size="sm" loading={busy}>Save App Access</Button></div></form>}
  </section>;
}
