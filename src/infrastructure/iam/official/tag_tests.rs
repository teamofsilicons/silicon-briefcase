//! Optional disclosure never substitutes cached tags or weakens independent ACLs.
use serde_json::{Value, json};

use super::authorization;
use crate::domain::{
    actor::{
        ActorId, ActorKind, ActorRef, ApplicationId, AuthenticationMode, OrganizationId, TagName,
    },
    entry::{EntryBoundary, EntryKind},
    ids::{EntryId, GrantId},
    permission::{
        Capability, EffectiveAuthorizationInput, GrantApplication, GrantedAccess, PermissionGrant,
        PermissionGrantParts, PermissionInheritance, evaluate_authorization,
    },
};

fn snapshot(
    tags: &Value,
    disclosed: bool,
) -> anyhow::Result<super::super::canonical::LocalAuthorization> {
    let mut scopes = vec!["self.identity.read", "self.membership.read"];
    if disclosed {
        scopes.push("self.tags.read");
    }
    Ok(serde_json::from_value(json!({
        "principal_id":"01990a9d-86f1-7000-8000-000000000001",
        "organization_id":"01990a9d-86f1-7000-8000-000000000002",
        "membership_id":"01990a9d-86f1-7000-8000-000000000003",
        "actor_type":"carbon","public_id":"member-a","org_id":"client-org",
        "membership_version":7,"authorization_epoch":7,"audience":"tos>briefcase",
        "testing_environment_id":null,"org_role":"member","scopes":scopes,"tags":tags
    }))?)
}

#[test]
fn production_authorization_preserves_unknown_and_explicit_empty_tags() -> anyhow::Result<()> {
    let app = ApplicationId::new("tos>briefcase")?;
    let org = OrganizationId::new("client-org")?;
    let unknown = authorization(
        snapshot(&Value::Null, false)?,
        &app,
        &org,
        None,
        AuthenticationMode::Bearer,
    )?;
    assert!(unknown.tags().is_none());
    assert!(
        unknown
            .iam_binding()
            .is_some_and(|binding| binding.tags.is_none())
    );
    assert!(!unknown.has_tag(&TagName::new("finance")?));
    let empty = authorization(
        snapshot(&json!([]), true)?,
        &app,
        &org,
        None,
        AuthenticationMode::Bearer,
    )?;
    assert!(
        empty
            .tags()
            .is_some_and(std::collections::BTreeSet::is_empty)
    );
    assert!(
        empty
            .iam_binding()
            .is_some_and(|binding| binding.tags.as_ref().is_some_and(Vec::is_empty))
    );
    Ok(())
}

#[test]
fn scope_disclosure_inconsistency_and_missing_role_remain_rejected() -> anyhow::Result<()> {
    let app = ApplicationId::new("tos>briefcase")?;
    let org = OrganizationId::new("client-org")?;
    for mismatched in [snapshot(&Value::Null, true)?, snapshot(&json!([]), false)?] {
        assert!(authorization(mismatched, &app, &org, None, AuthenticationMode::Bearer).is_err());
    }
    let mut missing_role = snapshot(&Value::Null, false)?;
    missing_role.org_role = None;
    assert!(authorization(missing_role, &app, &org, None, AuthenticationMode::Bearer).is_err());
    let mut missing_identity = snapshot(&Value::Null, false)?;
    missing_identity.public_id = None;
    assert!(
        authorization(
            missing_identity,
            &app,
            &org,
            None,
            AuthenticationMode::Bearer
        )
        .is_err()
    );
    Ok(())
}

#[test]
fn undisclosed_tags_do_not_block_explicit_actor_grants_or_ownership() -> anyhow::Result<()> {
    let app = ApplicationId::new("tos>briefcase")?;
    let org = OrganizationId::new("client-org")?;
    let context = authorization(
        snapshot(&Value::Null, false)?,
        &app,
        &org,
        None,
        AuthenticationMode::Bearer,
    )?;
    let peer = ActorRef::new(ActorKind::Carbon, ActorId::new("peer")?);
    let entry_id = EntryId::new();
    let boundary = EntryBoundary::Tag {
        tag: TagName::new("finance")?,
    };
    let input = EffectiveAuthorizationInput {
        context: &context,
        entry_organization_id: &org,
        entry_id,
        entry_kind: EntryKind::File,
        system_kind: None,
        boundary: &boundary,
        owner: &peer,
        origin_application_id: None,
        grants: &[],
        owns_ancestor: false,
        required_for_traversal: false,
    };
    assert!(!evaluate_authorization(&input).allows(Capability::Read));
    let grant = PermissionGrant::from_parts(PermissionGrantParts {
        id: GrantId::new(),
        organization_id: org.clone(),
        entry_id,
        principal: context.actor().clone(),
        access: GrantedAccess::READ_ONLY,
        inheritance: PermissionInheritance::EntryOnly,
        granted_by: peer.clone(),
        created_at: time::OffsetDateTime::UNIX_EPOCH,
    });
    let grants = [GrantApplication::Direct(&grant)];
    let explicit = evaluate_authorization(&EffectiveAuthorizationInput {
        grants: &grants,
        ..input
    });
    assert!(explicit.allows(Capability::Read));
    assert!(!explicit.allows(Capability::Delete));
    let owned = evaluate_authorization(&EffectiveAuthorizationInput {
        owner: context.actor(),
        ..input
    });
    assert!(owned.allows(Capability::Read));
    assert!(owned.allows(Capability::Delete));
    Ok(())
}
