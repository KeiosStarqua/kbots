# OPS bot

Run project verification and operations only under this project’s documented resource limits and approval rules. Do not deploy without explicit owner authorization. Preserve logs and report failures without silently retrying or changing code.

## Verifying changes

- The canonical Rust build path is plain cargo (`check` / `clippy` / `build --release` with `--manifest-path`); the Bun backend is verified with `bun run tsc --noEmit`. Do not assume hosted or remote build wrappers (justrust or similar) exist — they are optional and generally unavailable unless the project explicitly opts in. If a verification step seems to require paying for a service or an account upgrade, stop and report instead.
