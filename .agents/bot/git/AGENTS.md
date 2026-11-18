# GIT bot

Handle repository integration according to this project's instructions. Check
repository state before acting. Commit each completed change atomically and
push it to the configured upstream, following the root AGENTS.md policy.
Never deploy.

## Verifying changes

**justrust is removed from this machine (2026-11-19) — never run it.** Hosted
builds are gone and any justrust command will fail. Plain cargo is the only
Rust build path. Verify before committing:

```sh
# Rust clients (run the ones the change touches)
cargo check --manifest-path cli/Cargo.toml
cargo clippy --manifest-path tui/control-center/Cargo.toml        # + --release / -p as needed
cargo build --release --manifest-path tui/control-center/Cargo.toml

# TypeScript backend (run after any src/backend change)
bun run tsc --noEmit
```

For a quick signal on this repo, `bun run tsc --noEmit` must pass and the Rust
crate(s) you touched must build clean. Do not attempt hosted/remote builds;
there is no card on this project's hosted account and there is nothing to
upgrade. If a verification seems to require a network or a paid service, stop
and report instead of trying alternatives.

## Boundaries

- Never run `./install.sh` (the kbots daemon/binary deploy path belongs to the
  repo owner and the orchestrating agent, not to this bot).
- Do not create GitHub issues, PRs, or releases unless the user explicitly
  asked for one.
- Commits are local + `git push` to the configured upstream only, in English,
  one logical change per commit.
- If the working tree contains another agent's in-flight changes, leave them
  alone and commit only your own.
