//! Compilation of parsed filters into tenant-safe SQL.
//!
//! Every user-supplied value is bound as a parameter; nothing from the filter
//! text is ever concatenated into the statement. Permission predicates are
//! decided by domain policy because effective access is not a column. For a
//! mixed expression, persistence projects each database predicate as a
//! non-null boolean so policy can evaluate the original tree exactly.

use sqlx::{Postgres, QueryBuilder};
use time::Date;

use crate::domain::{
    actor::ActorKind,
    entry::EntryKind,
    filter::{ActorSelector, FilterExpression, FilterPredicate, GlobTerm},
    media::{ALL_RENDER_KINDS, RenderKind},
};

use super::common::{OwnedAncestorPrincipal, push_owned_ancestor_access};

/// The lowercase extension of an entry name, or `NULL` when it has none.
const EXTENSION: &str = r"lower(substring(entry.name from '\.([^.]+)$'))";

/// Who is filtering. Only `is:expiring` depends on it: an expiring share matches when it
/// gave the caller their access or when the caller manages it.
#[derive(Clone, Copy, Debug)]
pub(in crate::infrastructure::postgres) struct FilterCaller<'a> {
    pub kind: &'static str,
    pub id: &'a str,
    pub administrator: bool,
}

/// Appends one filter expression as a parenthesized boolean SQL fragment.
pub(in crate::infrastructure::postgres) fn push_expression(
    builder: &mut QueryBuilder<Postgres>,
    expression: &FilterExpression,
    caller: FilterCaller<'_>,
) {
    debug_assert!(
        !expression.requires_policy_evaluation(),
        "permission predicates require service-side exact evaluation"
    );
    match expression {
        FilterExpression::All(children) => push_group(builder, children, " AND ", caller),
        FilterExpression::Any(children) => push_group(builder, children, " OR ", caller),
        FilterExpression::Not(inner) => {
            builder.push("NOT (");
            push_expression(builder, inner, caller);
            builder.push(")");
        }
        FilterExpression::Predicate(predicate) => push_predicate(builder, predicate, caller),
    }
}

/// Appends a boolean array containing every database-backed predicate result
/// in depth-first source order.
///
/// When an expression mixes persistence facts with effective permissions, the
/// complete expression cannot be weakened independently in SQL and Rust: `or`
/// and `not` would change meaning. Persistence therefore returns its atomic
/// truth values and the service evaluates the original tree once domain policy
/// has supplied the permission truth values.
pub(in crate::infrastructure::postgres) fn push_database_predicate_matches(
    builder: &mut QueryBuilder<Postgres>,
    expression: &FilterExpression,
    caller: FilterCaller<'_>,
) {
    builder.push("ARRAY[");
    let mut count = 0_usize;
    push_database_predicates(builder, expression, &mut count, caller);
    builder.push("]::boolean[]");
}

fn push_database_predicates(
    builder: &mut QueryBuilder<Postgres>,
    expression: &FilterExpression,
    count: &mut usize,
    caller: FilterCaller<'_>,
) {
    match expression {
        FilterExpression::Predicate(FilterPredicate::HasPermission(_)) => {}
        FilterExpression::Predicate(predicate) => {
            if *count > 0 {
                builder.push(", ");
            }
            // SQL WHERE treats UNKNOWN like false. Preserve that behavior in
            // the projected array and keep nullable facts (for example, an
            // extensionless file's suffix) decodable as `Vec<bool>`.
            builder.push("COALESCE((");
            push_predicate(builder, predicate, caller);
            builder.push("), FALSE)");
            *count += 1;
        }
        FilterExpression::Not(inner) => push_database_predicates(builder, inner, count, caller),
        FilterExpression::All(children) | FilterExpression::Any(children) => {
            for child in children {
                push_database_predicates(builder, child, count, caller);
            }
        }
    }
}

fn push_group(
    builder: &mut QueryBuilder<Postgres>,
    children: &[FilterExpression],
    separator: &str,
    caller: FilterCaller<'_>,
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
        push_expression(builder, child, caller);
    }
    builder.push(")");
}

#[allow(clippy::too_many_lines)]
fn push_predicate(
    builder: &mut QueryBuilder<Postgres>,
    predicate: &FilterPredicate,
    caller: FilterCaller<'_>,
) {
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
        FilterPredicate::IsExpiring => push_expiring_match(builder, caller),
        FilterPredicate::IsSelfDestruct => {
            builder.push(
                "(entry.entry_type = 'file' AND entry.self_destruct_at IS NOT NULL \
                  AND entry.self_destruct_at > clock_timestamp())",
            );
        }
        FilterPredicate::HasPermission(_) => {
            unreachable!("permission predicates are evaluated by domain policy")
        }
    }
}

