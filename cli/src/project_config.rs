use std::{
    error::Error,
    fs, io,
    path::{Path, PathBuf},
};

use serde_json::{Map, Value, json};

const ROLES: [&str; 4] = ["ceo", "git", "ops", "audit"];
/// The embedded Codex task-router bot folder. Seeded and synced alongside the
/// OpenCode roles, but not an OpenCode role: no manifest entry, no config.
const SEEDED_ROLES: [&str; 5] = ["ceo", "git", "ops", "audit", "router"];
const CONFIG_FILE: &str = "kbots.json";
/// Local, machine-scoped record of what the installed CLI scaffolded in this
/// repo: the kbots version, a hash of the embedded template set, and the
/// hashes of the files it wrote. Lets `project sync` upgrade untouched files
/// after a kbots upgrade while never overwriting customized ones.
const SCAFFOLD_STAMP: &str = ".scaffold.json";
const OPENCODE_SCHEMA: &str = "https://opencode.ai/config.json";

pub fn ensure_project_bot_scaffold(project_root: &Path) -> Result<(), Box<dyn Error>> {
    ensure_role_folders_and_instructions(project_root)?;
    stamp_if_missing(project_root)?;
    let manifest_path = manifest_path(project_root);

    if !manifest_path.exists() {
        // Existing OpenCode configs are project-owned: adopt them into a
        // manifest instead of inventing defaults that would disagree, and
        // never rewrite the configs themselves (render_configs preserves).
        if ROLES.iter().any(|role| {
            project_root
                .join(bot_dir(role))
                .join("opencode.jsonc")
                .exists()
        }) {
            let config = manifest_from_existing_configs(project_root)?;
            write_manifest_value(project_root, &config)?;
        } else {
            write_manifest(project_root)?;
        }
    }

    ensure_manifest_roles(project_root)?;
    render_configs(project_root, false)?;
    Ok(())
}

pub fn configure_project(
    requested_path: Option<PathBuf>,
    overwrite: bool,
) -> Result<Value, Box<dyn Error>> {
    let requested_path = requested_path.unwrap_or(std::env::current_dir()?);
    let project_root = fs::canonicalize(&requested_path)?;
    if !project_root.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "project path is not a directory: {}",
                project_root.display()
            ),
        )
        .into());
    }

    ensure_role_folders_and_instructions(&project_root)?;
    stamp_if_missing(&project_root)?;
    let manifest_path = manifest_path(&project_root);
    if !manifest_path.exists() {
        if ROLES.iter().any(|role| {
            project_root
                .join(bot_dir(role))
                .join("opencode.jsonc")
                .exists()
        }) {
            let config = manifest_from_existing_configs(&project_root)?;
            write_manifest_value(&project_root, &config)?;
        } else {
            write_manifest(&project_root)?;
        }
    }

    ensure_manifest_roles(&project_root)?;
    let (written, preserved, disabled) = render_configs(&project_root, overwrite)?;
    Ok(json!({
        "projectPath": project_root,
        "manifest": manifest_path,
        "written": written,
        "preserved": preserved,
        "disabled": disabled,
    }))
}

