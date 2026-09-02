//! Executable checks for the published `OpenAPI` operation inventory.

use std::collections::BTreeSet;

use serde_yaml::{Mapping, Value};

const EXPECTED_OPERATIONS: [&str; 24] = [
    "abortMultipartUpload",
    "completeMultipartUpload",
    "configureOrganizationBucket",
    "createFolder",
    "decideAccessRequest",
    "downloadEntry",
    "getEntry",
    "grantPermission",
    "initiateMultipartUpload",
    "listBin",
    "listEntries",
    "listPermissions",
    "listVersions",
    "moveEntryToBin",
    "readEntryContent",
    "requestAccess",
    "resolvePermanentUrl",
    "restoreEntry",
    "restoreVersion",
    "revokePermission",
    "searchFiles",
    "updateEntry",
    "uploadFile",
    "uploadPart",
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
