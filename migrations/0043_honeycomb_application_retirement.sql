-- A retired participant can be explicitly imported again into the same world;
-- a permanently purged environment cannot. Both block old credentials.
ALTER TABLE briefcase.honeycomb_environments
 DROP CONSTRAINT honeycomb_environments_state_check,
 ADD CONSTRAINT honeycomb_environments_state_check
 CHECK(state IN ('active','disabled','cleaning','retired','purged'));
