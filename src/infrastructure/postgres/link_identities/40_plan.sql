-- briefcase-migrate link-identities, part 4: move each organization's files
-- into drives.
--
-- Every file or folder whose parent is one of the organization's structural
-- folders starts a subtree, and its place follows the folder it was in:
--
--   private/{member}/…              → the member's drive (top)
--   apps/{app}/private/{member}/…   → the member's drive, apps/{app}/
--   public/…                        → the owner's drive, {org}-public/
--   {tag}/…                         → the owner's drive, {org}-{tag}/
--   apps/{app}/public/…             → the owner's drive, apps/{app}/{org}-public/
--   a member without an account     → the owner's drive, unclaimed/{member id}/
--   anything else                   → the owner's drive, unclaimed/{org}/
--
-- The owner is the organization's active owner, or the account the mapping
-- names with `org:{org},{uuid}`. Creators keep their files; shared spaces get
-- snapshot shares afterwards (part 5).

CREATE TEMP TABLE link_orgs (
    org text PRIMARY KEY,
    owner_uuid text NOT NULL
) ON COMMIT DROP;
CREATE TEMP TABLE link_spaces (
    folder uuid PRIMARY KEY,
    drive text NOT NULL,
    org text NOT NULL,
    legacy_path text NOT NULL,
    audience text NOT NULL,
    tag_id text
) ON COMMIT DROP;
CREATE TEMP TABLE link_renamed (
    org text NOT NULL,
    legacy_path text NOT NULL,
    drive text NOT NULL,
    name text NOT NULL
) ON COMMIT DROP;
CREATE TEMP TABLE link_containers (
    org text NOT NULL,
    legacy_path text NOT NULL,
    drive text NOT NULL,
    entry_id uuid NOT NULL,
    PRIMARY KEY (org, legacy_path)
) ON COMMIT DROP;
CREATE TEMP TABLE link_unclaimed (
    folder uuid PRIMARY KEY,
    drive text NOT NULL,
    org text NOT NULL,
    legacy_id text NOT NULL
) ON COMMIT DROP;

CREATE OR REPLACE FUNCTION pg_temp.link_is_mapped(p_id text)
RETURNS boolean
LANGUAGE sql
STABLE
AS $$ SELECT EXISTS (SELECT 1 FROM link_map WHERE accounts_uuid = p_id) $$;

-- Who receives each organization's shared spaces. Runs after the re-key, so
-- members are named by uuid when the mapping linked them.
CREATE OR REPLACE FUNCTION pg_temp.link_owners()
RETURNS void
LANGUAGE plpgsql
AS $$
DECLARE
    organization record;
    chosen text;
    owners text[];
BEGIN
    FOR organization IN
        SELECT legacy.org FROM link_legacy_orgs AS legacy
         WHERE EXISTS (SELECT 1 FROM briefcase.entries AS entry
                        WHERE entry.org_id = legacy.org AND entry.system_kind IS NULL)
         ORDER BY legacy.org
    LOOP
        SELECT accounts_uuid INTO chosen FROM link_owner_override WHERE org = organization.org;
        IF chosen IS NULL THEN
            SELECT array_agg(member.actor_id ORDER BY member.actor_id) INTO owners
              FROM briefcase.organization_members AS member
             WHERE member.org_id = organization.org AND member.org_role = 'owner'
               AND member.membership_status = 'active';
            IF owners IS NULL THEN
                RAISE EXCEPTION 'organization % has no active owner to receive its shared folders; add the line org:%,<account uuid>',
                    organization.org, organization.org;
            END IF;
            IF cardinality(owners) > 1 THEN
                RAISE EXCEPTION 'organization % has several owners (%); add the line org:%,<account uuid> naming the one that receives its shared folders',
                    organization.org, array_to_string(owners, ', '), organization.org;
            END IF;
            chosen := owners[1];
            IF NOT pg_temp.link_is_mapped(chosen) THEN
                RAISE EXCEPTION 'the owner % of organization % is not linked to an account; link it, or add the line org:%,<account uuid>',
                    chosen, organization.org, organization.org;
            END IF;
        END IF;
        INSERT INTO link_orgs VALUES (organization.org, chosen);
    END LOOP;
END;
$$;

-- The unclaimed area for a member without an account, in the owner's drive.
CREATE OR REPLACE FUNCTION pg_temp.link_unclaimed_folder(p_owner text, p_org text, p_name text)
RETURNS uuid
LANGUAGE plpgsql
AS $$
DECLARE
    area uuid := pg_temp.link_folder('unclaimed:' || p_owner, p_owner,
                                     pg_temp.link_drive(p_owner), 'unclaimed', p_org);
    folder uuid;
BEGIN
    folder := pg_temp.link_folder('unclaimed:' || p_owner || ':' || p_org || ':' || p_name,
                                  p_owner, area, p_name, p_org);
    INSERT INTO link_unclaimed VALUES (folder, p_owner, p_org, p_name) ON CONFLICT DO NOTHING;
    RETURN folder;
END;
$$;

CREATE OR REPLACE FUNCTION pg_temp.link_move_organizations()
RETURNS void
LANGUAGE plpgsql
AS $$
DECLARE
    organization record;
    child record;
    target_drive text;
    target_parent uuid;
    app text;
    chosen_name text;
