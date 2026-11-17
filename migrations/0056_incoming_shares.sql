-- Existing accounts and accounts first discovered as share recipients start open.
ALTER TABLE briefcase.accounts
    ADD COLUMN accept_shares_from_anyone boolean NOT NULL DEFAULT true;