/// Refresh per-role AGENTS.md starters in an existing project from the
/// templates embedded at build time. Missing files are created; files that
/// still match a template are left alone; customized files are kept unless
/// `force` is set, in which case the old file is backed up next to the
/// original (`AGENTS.md.bak`, `AGENTS.md.bak.N`) before the template is
/// written. The project manifest and OpenCode configs are not touched.
pub fn sync_project(
    requested_path: Option<PathBuf>,
    force: bool,
    check_only: bool,
) -> Result<Value, Box<dyn Error>> {
    let requested_path = requested_path.unwrap_or(std::env::current_dir()?);
    let project_root = fs::canonicalize(&requested_path)?;
    if !project_root.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "project path is not a directory: {}",
                project_root.display()
            ),
        )
        .into());
    }

    let stamp = read_stamp(&project_root);
    let recorded_files = stamp
        .as_ref()
        .and_then(|stamp| stamp.get("files"))
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let recorded_version = stamp
        .as_ref()
        .and_then(|stamp| stamp.get("kbotsVersion"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    let templates_hash = templates_hash();

    let mut created: Vec<String> = Vec::new();
    let mut updated: Vec<String> = Vec::new();
    let mut current: Vec<String> = Vec::new();
    let mut kept: Vec<String> = Vec::new();
    let mut stale: Vec<String> = Vec::new();
    let mut backups: Vec<String> = Vec::new();
    // Only files whose bytes we actually wrote (or that already match the
    // current template) get recorded; a customized file keeps no hash so a
    // later sync still treats it as user-owned.
    let mut files = Map::new();

    for role in SEEDED_ROLES {
        let role_dir = project_root.join(bot_dir(role));
        if !check_only {
            fs::create_dir_all(&role_dir)?;
        }
        let agents_path = role_dir.join("AGENTS.md");
        let template = default_agents(role);
        if !agents_path.exists() {
            if check_only {
                stale.push(role.to_owned());
                continue;
            }
            fs::write(&agents_path, template)?;
            created.push(role.to_owned());
            files.insert(role.to_owned(), json!(content_hash(template)));
            continue;
        }
        let existing = fs::read_to_string(&agents_path)?;
        if existing == template {
            current.push(role.to_owned());
            files.insert(role.to_owned(), json!(content_hash(template)));
            continue;
        }
        let untouched_since_sync = recorded_files
            .get(role)
            .and_then(Value::as_str)
            .is_some_and(|recorded| recorded == content_hash(&existing));
        if untouched_since_sync || force {
            if check_only {
                stale.push(role.to_owned());
                continue;
            }
            if force {
                let backup = backup_agents_md(&role_dir)?;
                backups.push(format!("{role}: {}", backup.display()));
            }
            fs::write(&agents_path, template)?;
            updated.push(role.to_owned());
            files.insert(role.to_owned(), json!(content_hash(template)));
        } else {
            kept.push(role.to_owned());
        }
    }

    let up_to_date = stale.is_empty() && updated.is_empty();
    if check_only {
        return Ok(json!({
            "projectPath": project_root,
            "check": true,
            "upToDate": up_to_date,
            "stale": stale,
            "customized": kept,
            "current": current,
            "kbotsVersion": kbots_version(),
            "templatesHash": templates_hash,
            "recordedVersion": recorded_version,
        }));
    }

    let stamp_file = write_stamp(&project_root, &templates_hash, &files)?;
    Ok(json!({
        "projectPath": project_root,
        "force": force,
        "created": created,
        "updated": updated,
        "current": current,
        "kept": kept,
        "backups": backups,
        "kbotsVersion": kbots_version(),
        "templatesHash": templates_hash,
        "previousVersion": recorded_version,
        "stamp": stamp_file.display().to_string(),
    }))
}

/// The scaffold stamp is machine-local state (like the session registry), so
/// keep it out of the project's git. Appends the ignore entry once.
fn ensure_stamp_ignored(project_root: &Path) -> Result<(), Box<dyn Error>> {
    let dir = project_root.join(".agents").join("bot");
    fs::create_dir_all(&dir)?;
    let path = dir.join(".gitignore");
    let existing = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error.into()),
    };
    let sibling_entry = format!("/{SCAFFOLD_STAMP}");
    let already_listed = existing.lines().any(|line| {
        let line = line.trim();
        !line.is_empty()
            && !line.starts_with('#')
            && (line == SCAFFOLD_STAMP
                || line == sibling_entry
                || line.ends_with(&format!("/{SCAFFOLD_STAMP}")))
    });
    if already_listed {
        return Ok(());
    }
    let mut text = existing;
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    if text.is_empty() {
        text.push_str("# kbots scaffold state — machine-local, never committed.\n");
    }
    text.push_str(SCAFFOLD_STAMP);
    text.push('\n');
    fs::write(&path, text)?;
    Ok(())
}

