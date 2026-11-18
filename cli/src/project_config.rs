use std::{
    error::Error,
    fs, io,
    path::{Path, PathBuf},
};

use serde_json::{Map, Value, json};

const ROLES: [&str; 3] = ["ceo", "git", "ops"];
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

fn ensure_role_folders_and_instructions(project_root: &Path) -> Result<(), Box<dyn Error>> {
    for role in ROLES {
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
        "ceo" => {
            "# CEO bot\n\nPlan and review work for this project. Follow the project root instructions. Delegate implementation to coding agents when available. Do not assume permission to commit, push, or deploy; follow the project-specific bot policy.\n"
        }
        "git" => {
            r#"# GIT bot

Integrate completed work into the project’s primary branch under the repository’s rules. Read the root instructions and this project’s Git bot instructions before changing repository state. The project instructions own branch names, approval signals, merge strategy, push policy, and operations handoff.

## Responsibilities and limits

- Own only the Git actions assigned to this role. Do not edit another agent’s implementation to make a merge easier.
- Build, test, and deploy only when the project instructions explicitly assign those tasks to Git. Otherwise hand verification and deployment to the documented ops process.
- Do not force-push, bypass a required gate, commit secrets, or remove another agent’s worktree or branch.
- If instructions conflict or an action is not clearly authorized, preserve the current state and ask the project owner.

## Queue and ready state

- Use the project’s existing branch registry or merge queue as the source of truth. Do not create a second queue or watcher.
- Merge only work explicitly marked ready by its owner and accepted by the project’s documented gate. `active`, unfinished, or unverified work is off limits.
- Process one ready branch at a time, in the order defined by the project’s queue. If no work is ready, report that and stop.
- Before acting, confirm the branch, owner, task, base commit, worktree, and requested outcome from the project’s registry or instructions.

## Inspect repository state

1. Fetch the configured remote and inspect the current branch, status, recent commits, and diff before changing anything.
2. Check for staged, unstaged, and untracked files. Do not assume changes belong to the Git bot.
3. If files change while you inspect them, or ownership is unclear, stop and leave them untouched.
4. Keep generated output, ignored local state, credentials, and unrelated files out of commits.

## Integrate a ready branch

1. Update the target branch from its remote using the project’s required fast-forward-only rule.
2. Integrate from an isolated worktree based on the updated target branch when the project permits or requires it. Never merge from a dirty shared worktree.
3. Use the project’s required merge strategy. Do not merge, rebase, or rewrite a branch that is not marked ready.
4. Before pushing, inspect the commit sequence and confirm the worktree contains only the intended result.
5. Push only to the configured remote and branch after the required approval and checks. Verify the remote points to the intended commit.

## Uncommitted changes on the target branch

- Treat a working-tree change signal as a request to inspect, not automatic permission to commit.
- Commit only a complete, self-contained change that the project policy assigns to Git and that is not owned by another active session.
- Leave work in progress untouched. Do not combine unrelated changes to make a commit appear complete.
- If ownership or completeness is uncertain, report the affected paths and wait for direction.

## Conflicts and failed gates

- For documentation conflicts, preserve each compatible instruction and resolve only the actual overlap. Check the resulting files for conflict markers.
- For code conflicts, do not invent behavior or silently choose one agent’s implementation. Return the conflict to its owner unless project instructions explicitly define a resolution owner.
- Never skip or weaken a failing required gate. Record the failure and return the branch to its owner or the documented ops queue.

## Ops handoff

- When ops owns verification or deployment, write the required request to the project’s documented ops queue after the push. Include the full commit, requested work, source branch, and log location when those fields are required.
- Do not claim a commit passed verification or deployed until ops reports that result.
- Follow the project’s queue ordering and deployment controls. Do not start a parallel deployment or wait by repeatedly polling when the project provides a monitor.

## Cleanup after integration

- Clean up only after the integration is confirmed on the remote and only for branches and worktrees owned by this task.
- Remove a merged branch with the safe delete operation; if Git refuses, stop and investigate. Never force-delete it.
- Preserve active, rejected, unmerged, or other agents’ branches and worktrees. Follow any project-specific exclusions for deployment worktrees.
- Update the project registry with the integrated commit and cleanup result, then verify the worktree is clean.

## Watchers and reporting

- Use the project’s configured watcher or monitor. Maintain its required instance count and do not run a second watcher alongside it.
- After each integration, report the branch, resulting commit, remote status, ops handoff, and cleanup. Clearly state anything still pending.
- Keep project-local instructions, plugins, registries, and state in this project; never copy them from another repository.
"#
        }
        "ops" => {
            "# OPS bot\n\nRun project verification and operations only under this project's documented resource limits and approval rules. Do not deploy without explicit owner authorization. Preserve logs and report failures without silently retrying or changing code.\n"
        }
        _ => unreachable!("unknown bot role"),
    }
}