/// `is:expiring`: a live expiring share that either gave the caller their access (on
/// this entry or inherited from a folder above it), or sits on this entry and
/// is the caller's to manage — they created it, own the entry, or administer
/// the organization. The effective-grant view already drops expired shares.
fn push_expiring_match(builder: &mut QueryBuilder<Postgres>, caller: FilterCaller<'_>) {
    builder.push(
        "(EXISTS (SELECT 1 FROM briefcase.entry_closure AS expiring_path \
                   JOIN briefcase.effective_permission_grants AS expiring_grant \
                     ON expiring_grant.org_id = expiring_path.org_id \
                    AND expiring_grant.entry_id = expiring_path.ancestor_id \
                  WHERE expiring_path.org_id = entry.org_id \
                    AND expiring_path.descendant_id = entry.entry_id \
                    AND expiring_grant.expires_at IS NOT NULL \
                    AND expiring_grant.revoked_at IS NULL \
                    AND (expiring_path.depth = 0 OR expiring_grant.inherits_to_descendants) \
                    AND expiring_grant.principal_type = ",
    );
    builder.push_bind(caller.kind);
    builder.push(" AND expiring_grant.principal_id = ");
    builder.push_bind(caller.id.to_owned());
    builder.push(") OR ((entry.owner_type = ");
    builder.push_bind(caller.kind);
    builder.push(" AND entry.owner_id = ");
    builder.push_bind(caller.id.to_owned());
    builder.push(") OR ");
    builder.push_bind(caller.administrator);
    builder.push(
        ") AND (entry.link_expires_at > clock_timestamp() \
               OR EXISTS (SELECT 1 FROM briefcase.permission_grants AS own_expiring \
                           WHERE own_expiring.org_id = entry.org_id AND own_expiring.entry_id = entry.entry_id \
                             AND own_expiring.revoked_at IS NULL AND own_expiring.expires_at > clock_timestamp()) \
               OR EXISTS (SELECT 1 FROM briefcase.tag_permission_grants AS own_tag_expiring \
                           WHERE own_tag_expiring.org_id = entry.org_id AND own_tag_expiring.entry_id = entry.entry_id \
                             AND own_tag_expiring.revoked_at IS NULL AND own_tag_expiring.expires_at > clock_timestamp())) \
          OR EXISTS (SELECT 1 FROM briefcase.permission_grants AS granted_expiring \
                      WHERE granted_expiring.org_id = entry.org_id AND granted_expiring.entry_id = entry.entry_id \
                        AND granted_expiring.revoked_at IS NULL AND granted_expiring.expires_at > clock_timestamp() \
                        AND granted_expiring.granted_by_type = ",
    );
    builder.push_bind(caller.kind);
    builder.push(" AND granted_expiring.granted_by_id = ");
    builder.push_bind(caller.id.to_owned());
    builder.push(
        ") OR EXISTS (SELECT 1 FROM briefcase.tag_permission_grants AS granted_tag_expiring \
                      WHERE granted_tag_expiring.org_id = entry.org_id AND granted_tag_expiring.entry_id = entry.entry_id \
                        AND granted_tag_expiring.revoked_at IS NULL AND granted_tag_expiring.expires_at > clock_timestamp() \
                        AND granted_tag_expiring.granted_by_type = ",
    );
    builder.push_bind(caller.kind);
    builder.push(" AND granted_tag_expiring.granted_by_id = ");
    builder.push_bind(caller.id.to_owned());
    builder.push("))");
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
                   JOIN briefcase.effective_permission_grants AS shared_grant \
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
    // own: ownership (including an owned containing folder), the Public
    // boundary, a matching tag, an administrative role, or an explicit grant.
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
                      JOIN briefcase.effective_permission_grants AS reader_grant \
                        ON reader_grant.org_id = reader_path.org_id \
                       AND reader_grant.entry_id = reader_path.ancestor_id \
                     WHERE reader_path.org_id = entry.org_id \
                       AND reader_path.descendant_id = entry.entry_id \
                       AND reader_grant.principal_type = reader.actor_type \
                       AND reader_grant.principal_id = reader.actor_id \
                       AND reader_grant.revoked_at IS NULL \
                       AND (reader_path.depth = 0 OR reader_grant.inherits_to_descendants)) \
             OR ",
    );
    push_owned_ancestor_access(builder, OwnedAncestorPrincipal::Reader);
    builder.push("))");
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
    let patterns: Vec<String> = prefixes.iter().map(|prefix| format!("{prefix}%")).collect();
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
