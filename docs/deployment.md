# kbots local deployment

Laptop-local deployment only: no CI/CD, no containers, no remote servers. The
repo owns everything under `src/backend/` (Bun daemon), `gui/` (Slint desktop
client), `tui/control-center/` (Ratatui terminal client), and `cli/` (Rust
automation CLI). There are two install paths: the Nix flake (used on
Nix-managed hosts) and `./install.sh` (non-Nix hosts).

## Nix flake (the install path on Nix-managed hosts)

`flake.nix` packages the whole stack and exposes a Home Manager module. It
never touches `~/.local/bin` or an unmanaged systemd unit.

| Output | Contents |
|---|---|
| `packages.default` / `packages.kbots` | `kbots`, `kb`, `kbots-tui`, `kbots-gui`, `kbots-daemon`, desktop entry |
| `packages.kbots-cli` / `kbots-tui` / `kbots-gui` / `kbots-daemon` | one piece each |
| `homeModules.default` | `services.kbots.*` (package, daemon service, PATH, socket) |
| `devShells.default` | Bun, Rust, and the Slint/GUI build deps |

Build and run from a checkout:

```sh
nix build .#            # everything, result/ holds the combined package
nix build .#kbots-daemon
nix develop             # Bun + cargo + pkg-config + font libs
./result/bin/kbots-daemon --socket /tmp/kbots.sock
```

Home Manager integration (input + module + enable):

```nix
# flake.nix
inputs.kbots = {
  url = "git+file:///path/to/kbots";
  inputs.nixpkgs.follows = "nixpkgs";
};
# modules
modules = [ ./home.nix inputs.kbots.homeModules.default ];

# home.nix
services.kbots = {
  enable = true;
  # package = ...;                 # defaults to kbots.packages.<system>.default
  # socket = "/run/user/1000/kbots.sock";
  # path = [ ... ];                # where hc/opencode/codex live
};
```

The module adds the package to `home.packages` and runs
`kbots-daemon.service` as a systemd user service:
`ExecStart=<package>/bin/kbots-daemon`, `Restart=on-failure`,
`WantedBy=default.target`, with `KBOTS_DEPLOY_VERSION` set to the package
version and PATH defaulting to
`~/.local/bin:~/.opencode/bin:~/.bun/bin:/usr/local/bin:/usr/bin:/bin`.

Notes:

- The daemon is compiled to a standalone Bun binary; Bun is not a runtime
  dependency of the installed daemon.
- `bunDeps` in `flake.nix` is a fixed-output derivation running
  `bun install --frozen-lockfile`. When `bun.lock` changes, `nix build
  .#kbots-daemon` fails with `got: sha256-…`; paste that hash into
  `outputHash` and rebuild.
- Nix builds report the SemVer from `cli/Cargo.toml`; they do not create
  `deploy/*` tags or `.deploy-version`.
- Do not run `./install.sh` on a Nix-managed host: it would create competing
  binaries in `~/.local/bin` and an unmanaged user unit.

## What ./install.sh does (non-Nix hosts)

Run it from anywhere; it resolves the repo root from its own path. Steps, in
order:

1. Requires `bun`, `cargo`, `install`, `mkdir` on `PATH`. If one is missing it
   prints `Required command not found: <name>` and exits 1.
2. `bun install --frozen-lockfile` in the repo root — installs Bun
   dependencies from the frozen lockfile.
3. `bun run tsc --noEmit` — type-checks the TypeScript sources.
4. `cargo build --release` for three manifests:
   - `gui/Cargo.toml` (`kbots-control-center`)
   - `tui/control-center/Cargo.toml` (`kbots-tui`)
   - `cli/Cargo.toml` (`kbots`)
5. Installs the binaries with `install -m 755` into
   `KBOTS_INSTALL_DIR` (default `$HOME/.local/bin`):
   - gui binary → `kbots-gui`
   - tui binary → `kbots-tui`
   - cli binary → `kbots`

   It also removes a stale `$install_dir/kbots-cli`, and creates a `kb`
   symlink → `kbots` (removing any existing `kb` first).
