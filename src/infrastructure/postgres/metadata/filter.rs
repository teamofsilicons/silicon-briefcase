//! Compilation of parsed filters into tenant-safe SQL.
//!
//! Every user-supplied value is bound as a parameter; nothing from the filter
//! text is ever concatenated into the statement. Permission predicates are
//! compiled to `TRUE` here and decided by domain policy afterwards, because
//! effective access is not a column.

use sqlx::{Postgres, QueryBuilder};
use time::Date;

use crate::domain::{
    actor::ActorKind,
    entry::EntryKind,
    filter::{ActorSelector, FilterExpression, FilterPredicate, GlobTerm},
    media::{ALL_RENDER_KINDS, RenderKind},
};

/// The lowercase extension of an entry name, or `NULL` when it has none.
const EXTENSION: &str = r"lower(substring(entry.name from '\.([^.]+)$'))";

/// Appends one filter expression as a parenthesized boolean SQL fragment.
pub(in crate::infrastructure::postgres) fn push_expression(
    builder: &mut QueryBuilder<Postgres>,
    expression: &FilterExpression,
) {
    match expression {
        FilterExpression::All(children) => push_group(builder, children, " AND "),
        FilterExpression::Any(children) => push_group(builder, children, " OR "),
        FilterExpression::Not(inner) => {
            builder.push("NOT (");
            push_expression(builder, inner);
            builder.push(")");
        }
        FilterExpression::Predicate(predicate) => push_predicate(builder, predicate),
    }
}

fn push_group(
    builder: &mut QueryBuilder<Postgres>,
    children: &[FilterExpression],
    separator: &str,
) {
    if children.is_empty() {
        builder.push("TRUE");
        return;
    }
    builder.push("(");
    for (index, child) in children.iter().enumerate() {
        if index > 0 {
            builder.push(separator);
        }
        push_expression(builder, child);
    }
    builder.push(")");
}

#[allow(clippy::too_many_lines)]
fn push_predicate(builder: &mut QueryBuilder<Postgres>, predicate: &FilterPredicate) {
    match predicate {
        FilterPredicate::ChangedAfter(day) => {
            builder.push("entry.updated_at >= ");
            push_day(builder, *day);
        }
        FilterPredicate::ChangedBefore(day) => {
            builder.push("entry.updated_at < ");
            push_day(builder, *day);
        }
        FilterPredicate::ChangedBetween(start, end) => {
            // Both ends are inclusive days, so the range reaches the final
            // instant of the closing day.
            builder.push("(entry.updated_at >= ");
            push_day(builder, *start);
            builder.push(" AND entry.updated_at < ");
            push_day(builder, *end);
            builder.push(" + interval '1 day')");
        }
        FilterPredicate::CreatedBy(selector) => {
            builder.push("(entry.created_by_id = ");
            builder.push_bind(selector.id.clone());
            push_optional_kind(builder, selector, "entry.created_by_type");
            builder.push(")");
        }
        FilterPredicate::SharedWith(selector) => push_shared_with(builder, selector),
        FilterPredicate::AccessibleTo(selector) => push_accessible_to(builder, selector),
        FilterPredicate::Contains(term) => {
            builder.push("(");
            push_name_match(builder, term);
            builder.push(" OR ");
            push_content_match(builder, term);
            builder.push(")");
        }
        FilterPredicate::HasContent(term) => push_content_match(builder, term),
        FilterPredicate::NameMatches(term) => push_name_match(builder, term),
        FilterPredicate::IsKind(kind) => {
            builder.push("entry.entry_type = ");
            builder.push_bind(match kind {
                EntryKind::File => "file",
                EntryKind::Folder => "folder",
            });
        }
        FilterPredicate::IsRender(kind) => push_render_match(builder, *kind),
        FilterPredicate::HasExtension(extension) => {
            builder.push("(entry.entry_type = 'file' AND ");
            builder.push(EXTENSION);
            builder.push(" = ");
            builder.push_bind(extension.clone());
            builder.push(")");
        }
        FilterPredicate::InLocation(term) => {
            // Paths are exact identifiers, so a location prefix stays
            // case-sensitive and keeps using the path index.
            builder.push("entry.path LIKE ");
            builder.push_bind(term.prefix_pattern());
            builder.push(r" ESCAPE '\'");
        }
        FilterPredicate::HasPermission(_) => {
            // Decided by domain policy once the candidate is authorized.
            builder.push("TRUE");
        }
    }
}

fn push_day(builder: &mut QueryBuilder<Postgres>, day: Date) {
    // Filter days are absolute calendar days in UTC, independent of the
    // session time zone.
    builder.push("((");
    builder.push_bind(day);
    builder.push(")::date AT TIME ZONE 'UTC')");
}

fn push_optional_kind(
    builder: &mut QueryBuilder<Postgres>,
    selector: &ActorSelector,
    column: &str,
) {
    if let Some(kind) = selector.kind {
        builder.push(" AND ");
        builder.push(column);
        builder.push(" = ");
        builder.push_bind(actor_kind(kind));
    }
}

fn push_name_match(builder: &mut QueryBuilder<Postgres>, term: &GlobTerm) {
    builder.push("entry.name ILIKE ");
    builder.push_bind(term.like_pattern());
    builder.push(r" ESCAPE '\'");
}

fn push_content_match(builder: &mut QueryBuilder<Postgres>, term: &GlobTerm) {
    builder.push(
        "EXISTS (SELECT 1 FROM briefcase.search_documents AS document \
                  WHERE document.org_id = entry.org_id \
                    AND document.entry_id = entry.entry_id \
                    AND document.extracted_content ILIKE ",
    );
    builder.push_bind(term.like_pattern());
    builder.push(r" ESCAPE '\')");
}

