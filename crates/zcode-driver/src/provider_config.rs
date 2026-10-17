//! Bridge the existing CLI configuration to 0.16.9's provider registry.
//! The vendor's app-server no longer runs its standalone legacy importer.
use std::{
    io::{Read, Write},
    path::Path,
};

use serde_json::{Value, json};

use super::DriverError;

const LIMIT: u64 = 1024 * 1024;

fn unavailable() -> DriverError {
    DriverError::NotConfigured(
        "ZCode model configuration could not be adapted; check its native provider settings".into(),
    )
}

fn read(path: &Path) -> Result<Option<Value>, DriverError> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(unavailable()),
    };
    let mut bytes = Vec::new();
    file.take(LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| unavailable())?;
    if bytes.len() as u64 > LIMIT {
        return Err(unavailable());
    }
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|_| unavailable())
}

fn adapt(legacy: &Value) -> Result<Value, DriverError> {
    let providers = legacy
        .get("provider")
        .and_then(Value::as_object)
        .ok_or_else(unavailable)?;
    let mut rules = Vec::new();
    let mut model_rules = Vec::new();
    for (id, provider) in providers {
        if provider["enabled"] == false {
            continue;
        }
        // Account providers require their native account authority, not an API-key conversion.
        if id.starts_with("builtin:")
            || id.starts_with("account:")
            || provider["source"]
                .as_str()
                .is_some_and(|source| source != "custom")
        {
            return Err(unavailable());
        }
        let kind = match provider["kind"].as_str() {
            Some("anthropic") => "anthropic-messages",
            Some("openai") => "openai-responses",
            Some("openai-compatible") | None => "openai-chat-completions",
            _ => return Err(unavailable()),
        };
        let options = provider["options"].as_object().ok_or_else(unavailable)?;
        if options
            .keys()
            .any(|k| !matches!(k.as_str(), "apiKey" | "baseURL" | "headers"))
        {
            return Err(unavailable());
        }
        let key = options
            .get("apiKey")
            .and_then(Value::as_str)
            .filter(|k| !k.trim().is_empty())
            .ok_or_else(unavailable)?;
        // Do not feed ciphertext to an API as a key. The new native registry
        // does not decode the old CLI cipher; leave this case explicitly unavailable.
        if key.starts_with("enc:") {
            return Err(unavailable());
        }
        let base = options
            .get("baseURL")
            .and_then(Value::as_str)
            .ok_or_else(unavailable)?;
        if !(base.starts_with("https://") || base.starts_with("http://")) {
            return Err(unavailable());
        }
        let models = provider["models"].as_object().ok_or_else(unavailable)?;
        let mut ids = Vec::new();
        for (name, model) in models {
            if model["deleted"] == true {
                continue;
            }
            let name = model["id"].as_str().unwrap_or(name);
            ids.push(name);
            if let Some(context) = model
                .get("contextWindow")
                .or_else(|| model.pointer("/limit/context"))
                .and_then(Value::as_u64)
            {
                model_rules.push(json!({"providerId":id,"modelId":name,"config":{"properties":{"contextWindow":context}}}));
            }
        }
        let mut api = json!({"type":kind,"baseUrl":base});
        if let Some(headers) = options.get("headers").or_else(|| provider.get("headers")) {
            api["headers"] = headers.clone();
        }
        rules.push(json!({"providerId":id,"providerName":provider["name"].as_str().unwrap_or(id),"enabled":true,
            "config":{"group":"standard-personal","access":{"type":"api-key","apiKey":key},"api":api,"personalModelIds":ids}}));
    }
    if rules.is_empty() {
        return Err(unavailable());
    }
    let mut config = json!({"providerConfigRules":{"providerRules":rules},
        "modelConfigRules":{"providerModelRules":model_rules,"manualProviderModelRules":[]}});
    if let Some((provider, model)) = legacy["model"].as_str().and_then(|s| s.split_once('/')) {
        config["defaultModelSelection"] = json!({"providerId":provider,"modelId":model});
    }
    Ok(json!({"schemaVersion":1,"config":config}))
}

/// Keep the native v2 file authoritative. Only bridge an absent/empty registry.
/// The private file lives for this connection and never modifies vendor files.
pub(super) fn prepare(home: &Path) -> Result<Option<tempfile::NamedTempFile>, DriverError> {
    if let Some(current) = read(&home.join(".zcode/v2/provider_config.json"))? {
        if current["schemaVersion"] != 1 {
            return Ok(None);
        }
        let config = &current["config"];
        let configured = [
            "/providerConfigRules/providerRules",
            "/modelConfigRules/providerModelRules",
            "/modelConfigRules/manualProviderModelRules",
        ]
        .iter()
        .any(|p| {
            config
                .pointer(p)
                .and_then(Value::as_array)
                .is_none_or(|a| !a.is_empty())
        });
        if configured || config.get("defaultModelSelection").is_some() {
            return Ok(None);
        }
    }
    let Some(legacy) = read(&home.join(".zcode/cli/config.json"))? else {
        return Ok(None);
    };
    let config = adapt(&legacy)?;
    let mut file = tempfile::Builder::new()
        .prefix("dogcodes-zcode-provider-")
        .suffix(".json")
        .tempfile()
        .map_err(|_| unavailable())?;
    file.write_all(&serde_json::to_vec(&config).map_err(|_| unavailable())?)
        .map_err(|_| unavailable())?;
    Ok(Some(file))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_ciphertext_and_unsupported_execution_options_instead_of_silently_changing_them() {
        let mut value = json!({"model":"custom/model","provider":{"custom":{"kind":"anthropic","options":{"apiKey":"enc:v1:opaque","baseURL":"https://example.invalid"},"models":{"model":{}}}}});
        assert!(adapt(&value).is_err());
        value["provider"]["custom"]["options"]["apiKey"] = json!("test-only");
        value["provider"]["custom"]["options"]["apiKeyRequired"] = json!(false);
        assert!(adapt(&value).is_err());
    }
    #[test]
    fn private_bridge_preserves_selection_and_native_files() {
        let home = tempfile::tempdir().unwrap();
        let old = home.path().join(".zcode/cli/config.json");
        std::fs::create_dir_all(old.parent().unwrap()).unwrap();
        let value = json!({"model":"custom/model","provider":{"custom":{"kind":"anthropic","options":{"apiKey":"test-only","baseURL":"https://example.invalid"},"models":{"model":{}}}}});
        std::fs::write(&old, value.to_string()).unwrap();
        let bridge = prepare(home.path()).unwrap().unwrap();
        let output = read(bridge.path()).unwrap().unwrap();
        assert_eq!(
            output["config"]["defaultModelSelection"],
            json!({"providerId":"custom","modelId":"model"})
        );
        assert_eq!(read(&old).unwrap().unwrap(), value);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                bridge.as_file().metadata().unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        let path = bridge.path().to_owned();
        drop(bridge);
        assert!(!path.exists());
        let native = home.path().join(".zcode/v2/provider_config.json");
        std::fs::create_dir_all(native.parent().unwrap()).unwrap();
        std::fs::write(&native, output.to_string()).unwrap();
        assert!(prepare(home.path()).unwrap().is_none());
        assert_eq!(read(&native).unwrap().unwrap(), output);
    }
}
