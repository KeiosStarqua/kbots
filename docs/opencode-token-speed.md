# Measuring model token speed from OpenCode's local database

Reference for a future kbots feature that reports tokens/s, time to first token
(TTFT), and build time per assistant message or per model. No code is
implemented yet — this document records the verified method only.

## Where the data lives

OpenCode stores sessions in a SQLite database:

```sh
opencode debug paths db
# default on this machine: ~/.local/share/opencode/opencode.db
```

The database uses WAL mode (`opencode.db-wal`, `opencode.db-shm`). Open it
**read-only** so a measurement can never mutate session state:

```sh
sqlite3 -readonly ~/.local/share/opencode/opencode.db "<query>"
```

Relevant tables:

- `session_message` — one row per message. The `data` column is JSON with model,
  token usage, and timestamps.
- `session_v2` — session aggregates (`tokens_output`, `tokens_reasoning`,
  `time_created`, `time_updated`, `time_busy`, `model`, ...).
- `session_part` exists in the schema but was empty on the verified machine;
  do not rely on it.

Fields used from `session_message.data` for an assistant message:

| JSON path | Meaning |
|-----------|---------|
| `$.model.id` / `$.providerID` / `$.variant` | Model that produced the message |
| `$.tokens.output` | Output tokens (excludes reasoning) |
| `$.tokens.reasoning` | Reasoning tokens |
| `$.time.created` | Turn start (epoch ms) |
| `$.time.firstToken` | First streamed token (epoch ms) |
| `$.time.streamed` | Last streamed token (epoch ms) |
| `$.time.completed` | Turn complete (epoch ms) |

## Verified definitions

These formulas reproduce the numbers shown in the OpenCode UI exactly:

```text
ttft_ms     = firstToken - created
stream_ms   = streamed - firstToken
build_ms    = completed - created
tokens      = tokens.output + tokens.reasoning
tokens_per_s = tokens / (stream_ms / 1000)
```

- Throughput counts **reasoning tokens too**; using `tokens.output` alone gives
  a lower number than the UI.
- Throughput **excludes TTFT** (it starts at `firstToken`).
- Build time includes TTFT and any reasoning/tool time, so it is not the same
  as the streaming duration.

## Queries

Per message (read-only, parameter is the session ID):

```sql
SELECT json_extract(data, '$.model.id') AS model,
       json_extract(data, '$.tokens.output') AS output_tokens,
       json_extract(data, '$.tokens.reasoning') AS reasoning_tokens,
       round((json_extract(data, '$.time.firstToken') -
              json_extract(data, '$.time.created')) / 1000.0, 2) AS ttft_s,
       round((json_extract(data, '$.time.streamed') -
              json_extract(data, '$.time.firstToken')) / 1000.0, 2) AS stream_s,
       round((json_extract(data, '$.tokens.output') +
              json_extract(data, '$.tokens.reasoning')) /
             ((json_extract(data, '$.time.streamed') -
               json_extract(data, '$.time.firstToken')) / 1000.0), 1) AS tok_s,
       round((json_extract(data, '$.time.completed') -
              json_extract(data, '$.time.created')) / 1000.0, 1) AS build_s
FROM session_message
WHERE session_id = :session_id
  AND type = 'assistant'
  AND json_extract(data, '$.time.completed') IS NOT NULL
  AND json_extract(data, '$.time.streamed') > json_extract(data, '$.time.firstToken')
ORDER BY seq;
```

Aggregate by model for one session:

```sql
WITH m AS (
  SELECT json_extract(data, '$.model.id') AS model,
         json_extract(data, '$.tokens.output') +
           json_extract(data, '$.tokens.reasoning') AS tok,
         (json_extract(data, '$.time.firstToken') -
           json_extract(data, '$.time.created')) / 1000.0 AS ttft,
         (json_extract(data, '$.time.streamed') -
           json_extract(data, '$.time.firstToken')) / 1000.0 AS stream_s
  FROM session_message
  WHERE session_id = :session_id
    AND type = 'assistant'
    AND json_extract(data, '$.time.completed') IS NOT NULL
    AND json_extract(data, '$.time.streamed') > json_extract(data, '$.time.firstToken')
)
SELECT model, count(*) AS samples,
       round(min(tok / stream_s), 1) AS min_tps,
       round(avg(tok / stream_s), 1) AS avg_tps,
       round(max(tok / stream_s), 1) AS max_tps,
       round(avg(ttft), 2) AS avg_ttft_s
FROM m
GROUP BY model
ORDER BY avg_tps DESC;
```

Session IDs come from `bun run control --list-sessions` or from the TUI.

## Verification record (2026-11-21)

The formulas were checked against UI `Build · <model> · <time> · <tok/s>` lines
recorded in a live session. Exact matches:

| Model | Computed | UI line |
|-------|----------|---------|
| longcat-2.5-free | 40 tokens / 0.708 s = 56.5 tok/s, 4.616 s | `4.6s · 56.5 tok/s` |
| longcat-2.5-free | 251 tokens / 4.492 s = 55.9 tok/s, 8.941 s | `9.0s · 55.9 tok/s` |
| hy4-flash-free | 231 tokens / 1.440 s = 160.4 tok/s | `5.0s · 160.4 tok/s` |
| beam-free | 628 tokens / 6.283 s = 99.95 tok/s, 7.877 s | `7.9s · 100.0 tok/s` |
| beam-free | 144 tokens / 0.964 s = 149.4 tok/s, 2.074 s | `2.1s · 149.4 tok/s` |
| deepseek-flash | 62 tokens / 0.547 s = 113.3 tok/s, 1.489 s | `1.5s · 113.3 tok/s` |
| iceman-contributor-free | 185 tokens / 1.783 s = 103.8 tok/s | `6.4s · 103.8 tok/s` |

Aggregate over that session at verification time (19 hy4 samples, 2 beam, 1 deepseek, 1 iceman,
2 longcat): hy4 avg 156.3 tok/s (101.4–196.8, TTFT 3.40 s), beam 124.7
(100.0–149.4, TTFT 1.33 s), deepseek 113.4 (TTFT 0.92 s), iceman 103.8
(TTFT 4.59 s), longcat 56.2 (55.9–56.5, TTFT 4.15 s).

## Caveats

- These are message-level values from OpenCode's own timing data, not an
  independent stream measurement. The UI's tok/s includes reasoning tokens.
- TTFT varies heavily with provider and load; compare it separately from
  throughput. Short answers often show a worse tok/s because TTFT dominates.
- Open a copy or use `-readonly`; never write to the live database or its WAL.
- Do not print session content or credentials from the database in logs.

## Implementation notes for kbots (later)

- Add a **read-only** adapter under `src/backend/opencode/`; keep it separate
  from session control. The public surface should be
  `measure(sessionId, dbPath?) -> MessageSpeed[]` plus an aggregate-by-model
  helper, with the SQLite path resolved through `opencode debug paths db` and
  `OPENCODE_DB_PATH` as override (same convention as `smoke.ts`).
- Model the work with Effect v4: typed failures for "database missing",
  "session not found", and malformed JSON; no ad-hoc promise/error handling.
- Pass `sessionId` as a bound parameter, never by string interpolation.
- If exposed to clients, add it to the daemon IPC contract in
  `docs/kbots-ipc.md` and keep it read-only. Session-scope the query the same
  way as other existing-session actions.
- Verification for that change: `bun run tsc --noEmit`, plus a fixture DB test
  that asserts the verified numbers in this document.

## References

- [OpenCode troubleshooting and local data locations](https://opencode.ai/docs/troubleshooting/)
- [`docs/opencode-control.md`](opencode-control.md) — SDK/service setup and session commands
