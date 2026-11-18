# OpenCode SDK provider smoke check

Install with `bun install`, then run `bun run smoke`.

The check uses `@opencode/sdk` with a plugin, reads provider metadata, sends one
prompt, prints the assistant response, and removes its temporary session. It
uses the database path reported by `opencode debug paths db` so the embedded
host can access the same saved provider connection as the CLI. Set
`OPENCODE_DB_PATH` to override that path.

`bun run control` connects to the laptop's registered OpenCode service with
`@opencode/client`. It prints the server version and available providers.
`Service.ensure()` reuses the service when it is already running and starts it
otherwise. To send a prompt and keep its session:

```sh
bun run control --prompt "Summarize the current project"
```

Add `--model provider/model` or `--directory /absolute/project/path` to choose
the model or project. The service authentication header is applied internally
and never printed.

Session controls default to the `kir-auto` directory in the sibling
`film-production` checkout. Set `KIR_AUTO_DIRECTORY` to override it:

```sh
bun run control --list-sessions
bun run control --session ses_e2ea7ca94ffefSlt7Ok3z3OpfL
bun run control --session ses_e2ea7ca94ffefSlt7Ok3z3OpfL \
  --prompt "Report the current step and wait"
bun run control --session ses_e2ea7ca94ffefSlt7Ok3z3OpfL --interrupt
```

Interrupt stops only the current run. The recurring schedule configured for a
session remains active and can start another run later.

See [`docs/opencode-control.md`](docs/opencode-control.md) for the full
architecture, setup, provider behavior, and session-control reference.

The default model is `opencode/inkling-2-small-free`. To test OpenCode Go, run:

```sh
OPENCODE_MODEL=opencode-go/deepseek-v4.1-flash bun run smoke
```

Set `OPENCODE_MODEL` to any available `provider/model` ID to select another
model.