6. Writes a desktop launcher: renders `gui/kbots.desktop.in` (the
   `Exec=@KBOTS_GUI_EXEC@` placeholder is replaced with the quoted, escaped
   path to `kbots-gui`) to
   `${XDG_DATA_HOME:-$HOME/.local/share}/applications/kbots.desktop` and
   chmods it 644.
7. Installs and (re)starts the `kbots-daemon` systemd user service:
   - Skipped entirely when `KBOTS_SKIP_DAEMON_UNIT=1`.
   - Otherwise only if `systemctl --user list-units` works (a user systemd
     session is available); if not, it prints a note to run `bun run daemon`
     manually.
   - When available it renders `deploy/kbots-daemon.service` (substituting the
     `@REPO_ROOT@` and `@BUN@` placeholders) to
     `${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user/kbots-daemon.service`,
     chmods it 644, then runs `systemctl --user daemon-reload`,
     `systemctl --user enable --now kbots-daemon.service`, and
     `systemctl --user restart kbots-daemon.service`. It then waits up to
     ~10s (50 × 0.2s) for the socket to appear — never fails the install if
     it does not. `Type=simple` means systemd reports the unit active before
     the daemon actually binds the socket.
8. Tags the deployed commit with a CalVer deploy tag (`deploy/YYYY.MM.DD.n`,
   annotated) and writes `.deploy-version`. Runs on every successful deploy,
   including without systemd; skipped with a note when git is unavailable or
   the repo root is not a work tree. If the working tree is dirty, the
   recorded version gets a `+dirty` suffix and a warning is printed. See
   "Deploy tags and versioning (SemVer + CalVer)" below.
9. Warns if the install dir is not on `PATH`.

`./install.sh` is the deploy path on non-Nix hosts: clients and the daemon
must run freshly deployed code, so never skip it after code changes there.
On a Nix-managed host use `home-manager switch` instead. Use
`KBOTS_SKIP_DAEMON_UNIT=1` only when the service restart is explicitly
unwanted — then restart the daemon yourself
(`systemctl --user restart kbots-daemon`). Use `KBOTS_INSTALL_DIR` when the
default `~/.local/bin` is not the intended destination.

The installed unit (`deploy/kbots-daemon.service`) is `Type=simple`,
`WorkingDirectory=<repo root>`, `ExecStart=<bun> run src/backend/daemon.ts`,
`Restart=on-failure`, `RestartSec=2`, `WantedBy=default.target` — it starts on
login and auto-restarts. The socket defaults to `$XDG_RUNTIME_DIR/kbots.sock`
(falling back to `$HOME/.local/run/kbots.sock`); set `KBOTS_SOCKET` in the
unit to override. By default the daemon also binds a secondary listener on
`$HOME/.local/run/kbots.sock` (skipped when `KBOTS_SOCKET`/`--socket` is set
or another daemon serves that path) so sandboxed agents that cannot connect
to `/run/user/*` can reach it via the fallback.

## Deploy tags and versioning (SemVer + CalVer)

Two tag lines coexist, because stable releases and continuous deploys have
different cadences:

- **Stable releases — SemVer** (`vX.Y.Z`, e.g. `v0.1.0`). Created manually on
  a release commit. `DAEMON_VERSION` in `src/backend/orchestrator/system.ts`
  is the SemVer source of truth; bump it in the release commit to match the
  tag.
- **Deploys — CalVer** (`deploy/YYYY.MM.DD.n`, e.g. `deploy/2026.11.19.1`).
  Created automatically by `./install.sh` after every successful deploy: an
  annotated tag on the deployed commit (`n` is the per-day deploy counter),
  plus `.deploy-version` (untracked, gitignored) holding the same string.
  This is for the fast, commit-often agent-development pace — every deploy
  is traceable without cutting a release.