fn backup_agents_md(role_dir: &Path) -> Result<PathBuf, Box<dyn Error>> {
    let source = role_dir.join("AGENTS.md");
    for index in 0..1000u32 {
        let candidate = if index == 0 {
            role_dir.join("AGENTS.md.bak")
        } else {
            role_dir.join(format!("AGENTS.md.bak.{index}"))
        };
        if !candidate.exists() {
            fs::copy(&source, &candidate)?;
            return Ok(candidate);
        }
    }
    Err(io::Error::new(io::ErrorKind::Other, "too many AGENTS.md backups").into())
}

fn ensure_role_folders_and_instructions(project_root: &Path) -> Result<(), Box<dyn Error>> {
    for role in SEEDED_ROLES {
        let role_dir = project_root.join(bot_dir(role));
        fs::create_dir_all(&role_dir)?;
        let agents_path = role_dir.join("AGENTS.md");
        if !agents_path.exists() {
            fs::write(agents_path, default_agents(role))?;
        }
    }
    Ok(())
}

/// Decorate a `projects.list` response with per-repo scaffold state so the
/// operator can see which repos still carry an older plant.
pub fn enrich_projects_list(response: &mut Value) {
    let current_hash = templates_hash();
    let Some(items) = response.get_mut("items").and_then(Value::as_array_mut) else {
        return;
    };
    for item in items {
        let Some(path) = item.get("path").and_then(Value::as_str).map(str::to_owned) else {
            continue;
        };
        let Some(object) = item.as_object_mut() else {
            continue;
        };
        match read_stamp(Path::new(&path)) {
            Some(stamp) => {
                let version = stamp
                    .get("kbotsVersion")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
                    .to_owned();
                let outdated = stamp.get("templatesHash").and_then(Value::as_str)
                    != Some(current_hash.as_str());
                object.insert("scaffoldVersion".to_owned(), json!(version));
                object.insert("scaffoldOutdated".to_owned(), json!(outdated));
            }
            None => {
                object.insert("scaffoldVersion".to_owned(), Value::Null);
                object.insert("scaffoldOutdated".to_owned(), json!(true));
            }
        }
    }
}

fn kbots_version() -> &'static str {
    option_env!("KBOTS_VERSION").unwrap_or(env!("CARGO_PKG_VERSION"))
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn content_hash(text: &str) -> String {
    format!("{:016x}", fnv1a64(text.as_bytes()))
}

fn templates_hash() -> String {
    let mut acc = String::new();
    for role in SEEDED_ROLES {
        acc.push_str(role);
        acc.push('\0');
        acc.push_str(default_agents(role));
        acc.push('\0');
    }
    content_hash(&acc)
}

fn stamp_path(project_root: &Path) -> PathBuf {
    project_root
        .join(".agents")
        .join("bot")
        .join(SCAFFOLD_STAMP)
}

fn read_stamp(project_root: &Path) -> Option<Value> {
    let text = fs::read_to_string(stamp_path(project_root)).ok()?;
    serde_json::from_str::<Value>(&text).ok()
}

fn write_stamp(
    project_root: &Path,
    templates_hash: &str,
    files: &Map<String, Value>,
) -> Result<PathBuf, Box<dyn Error>> {
    ensure_stamp_ignored(project_root)?;
    let path = stamp_path(project_root);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let value = json!({
        "kbotsVersion": kbots_version(),
        "templatesHash": templates_hash,
        "files": files,
        "updatedAt": now_iso(),
    });
    fs::write(
        &path,
        format!("{}\n", serde_json::to_string_pretty(&value)?),
    )?;
    Ok(path)
}

