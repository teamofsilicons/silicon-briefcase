//! Executable checks for the published `OpenAPI` operation inventory.

use std::collections::BTreeSet;

use serde_yaml::{Mapping, Value};

const EXPECTED_OPERATIONS: [&str; 57] = [
    "submitReport",
    "submitTelemetry",
    "cancelDelegatedUpload",
    "commitDelegatedUpload",
    "configureOrganizationBucket",
    "createFileOnBehalfOfMember",
    "createFolder",
    "createFolderOnBehalfOfMember",
    "downloadEntry",
    "getDelegatedUploadStatus",
    "getEntry",
    "grantPermission",
    "inspectEffectivePermissions",
    "listBin",
    "listEntryActivity",
    "listEntries",
    "listEntriesOnBehalfOfMember",
    "listNotifications",
    "listPermissions",
    "listVersions",
    "moveEntryToBin",
    "readAccount",
    "readAccountsInfo",
    "readApiVersion",
    "readEntryContent",
    "readFileOnBehalfOfMember",
    "readOrganizationUsage",
    "readNotifications",
    "reserveDelegatedUpload",
    "resolvePermanentUrl",
    "resolveDriveTop",
    "resolveLegacyPermanentUrl",
    "listArchiveContents",
    "restoreEntry",
    "restoreVersion",
    "revokePermission",
    "searchFiles",
    "transferDelegatedUpload",
    "trashEntryOnBehalfOfMember",
    "updateEntry",
    "uploadFile",
    "listInvitations",
    "createInvitation",
    "revokeInvitation",
    "changeExpiringShare",
    "makeEntryPermanent",
    "readLinkAccess",
    "setLinkAccess",
    "listEntryLogs",
    "readPublicEntry",
    "inviteOnBehalfOfMember",
    "setLinkAccessOnBehalfOfMember",
    "listDrives",
    "listSharedWithMe",
    "listSiliconAllowedAccounts",
    "allowAccountToShareWithSilicon",
    "disallowAccountToShareWithSilicon",
];

#[test]
fn openapi_contract_is_parseable_and_operation_ids_are_stable() -> anyhow::Result<()> {
    let document = serde_yaml::from_str::<Value>(include_str!("../openapi.yaml"))?;
    let root = mapping(&document, "document root")?;
    let version = string_field(root, "openapi")?;
    assert_eq!(version, "3.1.0");

    let paths = mapping_field(root, "paths")?;
    let mut actual = BTreeSet::new();
    for path_item in paths.values() {
        let operations = mapping(path_item, "path item")?;
        for (method, operation) in operations {
            let Some(method) = method.as_str() else {
                continue;
            };
            if !matches!(method, "get" | "post" | "put" | "patch" | "delete") {
                continue;
            }
            let operation = mapping(operation, "operation")?;
            actual.insert(string_field(operation, "operationId")?.to_owned());
        }
    }

    let expected = EXPECTED_OPERATIONS
        .into_iter()
        .map(str::to_owned)
        .collect::<BTreeSet<_>>();
    assert_eq!(actual, expected);
    Ok(())
}

fn mapping<'a>(value: &'a Value, context: &str) -> anyhow::Result<&'a Mapping> {
    value
        .as_mapping()
        .ok_or_else(|| anyhow::anyhow!("{context} must be a mapping"))
}

fn mapping_field<'a>(mapping: &'a Mapping, field: &str) -> anyhow::Result<&'a Mapping> {
    let value = mapping
        .get(Value::String(field.to_owned()))
        .ok_or_else(|| anyhow::anyhow!("missing {field}"))?;
    self::mapping(value, field)
}

fn string_field<'a>(mapping: &'a Mapping, field: &str) -> anyhow::Result<&'a str> {
    mapping
        .get(Value::String(field.to_owned()))
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("{field} must be a string"))
}

