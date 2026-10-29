//! Vendor formats stay inside the client configuration part.

use super::{Client, Setup, helper};
use crate::config;
use serde_json::{Map, Value, json};
use toml_edit::{DocumentMut, Item, Table, value};

pub(super) fn header_json(key: &str) -> Result<String, String> {
    serde_json::to_string(&json!({"Authorization": format!("Bearer {key}")}))
        .map_err(|_| "could not encode the header".to_string())
}

fn object(value: &mut Value) -> Result<&mut Map<String, Value>, String> {
    value
        .as_object_mut()
        .ok_or("client configuration must contain objects, not scalars or lists".to_string())
}

fn parse_json(text: &str) -> Result<Value, String> {
    serde_json::from_str(text).map_err(|e| {
        format!(
            "invalid client JSON at line {}, column {}; the file was not changed",
            e.line(),
            e.column()
        )
    })
}

fn key(setup: &Setup<'_>, preview: bool) -> Result<Option<String>, String> {
    let Some(path) = setup.key_file else {
        return Ok(None);
    };
    if preview {
        return Ok(Some(format!(
            "<key read from {} by --write>",
            path.display()
        )));
    }
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("could not read {}: {e}", path.display()))?;
    let key = text.trim();
    config::check_access_key(key).map_err(|e| e.to_string())?;
    Ok(Some(key.to_string()))
}

fn json_entry(
    client: Client,
    setup: &Setup<'_>,
    preview: bool,
    original: Option<Value>,
) -> Result<Value, String> {
    let mut entry = original.unwrap_or_else(|| json!({}));
    let fields = object(&mut entry)?;
    // Replace transport/credential fields owned by connect; retain filters,
    // timeouts and all unrelated client settings, including other headers.
    for name in [
        "command",
        "args",
        "env",
        "url",
        "serverUrl",
        "headersHelper",
        "type",
        "transport",
    ] {
        fields.remove(name);
    }
    let mut headers = fields.remove("headers").unwrap_or_else(|| json!({}));
    object(&mut headers)?.retain(|name, _| !name.eq_ignore_ascii_case("authorization"));
    match client {
        Client::ClaudeCode => {
            fields.insert("type".into(), "http".into());
            fields.insert("url".into(), setup.url.into());
            if let Some(command) = helper(setup)? {
                fields.insert("headersHelper".into(), command.into());
            }
        }
        Client::Agy => {
            fields.insert("serverUrl".into(), setup.url.into());
            if let Some(key) = key(setup, preview)? {
                object(&mut headers)?
                    .insert("Authorization".into(), format!("Bearer {key}").into());
            }
        }
        Client::MuseCode => {
            fields.insert("transport".into(), "streamable_http".into());
            fields.insert("url".into(), setup.url.into());
            // A required server that is down aborts every Muse run, so
            // kkt's entry must never hold the run hostage.
            fields.insert("mode".into(), "optional".into());
            if let Some(key) = key(setup, preview)? {
                object(&mut headers)?
                    .insert("Authorization".into(), format!("Bearer {key}").into());
            }
        }
        Client::OpenCode => {
            fields.remove("environment");
            fields.remove("cwd");
            fields.insert("type".into(), "remote".into());
            fields.insert("url".into(), setup.url.into());
            fields.insert("oauth".into(), false.into());
            if let Some(path) = setup.key_file {
                let path = path.to_str().ok_or("key path is not UTF-8")?;
                if path.contains(['{', '}', '"', '\\']) || path.chars().any(char::is_control) {
                    return Err(
                        "OpenCode file references cannot represent braces, quotes, backslashes or control characters in a key path"
                            .into(),
                    );
                }
                object(&mut headers)?.insert(
                    "Authorization".into(),
                    format!("Bearer {{file:{path}}}").into(),
                );
            }
        }
        Client::Codex => return Err("Codex uses TOML".into()),
    }
    if !object(&mut headers)?.is_empty() {
        fields.insert("headers".into(), headers);
    }
    Ok(entry)
}