/// First-run baseline: when no stamp exists yet, record the hashes of files
/// that are byte-identical to the current templates, so later upgrades can
/// update them. Customized files stay unrecorded (user-owned).
fn stamp_if_missing(project_root: &Path) -> Result<(), Box<dyn Error>> {
    // The stamp is local state: keep it ignored even when it already exists
    // (older scaffolds predate the ignore entry).
    ensure_stamp_ignored(project_root)?;
    if read_stamp(project_root).is_some() {
        return Ok(());
    }
    let mut files = Map::new();
    for role in SEEDED_ROLES {
        let path = project_root.join(bot_dir(role)).join("AGENTS.md");
        let Ok(existing) = fs::read_to_string(&path) else {
            continue;
        };
        if existing == default_agents(role) {
            files.insert(role.to_owned(), json!(content_hash(&existing)));
        }
    }
    write_stamp(project_root, &templates_hash(), &files)?;
    Ok(())
}

/// JSON-safe timestamp without extra crates.
fn now_iso() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("@{secs}")
}

fn bot_dir(role: &str) -> PathBuf {
    PathBuf::from(".agents").join("bot").join(role)
}

fn manifest_path(project_root: &Path) -> PathBuf {
    project_root.join(".agents").join("bot").join(CONFIG_FILE)
}

fn write_manifest(project_root: &Path) -> Result<(), Box<dyn Error>> {
    let mut bots = Map::new();
    for role in ROLES {
        bots.insert(
            role.to_owned(),
            json!({
                "enabled": true,
                "opencode": {
                    "instructions": [format!(".agents/bot/{role}/AGENTS.md")],
                    "plugins": [],
                }
            }),
        );
    }
    let config = json!({ "version": 1, "bots": bots });
    write_manifest_value(project_root, &config)
}

fn write_manifest_value(project_root: &Path, config: &Value) -> Result<(), Box<dyn Error>> {
    let text = serde_json::to_string_pretty(config)? + "\n";
    fs::write(manifest_path(project_root), text)?;
    Ok(())
}

/// Manifests written before the audit role existed are still valid. Add
/// defaults for any missing role so `project add`/`configure` converge an
/// existing project instead of failing on `render_configs`.
fn ensure_manifest_roles(project_root: &Path) -> Result<(), Box<dyn Error>> {
    let path = manifest_path(project_root);
    let mut manifest: Value = serde_json::from_slice(&fs::read(&path)?)?;
    let bots = manifest
        .get_mut("bots")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{} must contain a bots object", path.display()),
            )
        })?;
    let mut changed = false;
    for role in ROLES {
        if !bots.contains_key(role) {
            bots.insert(
                role.to_owned(),
                json!({
                    "enabled": true,
                    "opencode": {
                        "instructions": [format!(".agents/bot/{role}/AGENTS.md")],
                        "plugins": [],
                    }
                }),
            );
            changed = true;
        }
    }
    if changed {
        write_manifest_value(project_root, &manifest)?;
    }
    Ok(())
}

/// Adopt a project scaffolded by hand or by an older tool: derive a version-1
/// manifest from the per-role `opencode.jsonc` files it already ships, so the
/// project reports no `manifestError` and `sessions start` can run. Roles
/// without a config get the starter defaults. Existing configs stay untouched
/// (`render_configs` preserves them unless `--overwrite` is explicit).
fn manifest_from_existing_configs(project_root: &Path) -> Result<Value, Box<dyn Error>> {
    let mut bots = Map::new();
    for role in ROLES {
        let config_path = project_root.join(bot_dir(role)).join("opencode.jsonc");
        let opencode = match fs::read_to_string(&config_path) {
            Ok(text) => {
                let parsed: Value = serde_json::from_str(&strip_jsonc(&text)).map_err(|error| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("cannot read {} as JSONC: {error}", config_path.display()),
                    )
                })?;
                let mut object = parsed.as_object().cloned().ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("{} must contain a JSON object", config_path.display()),
                    )
                })?;
                // The manifest feeds `render_configs`, which writes its own
                // `$schema`; a project-specific schema URL would be dropped
                // there, so don't carry it into the manifest either.
                object.remove("$schema");
                Value::Object(object)
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => json!({
                "instructions": [format!(".agents/bot/{role}/AGENTS.md")],
                "plugins": [],
            }),
            Err(error) => return Err(error.into()),
        };
        bots.insert(
            role.to_owned(),
            json!({ "enabled": true, "opencode": opencode }),
        );
    }
    Ok(json!({ "version": 1, "bots": bots }))
}