fn push_shared_with(builder: &mut QueryBuilder<Postgres>, selector: &ActorSelector) {
    builder.push(
        "EXISTS (SELECT 1 FROM briefcase.entry_closure AS shared_path \
                   JOIN briefcase.permission_grants AS shared_grant \
                     ON shared_grant.org_id = shared_path.org_id \
                    AND shared_grant.entry_id = shared_path.ancestor_id \
                  WHERE shared_path.org_id = entry.org_id \
                    AND shared_path.descendant_id = entry.entry_id \
                    AND shared_grant.revoked_at IS NULL \
                    AND (shared_path.depth = 0 OR shared_grant.inherits_to_descendants) \
                    AND shared_grant.principal_id = ",
    );
    builder.push_bind(selector.id.clone());
    push_optional_kind(builder, selector, "shared_grant.principal_type");
    builder.push(")");
}

fn push_accessible_to(builder: &mut QueryBuilder<Postgres>, selector: &ActorSelector) {
    // Reachability for another member is evaluated exactly like the caller's
    // own: ownership, the Public boundary, a matching tag, an administrative
    // role, or an explicit grant.
    builder.push(
        "EXISTS (SELECT 1 FROM briefcase.organization_members AS reader \
                  WHERE reader.org_id = entry.org_id \
                    AND reader.membership_status = 'active' \
                    AND reader.actor_id = ",
    );
    builder.push_bind(selector.id.clone());
    push_optional_kind(builder, selector, "reader.actor_type");
    builder.push(
        " AND ( \
             reader.org_role IN ('owner', 'admin') \
             OR (entry.owner_type = reader.actor_type AND entry.owner_id = reader.actor_id) \
             OR entry.root_type = 'public' \
             OR (entry.root_type = 'tag' AND EXISTS ( \
                    SELECT 1 FROM briefcase.organization_member_tags AS reader_tag \
                     WHERE reader_tag.org_id = entry.org_id \
                       AND reader_tag.actor_type = reader.actor_type \
                       AND reader_tag.actor_id = reader.actor_id \
                       AND reader_tag.tag_id = entry.tag_id)) \
             OR EXISTS ( \
                    SELECT 1 FROM briefcase.entry_closure AS reader_path \
                      JOIN briefcase.permission_grants AS reader_grant \
                        ON reader_grant.org_id = reader_path.org_id \
                       AND reader_grant.entry_id = reader_path.ancestor_id \
                     WHERE reader_path.org_id = entry.org_id \
                       AND reader_path.descendant_id = entry.entry_id \
                       AND reader_grant.principal_type = reader.actor_type \
                       AND reader_grant.principal_id = reader.actor_id \
                       AND reader_grant.revoked_at IS NULL \
                       AND (reader_path.depth = 0 OR reader_grant.inherits_to_descendants)) \
         ))",
    );
}

fn push_render_match(builder: &mut QueryBuilder<Postgres>, kind: RenderKind) {
    let known_extensions: Vec<String> = ALL_RENDER_KINDS
        .into_iter()
        .flat_map(RenderKind::extensions)
        .map(|extension| (*extension).to_owned())
        .collect();

    if kind == RenderKind::Unsupported {
        builder.push("(entry.entry_type = 'file' AND NOT (");
        push_extension_membership(builder, &known_extensions);
        builder.push(") AND NOT (");
        push_any_media_prefix(builder);
        builder.push("))");
        return;
    }

    let extensions: Vec<String> = kind
        .extensions()
        .iter()
        .map(|extension| (*extension).to_owned())
        .collect();
    // A known extension wins, exactly as the domain classifier decides; the
    // media type only speaks for a name with no recognized extension.
    builder.push("(entry.entry_type = 'file' AND (");
    push_extension_membership(builder, &extensions);
    builder.push(" OR (NOT (");
    push_extension_membership(builder, &known_extensions);
    builder.push(") AND ");
    push_media_prefixes(builder, kind.media_type_prefixes());
    builder.push(")))");
}

fn push_extension_membership(builder: &mut QueryBuilder<Postgres>, extensions: &[String]) {
    builder.push("COALESCE(");
    builder.push(EXTENSION);
    builder.push(" = ANY(");
    builder.push_bind(extensions.to_vec());
    builder.push("), false)");
}

/// The media-type prefixes are Briefcase constants, so they carry no `LIKE`
/// metacharacters and need no escape clause.
fn push_media_prefixes(builder: &mut QueryBuilder<Postgres>, prefixes: &[&str]) {
    if prefixes.is_empty() {
        builder.push("false");
        return;
    }
    let patterns: Vec<String> = prefixes
        .iter()
        .map(|prefix| format!("{prefix}%"))
        .collect();
    builder.push("COALESCE(entry.content_type ILIKE ANY(");
    builder.push_bind(patterns);
    builder.push("), false)");
}

fn push_any_media_prefix(builder: &mut QueryBuilder<Postgres>) {
    let patterns: Vec<&str> = ALL_RENDER_KINDS
        .into_iter()
        .flat_map(RenderKind::media_type_prefixes)
        .copied()
        .collect();
    push_media_prefixes(builder, &patterns);
}

const fn actor_kind(kind: ActorKind) -> &'static str {
    match kind {
        ActorKind::Carbon => "carbon",
        ActorKind::Silicon => "silicon",
    }
}
