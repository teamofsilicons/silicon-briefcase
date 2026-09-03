-- A filename is a single token to the `simple` dictionary: `to_tsvector` over
-- "notes.md" produces the one lexeme 'notes.md', so searching for "notes"
-- could never match the file it names. Every separator a filename actually
-- uses is turned into whitespace before indexing, which makes each word in a
-- name searchable on its own while the whole name still matches.
--
-- Both sides of the comparison go through this one function so the index and
-- the query can never drift apart: whatever splits a stored filename splits
-- the search terms the same way.
CREATE FUNCTION briefcase.searchable_text(value text)
RETURNS text
LANGUAGE sql
IMMUTABLE
PARALLEL SAFE
STRICT
SET search_path = pg_catalog
AS $$
    SELECT translate(value, './-_+:@', '        ')
$$;

-- Replacing a stored generated column requires dropping it, which also drops
-- its index; both are recreated here against the normalized expression.
ALTER TABLE briefcase.search_documents
    DROP COLUMN filename_search;

ALTER TABLE briefcase.search_documents
    ADD COLUMN filename_search tsvector
        GENERATED ALWAYS AS (
            to_tsvector('simple'::regconfig, briefcase.searchable_text(filename))
        ) STORED;

CREATE INDEX search_documents_filename_gin_idx
    ON briefcase.search_documents USING gin (filename_search);