fn render_configs(
    project_root: &Path,
    overwrite: bool,
) -> Result<(Vec<String>, Vec<String>, Vec<String>), Box<dyn Error>> {
    let path = manifest_path(project_root);
    let manifest: Value = serde_json::from_slice(&fs::read(&path)?)?;
    if manifest.get("version").and_then(Value::as_u64) != Some(1) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{} must declare version: 1", path.display()),
        )
        .into());
    }
    let bots = manifest
        .get("bots")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{} must contain a bots object", path.display()),
            )
        })?;

    let mut written = Vec::new();
    let mut preserved = Vec::new();
    let mut disabled = Vec::new();
    for role in ROLES {
        let bot = bots.get(role).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{} is missing the {role} bot config", path.display()),
            )
        })?;
        if bot.get("enabled").and_then(Value::as_bool) == Some(false) {
            disabled.push(role.to_owned());
            continue;
        }
        let output_path = project_root.join(bot_dir(role)).join("opencode.jsonc");
        if output_path.exists() && !overwrite {
            preserved.push(output_path.display().to_string());
            continue;
        }

        let mut open_code_config = bot
            .get("opencode")
            .and_then(Value::as_object)
            .cloned()
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("{}.bots.{role}.opencode must be an object", path.display()),
                )
            })?;
        open_code_config.insert("$schema".to_owned(), json!(OPENCODE_SCHEMA));
        resolve_local_plugins(project_root, &mut open_code_config, role)?;
        resolve_mcp_disables(&mut open_code_config, role)?;
        let output = serde_json::to_string_pretty(&Value::Object(open_code_config))? + "\n";
        fs::write(&output_path, output)?;
        written.push(output_path.display().to_string());
    }
    Ok((written, preserved, disabled))
}

fn resolve_local_plugins(
    project_root: &Path,
    config: &mut Map<String, Value>,
    role: &str,
) -> Result<(), Box<dyn Error>> {
    let Some(plugins) = config.get_mut("plugins") else {
        return Ok(());
    };
    let plugins = plugins.as_array_mut().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("bots.{role}.opencode.plugins must be an array"),
        )
    })?;

    for plugin in plugins {
        let Some(plugin_path) = plugin.as_str() else {
            continue;
        };
        if !is_local_path(plugin_path) {
            continue;
        }
        let local_path = if let Some(home_relative) = plugin_path.strip_prefix("~/") {
            PathBuf::from(std::env::var_os("HOME").ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    "HOME is not set for ~/ plugin path",
                )
            })?)
            .join(home_relative)
        } else {
            let path = PathBuf::from(plugin_path);
            if path.is_absolute() {
                path
            } else {
                project_root.join(path)
            }
        };
        let resolved = fs::canonicalize(&local_path)?;
        if !resolved.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "OpenCode local plugin must be a directory: {}",
                    resolved.display()
                ),
            )
            .into());
        }
        *plugin = json!(resolved.to_string_lossy());
    }
    Ok(())
}

/// Manifest key (`bots.<role>.opencode.mcp.disable`) that names MCP servers to
/// switch off for this role's sessions. It is expanded into real
/// `mcp.servers.<name>` entries before writing, so the generated config stays
/// valid for OpenCode.
const MCP_DISABLE_KEY: &str = "disable";
/// Keys copied from the global server definition when disabling it. Secrets
/// (`environment`, `headers`, `oauth`) are never copied into a project file.
const MCP_COPIED_KEYS: [&str; 5] = ["type", "command", "cwd", "url", "timeout"];

