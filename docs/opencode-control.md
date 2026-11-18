# OpenCode SDK and local session control

This guide covers the `kbots` Bun package, its embedded SDK smoke check, and
its client for the laptop's registered OpenCode service.

## Package

The package pins `@opencode/sdk`, `@opencode/client`, and `@opencode/plugin` at
2.3.2 and includes Effect v4 for application logic. Run commands from the
`kbots` directory with Bun:

```sh
bun install
bun run tsc --noEmit
```

`@opencode/sdk` embeds an OpenCode host in the current process. Use
`@opencode/client` to connect to the separate local service. The client uses
`Service.ensure()` to reuse or start the registered service and
`Service.headers()` to apply its authentication header. The header is never
printed.

Import Effect v4 APIs from `effect`. For example, `src/backend/cli/control.ts` uses
`Effect.try` to capture model argument parsing failures and `Effect.runSync` to
run that synchronous parse at the CLI boundary. See the [Effect v4 installation
guide](https://effect.website/docs/v4/getting-started/installation) for runtime
requirements and setup details.

## Embedded host smoke check

`bun run smoke` runs `src/backend/smoke.ts`. It creates the host with `OpenCode.create()`,
registers a plugin, reads provider metadata, creates a session, sends one
prompt, waits for completion, reads the assistant response, and removes the
temporary session.

The host uses the same local OpenCode database as the CLI so it can see the
saved provider connection. By default, the script gets the path from:

```sh
opencode debug paths db
```

Set `OPENCODE_DB_PATH` to override the database path. The script does not print
database contents or credentials.

The default model is `opencode/inkling-2-small-free`. At the time of the
verification below, that model returned a free-usage-limit error. To exercise
OpenCode Go instead, select a Go model:

```sh
OPENCODE_MODEL=opencode-go/deepseek-v4.1-flash bun run smoke
```

That Go model returned `OPENCODE_GO_SDK_OK` in the smoke check. Choose another
available model with the `OPENCODE_MODEL=provider/model` setting.

## Connect to the laptop's running service

`bun run control` reports the service version, process ID, endpoint, and
providers available to the selected project directory. It does not send a
prompt unless `--prompt` is supplied.

```sh
bun run control
bun run control --directory /absolute/project/path
```

Send a prompt to a new session with an optional model and project directory:

```sh
bun run control \
  --prompt "Summarize the current project" \
  --model opencode-go/deepseek-v4.1-flash \
  --directory /absolute/project/path
```

The new session is kept on the local service, and the command prints its ID.

## Manage existing `kir-auto` sessions

Session commands default to the absolute `kir-auto` directory in the sibling
`film-production` checkout. Set `KIR_AUTO_DIRECTORY` to override the default;
`--directory` can select another directory for one command. Existing-session
actions check that the session belongs to the selected directory.

List sessions and inspect one session without changing it:

```sh
bun run control --list-sessions
bun run control --session <session-id>
```

Send a prompt to an existing session, or interrupt its current run:

```sh
bun run control --session <session-id> --prompt "Your message"
bun run control --session <session-id> --interrupt
```

Sending a prompt adds to that session's conversation. Interrupting stops only
the current run. It does not delete the session or disable a recurring
schedule attached to it. Manage that schedule separately if future runs should
stop.

The session ID currently recorded for the `kir-auto` schedule is in
[`kir-auto/AGENTS.md`](../../film-production/.agents/bot/kir-auto/AGENTS.md).
Session IDs can change; use `--list-sessions` to check the live service before
acting.

## Verification record

On 2026-11-18:

- `bun run tsc --noEmit` passed.
- The embedded SDK returned `OPENCODE_GO_SDK_OK` with
  `opencode-go/deepseek-v4.1-flash`.
- The laptop service reported version 2.3.0 and providers
  `deepseek`, `opencode`, `opencode-go`, `ollama`, and `nghimmo`.
- The client sent a test prompt to OpenCode Go and received
  `LOCAL_SERVER_CLIENT_OK`.
- Session listing and metadata lookup found the scheduled `kir-auto` session.
- The smoke check removes its temporary session. The client control command
  keeps prompt sessions so they can be continued later.

## References

- [OpenCode SDK](https://opencode.ai/docs/build/sdk/)
- [OpenCode client](https://opencode.ai/docs/build/client/)
- [Server plugin providers](https://opencode.ai/docs/build/plugins/server/#providers)