BEGIN
    IF EXISTS (
        SELECT 1 FROM briefcase.entries AS structural
          JOIN link_legacy_orgs ON link_legacy_orgs.org = structural.org_id
          JOIN briefcase.entry_closure AS closure
            ON closure.org_id = structural.org_id AND closure.descendant_id = structural.entry_id
           AND closure.depth > 0
          JOIN briefcase.entries AS above
            ON above.org_id = closure.org_id AND above.entry_id = closure.ancestor_id
         WHERE structural.system_kind IS NOT NULL AND above.system_kind IS NULL
    ) THEN
        RAISE EXCEPTION 'an organization has a structural folder inside an ordinary folder; this layout is not one Briefcase created, fix it before linking';
    END IF;
    FOR organization IN SELECT * FROM link_orgs ORDER BY org LOOP
        PERFORM pg_temp.link_drive(organization.owner_uuid);
        FOR child IN
            SELECT entry.entry_id, entry.name, entry.path, parent.entry_id AS parent_id,
                   parent.path AS parent_path, parent.name AS parent_name,
                   parent.system_kind AS parent_kind, parent.owner_id AS parent_owner,
                   parent.origin_app_id AS parent_app, parent.tag_id AS parent_tag,
                   parent.link_public AS parent_link, parent.link_expires_at AS parent_link_expires
              FROM briefcase.entries AS entry
              LEFT JOIN briefcase.entries AS parent
                ON parent.org_id = entry.org_id AND parent.entry_id = entry.parent_id
             WHERE entry.org_id = organization.org AND entry.system_kind IS NULL
               AND (entry.parent_id IS NULL OR parent.system_kind IS NOT NULL)
             ORDER BY entry.path, entry.entry_id
        LOOP
            app := COALESCE(child.parent_app, child.parent_name);
            target_drive := organization.owner_uuid;
            CASE child.parent_kind
                WHEN 'public_root' THEN
                    target_parent := pg_temp.link_folder(
                        'space:' || organization.org || ':public', target_drive,
                        pg_temp.link_drive(target_drive), organization.org || '-public',
                        organization.org, child.parent_link, child.parent_link_expires);
                    INSERT INTO link_spaces VALUES (target_parent, target_drive, organization.org,
                                                    child.parent_path, 'members', NULL)
                    ON CONFLICT DO NOTHING;
                WHEN 'tag_root' THEN
                    target_parent := pg_temp.link_folder(
                        'space:' || organization.org || ':tag:' || child.parent_tag, target_drive,
                        pg_temp.link_drive(target_drive),
                        organization.org || '-' || child.parent_name,
                        organization.org, child.parent_link, child.parent_link_expires);
                    INSERT INTO link_spaces VALUES (target_parent, target_drive, organization.org,
                                                    child.parent_path, 'tag', child.parent_tag)
                    ON CONFLICT DO NOTHING;
                WHEN 'app_public' THEN
                    target_parent := pg_temp.link_folder(
                        'space:' || organization.org || ':app:' || app, target_drive,
                        pg_temp.link_app_folder(target_drive, app), organization.org || '-public',
                        organization.org, child.parent_link, child.parent_link_expires);
                    INSERT INTO link_spaces VALUES (target_parent, target_drive, organization.org,
                                                    child.parent_path, 'members', NULL)
                    ON CONFLICT DO NOTHING;
                WHEN 'actor_root' THEN
                    IF pg_temp.link_is_mapped(child.parent_owner) THEN
                        target_drive := child.parent_owner;
                        target_parent := pg_temp.link_drive(target_drive);
                    ELSE
                        target_parent := pg_temp.link_unclaimed_folder(
                            target_drive, organization.org, child.parent_owner);
                    END IF;
                WHEN 'app_actor', 'app_container' THEN
                    IF pg_temp.link_is_mapped(child.parent_owner) THEN
                        target_drive := child.parent_owner;
                        target_parent := pg_temp.link_app_folder(target_drive, app);
                    ELSE
                        target_parent := pg_temp.link_folder(
                            'unclaimed:' || target_drive || ':' || organization.org || ':'
                                || child.parent_owner || ':app:' || app,
                            target_drive,
                            pg_temp.link_folder(
                                'unclaimed:' || target_drive || ':' || organization.org || ':'
                                    || child.parent_owner || ':apps',
                                target_drive,
                                pg_temp.link_unclaimed_folder(target_drive, organization.org,
                                                              child.parent_owner),
                                'apps', organization.org),
                            app, organization.org);
                    END IF;
                ELSE
                    target_parent := pg_temp.link_unclaimed_folder(
                        target_drive, organization.org, organization.org);
            END CASE;
            IF child.parent_id IS NOT NULL THEN
                INSERT INTO link_containers VALUES (organization.org, child.parent_path,
                                                    target_drive, target_parent)
                ON CONFLICT DO NOTHING;
            END IF;
            chosen_name := pg_temp.link_free_name(target_drive, target_parent, child.name,
                                                  organization.org);
            IF chosen_name <> child.name THEN
                INSERT INTO link_renamed VALUES (organization.org, child.path, target_drive, chosen_name);
            END IF;
            PERFORM pg_temp.link_move(child.entry_id, organization.org, target_drive,
                                      target_parent, chosen_name, true);
        END LOOP;
    END LOOP;
END;
$$;
