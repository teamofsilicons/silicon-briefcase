-- Work accounts add one organization segment to the personal account ID.
-- Keep these checks aligned with PublicId while retaining the existing rows
-- and the same validation for each component.
ALTER TABLE briefcase.accounts
    DROP CONSTRAINT accounts_public_id_check,
    ADD CONSTRAINT accounts_public_id_check
        CHECK (public_id IS NULL OR public_id ~ '^(c|si):[a-z0-9_-]{1,64}([.][a-z0-9_-]{1,64})?$');

ALTER TABLE briefcase.account_id_history
    DROP CONSTRAINT account_id_history_public_id_check,
    ADD CONSTRAINT account_id_history_public_id_check
        CHECK (public_id ~ '^(c|si):[a-z0-9_-]{1,64}([.][a-z0-9_-]{1,64})?$');
