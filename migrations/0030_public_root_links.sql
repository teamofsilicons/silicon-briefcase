ALTER TABLE briefcase.entries DROP CONSTRAINT entries_link_public_safe;
ALTER TABLE briefcase.entries ADD CONSTRAINT entries_link_public_safe CHECK (NOT link_public OR system_kind IS NULL OR system_kind IN ('public_root','tag_root','app_public'));
