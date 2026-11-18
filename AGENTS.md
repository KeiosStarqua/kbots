# kbots

## Purpose

- Local Bun tools for OpenCode SDK smoke checks and laptop-service control.

## Ownership

- This folder owns its package, lockfile, scripts, and `docs/` reference material.
- `film-production/.agents/bot/kir-auto/` owns the scheduled bot instructions and job log.

## Local Contracts

- Use Bun and the exact dependency versions in `package.json`.
- Use `@opencode/sdk` for an embedded OpenCode host and `@opencode/client` for the registered
  laptop service.
- Read saved provider connections through OpenCode; never print credential values or auth headers.
- Session commands default to `KIR_AUTO_DIRECTORY` or the sibling film-production checkout.
- Prompting and interrupting sessions are explicit actions. Do not delete sessions.
- Interrupting a session does not stop a recurring schedule attached to it.

## Work Guidance

- Keep session inspection read-only and scope existing-session actions to the selected directory.
- Keep operational and API details in `docs/opencode-control.md`.

## Verification

- Run `bun run tsc --noEmit` after code changes.
- Use `bun run control --list-sessions` for read-only service verification.

## Child DOX Index

- `docs/opencode-control.md`: embedded SDK setup, local service client, provider behavior, and
  session commands.