The daemon reports the deploy version in `system.status` (`kb status`):
`.deploy-version` wins when present, then `KBOTS_DEPLOY_VERSION` (useful for
manual runs) if set, else the SemVer `DAEMON_VERSION`. If the working tree
was dirty at deploy time, the recorded version carries a `+dirty` suffix
(e.g. `2026.11.19.1+dirty`) and `install.sh` prints a warning — the running
code is then not exactly the tagged commit.

Checking what is deployed / deploy history:

```sh
kb status                      # version of the running daemon
git tag -l 'deploy/*'          # every deploy, oldest first
git log --oneline deploy/2026.11.19.1..HEAD   # changes since that deploy
git diff deploy/2026.11.19.1 deploy/2026.11.19.2   # what changed between two deploys
```

Deploy tags are created locally by `./install.sh` and are not pushed
automatically; run `git push origin --tags` if you want them on the remote.
(CI/CD and remote servers remain out of scope; this repo deploys from a
laptop working tree.)

## Verifying the deployment

Check the daemon is up (`kb` is the symlink to `kbots`):

```sh
kb status          # or: kbots status
```

Prints daemon name/pid/socketPath/uptime/version, pendingApprovals, and
project/task counts; JSON when piped or with `--json`. Sample (real, trimmed):

```json
{"daemon":{"name":"kbots-daemon","pid":416345,"socketPath":"/run/user/1000/kbots.sock","uptimeSeconds":355,"version":"2026.11.19.1"}, ...}
```

The socket: default path is `$XDG_RUNTIME_DIR/kbots.sock`, fallback
`$HOME/.local/run/kbots.sock` (also bound as a secondary listener by default —
see above); override with `KBOTS_SOCKET` or `--socket` (an explicit override
disables the secondary listener). The daemon creates the socket's parent
directory, unlinks stale sockets, sets mode 0600, and cleans up on
SIGINT/SIGTERM. Check it:

```sh
ls -l "$XDG_RUNTIME_DIR/kbots.sock"   # expect srw-------
```

One newline-delimited JSON-RPC 2.0 request/response per connection. A raw
health check over the socket, using bun:

```sh
bun -e 'const c = await Bun.connect({ unix: process.env.XDG_RUNTIME_DIR + "/kbots.sock", socket: { data(s,d){process.stdout.write(d);process.exit(0)}, open(s){s.write(JSON.stringify({jsonrpc:"2.0",id:1,method:"system.status",params:{}})+"\n")} } })'
```

(If `XDG_RUNTIME_DIR` is unset, use `$HOME/.local/run/kbots.sock`.)

Service status and logs:

```sh
systemctl --user status kbots-daemon
journalctl --user -u kbots-daemon -f
```

Sanity check that the deployed binaries are on `PATH` and fresh:

```sh
command -v kb kbots kbots-gui kbots-tui
```

## Testing DB migrations safely with a scratch XDG_STATE_HOME

Daemon state lives at `${XDG_STATE_HOME:-$HOME/.local/state}/kbots/kbots.db`
(SQLite, WAL mode). Migrations run automatically at `openDatabase()` time based
on `PRAGMA user_version` (current `SCHEMA_VERSION` is 5; fresh DBs get
`SCHEMA_DDL`; existing DBs apply incremental deltas: 1→2 agents, ≤2→3 watchers,
≤3→4 tasks rebuild with `foreign_keys` OFF, ≤4→5 `agents.last_outcome`/
`last_outcome_at` and `task_messages.attempts`; then `user_version` is set).

NEVER point a migration test at the real state dir. Instead:

1. Back up first:

   ```sh
   cp "${XDG_STATE_HOME:-$HOME/.local/state}/kbots/kbots.db" kbots.db.bak
   # also -wal/-shm if present; or work on a copy only
   ```

