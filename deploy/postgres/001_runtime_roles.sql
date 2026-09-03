CREATE ROLE briefcase_api
    LOGIN
    PASSWORD 'briefcase-api-local-only'
    NOSUPERUSER
    NOCREATEDB
    NOCREATEROLE
    NOINHERIT
    NOBYPASSRLS;

CREATE ROLE briefcase_worker
    LOGIN
    PASSWORD 'briefcase-worker-local-only'
    NOSUPERUSER
    NOCREATEDB
    NOCREATEROLE
    NOINHERIT
    BYPASSRLS;