/// Turn `mcp.disable: ["rea", ...]` into `mcp.servers.rea.disabled: true`.
///
/// OpenCode merges MCP servers by name and replaces the whole entry, so a
/// disabled server must be a complete definition. Rather than asking every
/// project to restate a global command (and risking secrets in a tracked
/// file), the definition is read from the global OpenCode config and copied
/// without `environment`/`headers`/`oauth`.
fn resolve_mcp_disables(config: &mut Map<String, Value>, role: &str) -> Result<(), Box<dyn Error>> {
    if config.get("mcp").is_none() {
        return Ok(());
    }
    resolve_mcp_disables_with(config, role, &load_global_mcp_servers())
}

fn resolve_mcp_disables_with(
    config: &mut Map<String, Value>,
    role: &str,
    global: &Map<String, Value>,
) -> Result<(), Box<dyn Error>> {
    let Some(mcp_value) = config.get_mut("mcp") else {
        return Ok(());
    };
    let mcp = mcp_value
        .as_object_mut()
        .ok_or_else(|| invalid_data(format!("bots.{role}.opencode.mcp must be an object")))?;
    let Some(disable) = mcp.remove(MCP_DISABLE_KEY) else {
        return Ok(());
    };
    let names = disable.as_array().ok_or_else(|| {
        invalid_data(format!(
            "bots.{role}.opencode.mcp.{MCP_DISABLE_KEY} must be an array of MCP server names"
        ))
    })?;
    if names.is_empty() {
        return Ok(());
    }

    let servers = mcp
        .entry("servers".to_owned())
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or_else(|| {
            invalid_data(format!(
                "bots.{role}.opencode.mcp.servers must be an object"
            ))
        })?;

    for name in names {
        let name = name.as_str().ok_or_else(|| {
            invalid_data(format!(
                "bots.{role}.opencode.mcp.{MCP_DISABLE_KEY} must contain only strings"
            ))
        })?;
        if servers.contains_key(name) {
            return Err(invalid_data(format!(
                "bots.{role}.opencode.mcp defines servers.{name} and also disables it; pick one"
            ))
            .into());
        }
        let definition = global.get(name).and_then(Value::as_object).ok_or_else(|| {
            invalid_data(format!(
                "bots.{role}.opencode.mcp.{MCP_DISABLE_KEY} names unknown MCP server \"{name}\": not found in the global OpenCode config"
            ))
        })?;
        let mut entry = Map::new();
        for key in MCP_COPIED_KEYS {
            if let Some(value) = definition.get(key) {
                entry.insert(key.to_owned(), value.clone());
            }
        }
        if entry.get("type").is_none() {
            return Err(invalid_data(format!(
                "global MCP server \"{name}\" has no type; cannot disable it from the project config"
            ))
            .into());
        }
        entry.insert("disabled".to_owned(), json!(true));
        servers.insert(name.to_owned(), Value::Object(entry));
    }
    Ok(())
}

/// MCP servers declared by the global OpenCode config, keyed by name. Both the
/// v2 (`mcp.servers.<name>`) and legacy (`mcp.<name>`) shapes are read; the v2
/// entry wins when a name appears in both.
fn load_global_mcp_servers() -> Map<String, Value> {
    let mut servers = Map::new();
    for path in global_config_paths() {
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        let Ok(value) =
            serde_json::from_str::<Value>(&strip_jsonc(text.trim_start_matches('\u{feff}')))
        else {
            continue;
        };
        let Some(mcp) = value.get("mcp").and_then(Value::as_object) else {
            continue;
        };
        if let Some(v2) = mcp.get("servers").and_then(Value::as_object) {
            for (name, definition) in v2 {
                if definition.is_object() {
                    servers
                        .entry(name.clone())
                        .or_insert_with(|| definition.clone());
                }
            }
        }
        for (name, definition) in mcp {
            if name == "servers" || name == "timeout" || name == MCP_DISABLE_KEY {
                continue;
            }
            if definition.get("type").is_some() {
                servers
                    .entry(name.clone())
                    .or_insert_with(|| definition.clone());
            }
        }
    }
    servers
}

