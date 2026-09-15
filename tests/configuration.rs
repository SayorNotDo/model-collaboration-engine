use model_collaboration_engine::configuration::{load, parse};
use serde_json::{json, Value};

fn document() -> Value {
    let old: Value = serde_json::from_str(include_str!("../examples/config.json")).unwrap();
    let mut model = old["models"][0].clone();
    for name in ["base_url", "api_key_env", "provider", "region", "local"] {
        model.as_object_mut().unwrap().remove(name);
    }
    json!({
        "schema_version":1,
        "storage":{"database_path":"ledger.db"},
        "providers":[{"id":"gateway","kind":"relay","base_url":"https://relay.example/v1",
            "auth":{"type":"bearer","env":"RELAY_API_KEY"},"endpoints":["chat_completions"],
            "region":"test-region","local":false,"models":[model]}],
        "routing":{"planner_models":["local"],"weights":old["weights"]},
        "runtime":{"max_concurrency":4,"event_capacity":64,"event_max_bytes":8192,
            "stream_frame_max_bytes":65536,"context_max_bytes":1048576,
            "close_grace_ms":1000,"cleanup_timeout_ms":1000}
    })
}

#[test]
fn shared_gateway_resolves_aliases_and_actual_service_identity() {
    let dir = tempfile::tempdir().unwrap();
    let mut doc = document();
    let mut second = doc["providers"][0]["models"][0].clone();
    second["id"] = json!("second");
    second["model"] = json!("upstream/alias");
    doc["providers"][0]["models"]
        .as_array_mut()
        .unwrap()
        .push(second);
    let resolved = parse(&doc.to_string(), dir.path()).unwrap();
    let config = resolved.as_config();
    assert_eq!(config.models.len(), 2);
    assert_eq!(config.models[1].model, "upstream/alias");
    for model in &config.models {
        assert_eq!(model.provider, "gateway");
        assert_eq!(model.base_url, "https://relay.example/v1");
        assert_eq!(model.api_key_env, "RELAY_API_KEY");
        assert!(!model.local);
        assert_eq!(model.region, "test-region");
    }
    assert_eq!(
        std::path::Path::new(&config.database_path),
        dir.path().join("ledger.db")
    );
    assert!(!dir.path().join("ledger.db").exists());
}

#[test]
fn loading_uses_file_directory_without_opening_database_or_credentials() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("engine.json");
    std::fs::write(&path, document().to_string()).unwrap();
    let config = load(&path).unwrap().into_config();
    assert_eq!(
        std::path::Path::new(&config.database_path),
        dir.path().join("ledger.db")
    );
    assert!(!dir.path().join("ledger.db").exists());
}

#[test]
fn configuration_rejects_invalid_references_protocols_and_values_with_paths() {
    let cases = [
        (
            "/providers/0/models/0/context_tokens",
            json!(0),
            "providers[0].models[0]",
        ),
        ("/schema_version", json!(2), "schema_version"),
        (
            "/providers/0/models/0/endpoint",
            json!("responses"),
            "providers[0].models[0].endpoint",
        ),
        (
            "/providers/0/auth/type",
            json!("password"),
            "providers[0].auth.type",
        ),
        (
            "/providers/0/auth/env",
            json!("BAD NAME"),
            "providers[0].auth.env",
        ),
        (
            "/providers/0/base_url",
            json!("https://relay.example/?key=secret"),
            "providers[0].base_url",
        ),
        (
            "/storage/database_path",
            json!(":memory:"),
            "storage.database_path",
        ),
        ("/runtime/max_concurrency", json!(0), "runtime"),
    ];
    for (pointer, value, field) in cases {
        let mut doc = document();
        *doc.pointer_mut(pointer).unwrap() = value;
        let error = parse(&doc.to_string(), ".").err().unwrap();
        assert_eq!(error.kind, "configuration");
        assert!(
            error.details["field"].as_str().unwrap().starts_with(field),
            "{error:?}"
        );
        assert!(!serde_json::to_string(&error)
            .unwrap()
            .contains("key=secret"));
    }
}

#[test]
fn duplicate_ids_unknown_fields_and_trailing_json_are_rejected() {
    let original = document();
    for section in ["/providers", "/providers/0/models"] {
        let mut doc = original.clone();
        let duplicate = doc.pointer(section).unwrap()[0].clone();
        doc.pointer_mut(section)
            .unwrap()
            .as_array_mut()
            .unwrap()
            .push(duplicate);
        assert!(parse(&doc.to_string(), ".").is_err());
    }
    let mut doc = original.clone();
    doc["providers"][0]["api_key"] = json!("secret-value");
    let error = parse(&doc.to_string(), ".").err().unwrap();
    assert!(!serde_json::to_string(&error)
        .unwrap()
        .contains("secret-value"));
    assert!(parse(&(original.to_string() + " {}"), ".").is_err());
}

#[test]
fn nested_models_preserve_provider_identity_and_global_id_uniqueness() {
    let mut doc = document();
    let mut provider = doc["providers"][0].clone();
    provider["id"] = json!("direct");
    provider["base_url"] = json!("https://direct.example/v1");
    provider["models"][0]["id"] = json!("direct-model");
    doc["providers"].as_array_mut().unwrap().push(provider);
    let resolved = parse(&doc.to_string(), ".").unwrap();
    assert_eq!(resolved.as_config().models[1].provider, "direct");
    assert_eq!(
        resolved.as_config().models[1].base_url,
        "https://direct.example/v1"
    );
    doc["providers"][1]["models"][0]["id"] = json!("local");
    let error = parse(&doc.to_string(), ".").unwrap_err();
    assert_eq!(error.details["field"], "providers[1].models[0].id");
}

#[test]
fn old_file_layout_and_empty_provider_models_are_rejected() {
    let mut doc = document();
    doc["models"] = json!([]);
    assert!(parse(&doc.to_string(), ".").is_err());
    let mut doc = document();
    doc["providers"][0]["models"][0]["provider_id"] = json!("gateway");
    assert!(parse(&doc.to_string(), ".").is_err());
    doc["providers"][0]["models"] = json!([]);
    let error = parse(&doc.to_string(), ".").unwrap_err();
    assert_eq!(error.details["field"], "providers[0].models");
}
