use std::{
    error::Error,
    fs, io,
    path::{Path, PathBuf},
};

use serde_json::{Map, Value, json};

const ROLES: [&str; 3] = ["ceo", "git", "ops"];
/// The embedded Codex task-router bot folder. Seeded and synced alongside the
/// OpenCode roles, but not an OpenCode role: no manifest entry, no config.
const SEEDED_ROLES: [&str; 4] = ["ceo", "git", "ops", "router"];
const CONFIG_FILE: &str = "kbots.json";
const OPENCODE_SCHEMA: &str = "https://opencode.ai/config.json";

pub fn ensure_project_bot_scaffold(project_root: &Path) -> Result<(), Box<dyn Error>> {
    ensure_role_folders_and_instructions(project_root)?;
    let manifest_path = manifest_path(project_root);

    if !manifest_path.exists() {
        // Existing OpenCode configs are project-owned. Don't invent a manifest
        // that could silently disagree with them; migrate them explicitly.
        if ROLES.iter().any(|role| {
            project_root
                .join(bot_dir(role))
                .join("opencode.jsonc")
                .exists()
        }) {
            return Ok(());
        }
        write_manifest(project_root)?;
    }

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
    let manifest_path = manifest_path(&project_root);
    if !manifest_path.exists() {
        if ROLES.iter().any(|role| {
            project_root
                .join(bot_dir(role))
                .join("opencode.jsonc")
                .exists()
        }) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "{} already has opencode.jsonc files; create {} from their settings before generating configs",
                    project_root.display(),
                    manifest_path.display()
                ),
            )
            .into());
        }
        write_manifest(&project_root)?;
    }

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
) -> Result<Value, Box<dyn Error>> {
    let requested_path = requested_path.unwrap_or(std::env::current_dir()?);
    let project_root = fs::canonicalize(&requested_path)?;
    if !project_root.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("project path is not a directory: {}", project_root.display()),
        )
        .into());
    }

    let mut created: Vec<String> = Vec::new();
    let mut updated: Vec<String> = Vec::new();
    let mut current: Vec<String> = Vec::new();
    let mut kept: Vec<String> = Vec::new();
    let mut backups: Vec<String> = Vec::new();
    for role in SEEDED_ROLES {
        let role_dir = project_root.join(bot_dir(role));
        fs::create_dir_all(&role_dir)?;
        let agents_path = role_dir.join("AGENTS.md");
        let template = default_agents(role);
        if !agents_path.exists() {
            fs::write(&agents_path, template)?;
            created.push(role.to_owned());
            continue;
        }
        let existing = fs::read_to_string(&agents_path)?;
        if existing == template {
            current.push(role.to_owned());
            continue;
        }
        if force {
            let backup = backup_agents_md(&role_dir)?;
            fs::write(&agents_path, template)?;
            updated.push(role.to_owned());
            backups.push(format!("{role}: {}", backup.display()));
        } else {
            kept.push(role.to_owned());
        }
    }
    Ok(json!({
        "projectPath": project_root,
        "force": force,
        "created": created,
        "updated": updated,
        "current": current,
        "kept": kept,
        "backups": backups,
    }))
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
    let text = serde_json::to_string_pretty(&config)? + "\n";
    fs::write(manifest_path(project_root), text)?;
    Ok(())
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
        "router" => include_str!("../templates/bots/router/AGENTS.md"),
        _ => unreachable!("unknown bot role"),
    }
}
