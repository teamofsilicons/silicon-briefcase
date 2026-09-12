//! Executable checks for the published `OpenAPI` operation inventory.

use std::collections::BTreeSet;

use serde_yaml::{Mapping, Value};

const EXPECTED_OPERATIONS: [&str; 58] = [
    "cancelDelegatedUpload",
    "cleanCurrentTestingEnvironment",
    "cleanTestingEnvironment",
    "commitDelegatedUpload",
    "configureOrganizationBucket",
    "createFileOnBehalfOfMember",
    "createFolder",
    "createFolderOnBehalfOfMember",
    "createTestingEnvironment",
    "deleteTestingEnvironment",
    "describeCurrentTestingEnvironment",
    "downloadEntry",
    "exchangeShortLivedToken",
    "getDelegatedUploadStatus",
    "getEntry",
    "getTestingEnvironment",
    "getTestingEnvironmentKey",
    "grantPermission",
    "inspectEffectivePermissions",
    "listBin",
    "listEntryActivity",
    "listEntries",
    "listEntriesOnBehalfOfMember",
    "listNotifications",
    "listPermissions",
    "listTestingEnvironments",
    "listVersions",
    "moveEntryToBin",
    "readApiVersion",
    "readIamInfo",
    "readLoginStatus",
    "readEntryContent",
    "readFileOnBehalfOfMember",
    "readOrganizationUsage",
    "readNotifications",
    "refreshApplicationSession",
    "replaceTestingEnvironmentIamPairing",
    "reserveDelegatedUpload",
    "resolvePermanentUrl",
    "restoreEntry",
    "restoreTestingEnvironment",
    "restoreVersion",
    "revokePermission",
    "searchFiles",
    "transferDelegatedUpload",
    "trashEntryOnBehalfOfMember",
    "updateEntry",
    "updateTestingEnvironment",
    "uploadFile",
    "listInvitations",
    "createInvitation",
    "revokeInvitation",
    "readLinkAccess",
    "setLinkAccess",
    "listEntryLogs",
    "readPublicEntry",
    "inviteOnBehalfOfMember",
    "setLinkAccessOnBehalfOfMember",
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
fn the_served_operation_registry_matches_the_published_contract() {
    let registry: BTreeSet<&str> = silicon_briefcase::api::versioning::OPERATIONS
        .iter()
        .map(|operation| operation.id)
        .collect();
    let published = EXPECTED_OPERATIONS.into_iter().collect::<BTreeSet<_>>();
    assert_eq!(registry, published);
}

#[test]
fn testing_environment_listing_and_root_self_cache_contract_are_exact() -> anyhow::Result<()> {
    let document = serde_yaml::from_str::<Value>(include_str!("../openapi.yaml"))?;
    assert!(
        document["components"]["schemas"]["TestingEnvironmentPage"]["properties"]["items"]
            ["maxItems"]
            .is_null(),
        "retained deleted environments make the result larger than the active limit"
    );
    assert_eq!(
        document["paths"]["/testing-environment"]["get"]["responses"]["200"]["headers"]
            ["Cache-Control"]["schema"]["const"]
            .as_str(),
        Some("private, no-store")
    );
    Ok(())
}

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
            if let Some(id) = operation["operationId"].as_str() {
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