2. Create a scratch dir (`/tmp/opencode` is approved; or use plain
   `mktemp -d`):

   ```sh
   SCRATCH=$(mktemp -d /tmp/opencode/kbots-migrate.XXXXXX)
   ```

3. Copy the production DB into the scratch:

   ```sh
   mkdir -p "$SCRATCH/kbots"
   cp "${XDG_STATE_HOME:-$HOME/.local/state}/kbots/kbots.db" "$SCRATCH/kbots/kbots.db"
   ```

4. Run the migrator against the scratch copy with `XDG_STATE_HOME="$SCRATCH"`.
   Two safe options:

   (i) One-shot script, from the repo root:

   ```sh
   XDG_STATE_HOME="$SCRATCH" bun -e 'const { openDatabase } = await import("./src/backend/db/database.ts"); const db = openDatabase(); console.log("user_version:", db.query("PRAGMA user_version").get());'
   ```

   (ii) A throwaway daemon against the scratch — point BOTH `XDG_STATE_HOME`
   and `KBOTS_SOCKET` at scratch values (critical: otherwise it tries to bind
   the real socket and fails, or worse):

   ```sh
   XDG_STATE_HOME="$SCRATCH" KBOTS_SOCKET="$SCRATCH/kbots-test.sock" bun run daemon
   ls -l "$SCRATCH/kbots-test.sock"
   # stop it with Ctrl-C / kill when done
   ```

5. Verify: `PRAGMA user_version` equals `SCHEMA_VERSION` (5), tables exist,
   row counts are preserved (e.g. tasks count before/after). If the tasks
   table was rebuilt (v3→v4), spot-check that task ids survived and legacy
   statuses were remapped (`planning`→`queued`, `in_progress`→`running`,
   `completed`→`done`).

6. Inspect with `bun -e` or `sqlite3` if available:

   ```sh
   XDG_STATE_HOME="$SCRATCH" bun -e '...'
   ```

   Do NOT open the production DB from another process while the daemon is
   running without a backup; WAL allows concurrent readers, but the migration
   itself must only ever run against the scratch copy.

7. Clean up the scratch dir when done. Delete it; never copy the scratch DB
   back over production.

Fresh-install migration testing: point `XDG_STATE_HOME` at an empty scratch
dir and open the DB — it should create the full latest schema
(`user_version = 5`).

Do not set `XDG_STATE_HOME` for the systemd user service casually — the unit
inherits the default; mixed state dirs create a second, empty `kbots.db`.

## Restarting the daemon manually

```sh
systemctl --user restart kbots-daemon
```

Then check:

```sh
systemctl --user status kbots-daemon
journalctl --user -u kbots-daemon -n 50 --no-pager
kb status
ls -l "$XDG_RUNTIME_DIR/kbots.sock"
```

Without systemd (or with `KBOTS_SKIP_DAEMON_UNIT=1`), run the daemon in the
foreground from the repo root:

```sh
bun run daemon     # equivalently: bun run src/backend/daemon.ts
```

Stop it with Ctrl-C / SIGINT; the socket is cleaned up on SIGINT/SIGTERM. For
a background run without systemd, use your own session manager (e.g.
`nohup`/`setsid`) — the repo does not ship one.

The daemon unlinks stale sockets at startup, so a restart after a crash is
safe.

If you changed only the daemon code and want to redeploy, `./install.sh` is
the normal path (it restarts the service). Do NOT hand-edit the installed
unit (`~/.config/systemd/user/kbots-daemon.service`) — it is regenerated from
`deploy/kbots-daemon.service` on every `./install.sh`.

## See also

- [docs/kbots-ipc.md](kbots-ipc.md)
- [ARCHITECTURE.md](../ARCHITECTURE.md)
- [cli/README.md](../cli/README.md)
- [deploy/kbots-daemon.service](../deploy/kbots-daemon.service)
- [install.sh](../install.sh)