fn global_config_paths() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME") {
        dirs.push(PathBuf::from(xdg).join("opencode"));
    }
    if let Some(home) = std::env::var_os("HOME") {
        dirs.push(PathBuf::from(home).join(".config").join("opencode"));
    }
    dirs.into_iter()
        .flat_map(|dir| [dir.join("opencode.json"), dir.join("opencode.jsonc")])
        .collect()
}

/// Strip `//` and `/* */` comments and trailing commas so a JSONC config can be
/// parsed with serde_json. Bytes are copied verbatim (outside comments), so
/// non-ASCII text and escapes survive.
fn strip_jsonc(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut index = 0;
    let mut in_string = false;
    while index < bytes.len() {
        let byte = bytes[index];
        if in_string {
            out.push(byte);
            if byte == b'\\' && index + 1 < bytes.len() {
                out.push(bytes[index + 1]);
                index += 2;
                continue;
            }
            if byte == b'"' {
                in_string = false;
            }
            index += 1;
            continue;
        }
        if byte == b'"' {
            in_string = true;
            out.push(byte);
            index += 1;
            continue;
        }
        if byte == b'/' && index + 1 < bytes.len() {
            if bytes[index + 1] == b'/' {
                while index < bytes.len() && bytes[index] != b'\n' {
                    index += 1;
                }
                continue;
            }
            if bytes[index + 1] == b'*' {
                index += 2;
                while index + 1 < bytes.len() && !(bytes[index] == b'*' && bytes[index + 1] == b'/')
                {
                    index += 1;
                }
                index = (index + 2).min(bytes.len());
                continue;
            }
        }
        out.push(byte);
        index += 1;
    }
    strip_trailing_commas(&String::from_utf8_lossy(&out))
}

