//! Durable, protocol-neutral Audit Log collaboration contract.

include!("generated.rs");

/// Envelope key for a legacy metadata value whose top-level JSON shape was not an object.
pub const LEGACY_METADATA_VALUE_KEY: &str = "_lenso_legacy_value";

/// Envelope key for legacy metadata serialized as text to preserve a non-portable JSON number.
pub const LEGACY_METADATA_PORTABLE_JSON_KEY: &str = "_lenso_legacy_portable_json";

/// Recovers metadata placed in one of the legacy compatibility envelopes.
///
/// `None` means the map is ordinary object-shaped metadata and should be used as-is. The
/// portable-JSON envelope intentionally uses ordinary JSON decoding here: its purpose is to let
/// a caller recover an historical value that cannot itself cross the portable wire as a number.
pub fn recover_legacy_metadata(
    metadata: &std::collections::BTreeMap<String, serde_json::Value>,
) -> Result<Option<serde_json::Value>, serde_json::Error> {
    if metadata.len() != 1 {
        return Ok(None);
    }
    if let Some(value) = metadata.get(LEGACY_METADATA_VALUE_KEY) {
        return Ok(Some(value.clone()));
    }
    let Some(encoded) = metadata
        .get(LEGACY_METADATA_PORTABLE_JSON_KEY)
        .and_then(serde_json::Value::as_str)
    else {
        return Ok(None);
    };
    serde_json::from_str(encoded).map(Some)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde_json::{Value, json};

    use super::{
        AppendEventError, LEGACY_METADATA_PORTABLE_JSON_KEY, LEGACY_METADATA_VALUE_KEY,
        decode_append_event_error, encode_append_event_error, recover_legacy_metadata,
    };

    #[test]
    fn generated_binding_preserves_unknown_domain_error_shape() {
        let wire =
            r#"{"code":"retention_locked","payload":{"until":"2030-01-01"},"retryable":false}"#;
        let decoded = decode_append_event_error(wire).expect("future Domain Error should decode");
        let AppendEventError::Unknown(error) = &decoded else {
            panic!("unknown Domain code must remain unknown");
        };
        assert_eq!(error.code, "retention_locked");
        assert_eq!(error.payload.as_ref().unwrap()["until"], "2030-01-01");
        assert_eq!(error.extra["retryable"], false);

        let reencoded = encode_append_event_error(&decoded).expect("unknown error should encode");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&reencoded).unwrap(),
            serde_json::from_str::<serde_json::Value>(wire).unwrap()
        );
    }

    #[test]
    fn legacy_metadata_envelopes_are_explicitly_recoverable() {
        let value = json!(["one", 2]);
        assert_eq!(
            recover_legacy_metadata(&BTreeMap::from([(
                LEGACY_METADATA_VALUE_KEY.to_owned(),
                value.clone(),
            )]))
            .unwrap(),
            Some(value)
        );

        let unsafe_number = json!({"nested": [9_007_199_254_740_992_u64]});
        assert_eq!(
            recover_legacy_metadata(&BTreeMap::from([(
                LEGACY_METADATA_PORTABLE_JSON_KEY.to_owned(),
                serde_json::Value::String(serde_json::to_string(&unsafe_number).unwrap()),
            )]))
            .unwrap(),
            Some(unsafe_number)
        );
    }

    #[test]
    fn every_recursive_wire_shape_has_an_explicit_structural_bound() {
        for (name, source) in [
            (
                "append-event-request",
                include_str!("../schemas/append-event-request.schema.json"),
            ),
            (
                "append-event-response",
                include_str!("../schemas/append-event-response.schema.json"),
            ),
            (
                "audit-event",
                include_str!("../schemas/audit-event.schema.json"),
            ),
            (
                "get-event-request",
                include_str!("../schemas/get-event-request.schema.json"),
            ),
            (
                "get-event-response",
                include_str!("../schemas/get-event-response.schema.json"),
            ),
            (
                "list-events-request",
                include_str!("../schemas/list-events-request.schema.json"),
            ),
            (
                "list-events-response",
                include_str!("../schemas/list-events-response.schema.json"),
            ),
        ] {
            let schema: Value = serde_json::from_str(source).unwrap();
            assert_schema_bounds(&schema, name);
        }

        for source in [
            include_str!("../schemas/append-event-request.schema.json"),
            include_str!("../schemas/audit-event.schema.json"),
        ] {
            let schema: Value = serde_json::from_str(source).unwrap();
            let metadata = &schema["properties"]["metadata"];
            assert_eq!(metadata["maxProperties"], 1_024);
            assert_eq!(metadata["propertyNames"]["maxLength"], 256);
            assert_eq!(metadata["x-lenso-max-container-items"], 1_024);
            assert_eq!(metadata["x-lenso-max-depth"], 32);
            assert_eq!(metadata["x-lenso-max-encoded-bytes"], 65_536);
            assert_eq!(metadata["x-lenso-max-nodes"], 16_384);
            assert_eq!(metadata["x-lenso-max-string-length"], 65_536);
        }
    }

    fn assert_schema_bounds(schema: &Value, path: &str) {
        let Some(object) = schema.as_object() else {
            return;
        };
        let includes_type = |expected: &str| match object.get("type") {
            Some(Value::String(actual)) => actual == expected,
            Some(Value::Array(types)) => types.iter().any(|actual| actual == expected),
            _ => false,
        };
        if includes_type("string") && !object.contains_key("const") && !object.contains_key("enum")
        {
            assert!(
                object.get("maxLength").and_then(Value::as_u64).is_some(),
                "unbounded string Schema at {path}"
            );
        }
        if includes_type("array") {
            assert!(
                object.get("maxItems").and_then(Value::as_u64).is_some(),
                "unbounded array Schema at {path}"
            );
        }
        if includes_type("object") && object.get("additionalProperties") == Some(&Value::Bool(true))
        {
            assert!(
                object
                    .get("maxProperties")
                    .and_then(Value::as_u64)
                    .is_some(),
                "unbounded open object Schema at {path}"
            );
        }
        for (key, child) in object {
            match child {
                Value::Object(_) => assert_schema_bounds(child, &format!("{path}.{key}")),
                Value::Array(values) => {
                    for (index, value) in values.iter().enumerate() {
                        assert_schema_bounds(value, &format!("{path}.{key}[{index}]"));
                    }
                }
                _ => {}
            }
        }
    }
}