fn json_document(
    client: Client,
    setup: &Setup<'_>,
    text: Option<&str>,
    preview: bool,
) -> Result<(Value, Value), String> {
    let mut document = match text {
        Some(text) => parse_json(text)?,
        None => json!({}),
    };
    let original = document.clone();
    if client == Client::MuseCode {
        // Muse Code refuses the whole file without this, so a file we
        // create or extend has to carry it. A version already there is
        // left alone: Muse reports it if it cannot use it.
        object(&mut document)?
            .entry("schema_version")
            .or_insert_with(|| json!(1));
    }
    let group = if client == Client::OpenCode {
        "mcp"
    } else {
        "mcpServers"
    };
    let servers = object(&mut document)?
        .entry(group)
        .or_insert_with(|| json!({}));
    let servers = object(servers)?;
    let entry = json_entry(client, setup, preview, servers.remove("kkt"))?;
    servers.insert("kkt".into(), entry);
    Ok((original, document))
}

fn toml_document(setup: &Setup<'_>, text: Option<&str>) -> Result<String, String> {
    let mut document = text
        .unwrap_or("")
        .parse::<DocumentMut>()
        .map_err(|_| "invalid Codex TOML; the file was not changed".to_string())?;
    let servers = document
        .entry("mcp_servers")
        .or_insert(Item::Table(Table::new()))
        .as_table_like_mut()
        .ok_or("mcp_servers must be a TOML table")?;
    let entry = servers
        .entry("kkt")
        .or_insert(Item::Table(Table::new()))
        .as_table_like_mut()
        .ok_or("mcp_servers.kkt must be a TOML table")?;
    for name in [
        "command",
        "args",
        "env",
        "bearer_token_env_var",
        "http_headers_helper",
    ] {
        entry.remove(name);
    }
    for name in ["http_headers", "env_http_headers"] {
        if let Some(headers) = entry.get_mut(name) {
            let headers = headers
                .as_table_like_mut()
                .ok_or("Codex HTTP headers must be a table")?;
            let names: Vec<_> = headers
                .iter()
                .filter(|(name, _)| name.eq_ignore_ascii_case("authorization"))
                .map(|(name, _)| name.to_string())
                .collect();
            for name in names {
                headers.remove(&name);
            }
        }
    }
    entry.insert("url", value(setup.url));
    if let Some(command) = helper(setup)? {
        entry.insert("http_headers_helper", value(command));
    }
    Ok(document.to_string())
}

pub(super) fn preview(client: Client, setup: &Setup<'_>) -> Result<String, String> {
    if client == Client::Codex {
        return toml_document(setup, None);
    }
    let (_, doc) = json_document(client, setup, None, true)?;
    if client == Client::ClaudeCode {
        let entry =
            claude_entry(&serde_json::to_string(&doc).map_err(|_| "could not encode config")?)?;
        return Ok(format!(
            "claude mcp add-json --scope user kkt {}",
            super::shell_word(&entry)
        ));
    }
    serde_json::to_string_pretty(&doc).map_err(|_| "could not encode config".into())
}

pub(super) fn merge(
    client: Client,
    setup: &Setup<'_>,
    text: Option<&str>,
) -> Result<Option<String>, String> {
    if client == Client::Codex {
        let updated = toml_document(setup, text)?;
        return Ok(if text == Some(updated.as_str()) {
            None
        } else {
            Some(updated)
        });
    }
    let (original, updated) = json_document(client, setup, text, false)?;
    if original == updated {
        return Ok(None);
    }
    serde_json::to_string_pretty(&updated)
        .map(|text| Some(format!("{text}\n")))
        .map_err(|_| "could not encode config".to_string())
}

pub(super) fn has_claude_entry(text: &str) -> Result<bool, String> {
    Ok(parse_json(text)?
        .get("mcpServers")
        .and_then(|s| s.get("kkt"))
        .is_some())
}

pub(super) fn claude_entry(text: &str) -> Result<String, String> {
    let document = parse_json(text)?;
    let entry = document
        .get("mcpServers")
        .and_then(|s| s.get("kkt"))
        .ok_or("missing kkt entry")?;
    serde_json::to_string(entry).map_err(|_| "could not encode Claude config".into())
}