/// Drop commas that are followed only by whitespace and a closing brace or
/// bracket. Strings are skipped so `"a,"` keeps its comma.
fn strip_trailing_commas(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut index = 0;
    let mut in_string = false;
    while index < bytes.len() {
        let byte = bytes[index];
        if in_string {
            out.push(byte);
            if byte == b'\\' && index + 1 < bytes.len() {
                out.push(bytes[index + 1]);
                index += 2;
                continue;
            }
            if byte == b'"' {
                in_string = false;
            }
            index += 1;
            continue;
        }
        if byte == b'"' {
            in_string = true;
            out.push(byte);
            index += 1;
            continue;
        }
        if byte == b',' {
            let mut next = index + 1;
            while next < bytes.len() && (bytes[next] as char).is_ascii_whitespace() {
                next += 1;
            }
            if next < bytes.len() && (bytes[next] == b'}' || bytes[next] == b']') {
                index += 1;
                continue;
            }
        }
        out.push(byte);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn invalid_data(message: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn is_local_path(value: &str) -> bool {
    value.starts_with('/')
        || value.starts_with("./")
        || value.starts_with("../")
        || value.starts_with("~/")
}

fn default_agents(role: &str) -> &'static str {
    match role {
        "ceo" => include_str!("../templates/bots/ceo/AGENTS.md"),
        "git" => include_str!("../templates/bots/git/AGENTS.md"),
        "ops" => include_str!("../templates/bots/ops/AGENTS.md"),
        "audit" => include_str!("../templates/bots/audit/AGENTS.md"),
        "router" => include_str!("../templates/bots/router/AGENTS.md"),
        _ => unreachable!("unknown bot role"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn global_servers() -> Map<String, Value> {
        let mut servers = Map::new();
        servers.insert(
            "rea".to_owned(),
            json!({
                "type": "local",
                "command": ["npx", "-y", "rea-agents@6.8.0", "mcp"],
                "environment": { "REA_TOKEN": "secret" },
            }),
        );
        servers
    }

    #[test]
    fn jsonc_comments_and_trailing_commas_are_stripped() {
        let text = "{\n  // line comment with \"quotes\"\n  \"url\": \"http://x/y\", /* block */\n  \"list\": [1, 2,],\n}";
        let value: Value = serde_json::from_str(&strip_jsonc(text)).expect("parseable");
        assert_eq!(value["url"], json!("http://x/y"));
        assert_eq!(value["list"], json!([1, 2]));
    }

    #[test]
    fn disable_copies_a_sanitized_definition() {
        let mut config = json!({
            "instructions": [".agents/bot/ceo/AGENTS.md"],
            "mcp": { "disable": ["rea"] },
        })
        .as_object()
        .expect("object")
        .clone();
        resolve_mcp_disables_with(&mut config, "ceo", &global_servers()).expect("resolves");
        let mcp = config.get("mcp").and_then(Value::as_object).expect("mcp");
        assert!(mcp.get(MCP_DISABLE_KEY).is_none());
        let server = mcp
            .get("servers")
            .and_then(Value::as_object)
            .and_then(|servers| servers.get("rea"))
            .expect("rea entry");
        assert_eq!(
            server,
            &json!({
                "type": "local",
                "command": ["npx", "-y", "rea-agents@6.8.0", "mcp"],
                "disabled": true,
            })
        );
        assert!(server.get("environment").is_none());
    }

    #[test]
    fn disable_without_a_known_server_is_an_error() {
        let mut config = json!({ "mcp": { "disable": ["nope"] } })
            .as_object()
            .expect("object")
            .clone();
        let error = resolve_mcp_disables_with(&mut config, "git", &global_servers())
            .expect_err("unknown server must fail");
        assert!(error.to_string().contains("unknown MCP server"));
    }

    #[test]
    fn disable_conflicting_with_an_explicit_entry_is_an_error() {
        let mut config = json!({
            "mcp": {
                "disable": ["rea"],
                "servers": { "rea": { "type": "local", "command": ["x"], "disabled": false } },
            },
        })
        .as_object()
        .expect("object")
        .clone();
        let error = resolve_mcp_disables_with(&mut config, "ops", &global_servers())
            .expect_err("conflict must fail");
        assert!(error.to_string().contains("also disables"));
    }

    #[test]
    fn config_without_mcp_is_left_alone() {
        let mut config = json!({ "instructions": ["a"] })
            .as_object()
            .expect("object")
            .clone();
        resolve_mcp_disables_with(&mut config, "ceo", &global_servers()).expect("no-op");
        assert_eq!(config.len(), 1);
    }

    #[test]
    fn scaffold_roles_include_audit_with_a_template() {
        assert_eq!(ROLES, ["ceo", "git", "ops", "audit"]);
        assert_eq!(SEEDED_ROLES, ["ceo", "git", "ops", "audit", "router"]);
        assert!(default_agents("audit").contains("# AUDIT bot"));
    }

    #[test]
    fn manifest_written_before_audit_gains_it_without_touching_existing_roles() {
        let root = std::env::temp_dir().join(format!("kbots-manifest-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join(".agents/bot")).expect("bot dir");
        let manifest = json!({
            "version": 1,
            "bots": {
                "ceo": { "enabled": false, "opencode": { "model": "custom/model" } },
                "git": { "enabled": true },
            },
        });
        write_manifest_value(&root, &manifest).expect("write manifest");

        ensure_manifest_roles(&root).expect("converges");
        let upgraded: Value =
            serde_json::from_str(&fs::read_to_string(manifest_path(&root)).expect("read"))
                .expect("parse");
        let bots = upgraded["bots"].as_object().expect("bots");
        assert_eq!(bots["ceo"]["opencode"]["model"], json!("custom/model"));
        assert_eq!(bots["ceo"]["enabled"], json!(false));
        assert_eq!(bots["ops"]["enabled"], json!(true));
        assert_eq!(bots["audit"]["enabled"], json!(true));

        let once = fs::read_to_string(manifest_path(&root)).expect("read");
        ensure_manifest_roles(&root).expect("idempotent");
        assert_eq!(
            fs::read_to_string(manifest_path(&root)).expect("read"),
            once
        );

        let _ = fs::remove_dir_all(&root);
    }
}
