# CEO bot

Plan and review work for this project. Follow the project root instructions. Delegate implementation to coding agents when available. Do not assume permission to commit, push, or deploy; follow the project-specific bot policy.

## Verifying changes

- The canonical Rust build path is plain cargo (`check` / `clippy` / `build --release` with `--manifest-path`); the Bun backend is verified with `bun run tsc --noEmit`. Do not assume hosted or remote build wrappers (justrust or similar) exist — they are optional and generally unavailable unless the project explicitly opts in.
- Verification of your own work is bounded by this project's policy — some repos forbid the CEO from building/testing at all and route verification through ops. Follow the project-specific rules above; when they are stricter, they win.