/// The version document must describe exactly the operations that exist.
///
/// A client checks its own operations against the registry the backend serves,
/// so an operation published in the contract but missing from the registry
/// would be unverifiable, and one in the registry but not the contract would
/// promise a revision for something a client cannot call.
#[test]
fn the_served_operation_registry_matches_the_published_contract() -> anyhow::Result<()> {
    let registry: BTreeSet<&str> = silicon_briefcase::api::versioning::OPERATIONS
        .iter()
        .map(|operation| operation.id)
        .collect();
    let published = EXPECTED_OPERATIONS.into_iter().collect::<BTreeSet<_>>();
    assert_eq!(registry, published);

    // Each operation's method, path and revision agree too, so a client that
    // trusts the served registry calls exactly what the document describes.
    let document = serde_yaml::from_str::<Value>(include_str!("../openapi.yaml"))?;
    let mut documented = BTreeSet::new();
    for (path, item) in mapping_field(mapping(&document, "document root")?, "paths")? {
        for (method, operation) in mapping(item, "path item")? {
            let Some(id) = operation.get("operationId").and_then(Value::as_str) else {
                continue;
            };
            documented.insert((
                id.to_owned(),
                method.as_str().unwrap_or_default().to_uppercase(),
                path.as_str().unwrap_or_default().to_owned(),
                operation
                    .get("x-operation-version")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
            ));
        }
    }
    let served: BTreeSet<_> = silicon_briefcase::api::versioning::OPERATIONS
        .iter()
        .map(|operation| {
            (
                operation.id.to_owned(),
                operation.method.to_owned(),
                operation.path.to_owned(),
                operation.version.to_owned(),
            )
        })
        .collect();
    assert_eq!(served, documented);
    Ok(())
}

#[test]
fn the_contract_is_version_four_with_accounts_security() -> anyhow::Result<()> {
    let document = serde_yaml::from_str::<Value>(include_str!("../openapi.yaml"))?;
    assert_eq!(document["info"]["version"].as_str(), Some("4.1.0"));
    assert_eq!(
        document["info"]["version"].as_str(),
        Some(silicon_briefcase::api::versioning::CONTRACT_VERSION)
    );
    let schemes = &document["components"]["securitySchemes"];
    assert_eq!(schemes["bearerAuth"]["scheme"].as_str(), Some("bearer"));
    assert_eq!(schemes["proofAuth"]["scheme"].as_str(), Some("Proof"));
    assert!(document["paths"]["/testing-environment"].is_null());
    assert!(document["paths"]["/auth/slt"].is_null());
    Ok(())
}

/// Operations the service still serves for older clients but the official
/// client no longer calls. Each answers 410 and records nothing.
const RETIRED_FOR_CLIENTS: [&str; 1] = ["submitTelemetry"];

#[test]
fn official_client_consumes_the_exact_published_operations() -> anyhow::Result<()> {
    let document: Value = serde_yaml::from_str(include_str!("../openapi.yaml"))?;
    let mut actual = BTreeSet::new();
    for (path, item) in document["paths"]
        .as_mapping()
        .ok_or_else(|| anyhow::anyhow!("missing paths"))?
    {
        for (method, operation) in item
            .as_mapping()
            .ok_or_else(|| anyhow::anyhow!("missing path item"))?
        {
            if let Some(id) = operation["operationId"]
                .as_str()
                .filter(|id| !RETIRED_FOR_CLIENTS.contains(id))
            {
                actual.insert((
                    id.to_owned(),
                    method.as_str().unwrap_or_default().to_uppercase(),
                    path.as_str().unwrap_or_default().to_owned(),
                    operation["x-operation-version"]
                        .as_str()
                        .unwrap_or_default()
                        .to_owned(),
                ));
            }
        }
    }
    let consumers: BTreeSet<_> = briefcase_client::OPERATIONS
        .iter()
        .map(|op| {
            (
                op.id.to_owned(),
                op.method.to_owned(),
                op.path.to_owned(),
                op.version.to_owned(),
            )
        })
        .collect();
    assert_eq!(
        actual, consumers,
        "SDK consumers must agree on each method, path, and operation revision"
    );
    Ok(())
}
