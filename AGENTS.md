# AGENTS.md

## Project Context

- Resteyes is a Rust application intended to become a minimal, cross-platform
  Safe Eyes replacement.
- Keep Cargo as the source of truth for Rust builds. Use `make` only as a thin
  task runner around Cargo and Nix.
- Claude-specific configuration is intentionally out of scope for now.

## Commands

- Run the app: `make run`
- Format code: `make fmt`
- Check formatting: `make fmt-check`
- Lint: `make lint`
- Test: `make test`
- Full local check: `make check`
- Build: `make build`

## Expectations

- Formatting is automatic for Codex and Claude edits via their agent hooks.
- Do not run `make fmt`, `cargo fmt`, or other formatter commands after editing.
- Run `make check` before considering a change complete.
- Do not add production dependencies without a clear reason.
- Keep platform-specific backend work out of the initial scaffold unless asked.

## Build Approach

- Build Resteyes one step at a time.
- Treat each meaningful step as a small, reviewable increment with its own
  working program or verified behavior.
- Read our current progress from ./build-notes/progress.md
- Write notes for each step in `./build-notes/$step.md`, where `$step` is a
  short kebab-case name such as `hello-world`, `break-scheduler`, or
  `x11-screen-blanking`.
- Use step notes to record decisions, tradeoffs, commands run, open questions,
  and follow-up work that should not be lost between sessions.
- Fold cleanup, refinement work, and fixes into the existing step note when
  they clarify, simplify, or correct that step, instead of creating a separate
  cleanup note. Create a new step note only for a distinct reviewable
  increment.
- Keep notes factual and concise; update them as implementation details change.
- When something is done, update the progress file in ./build-notes/progress.md
- Commit each completed step as its own focused git commit.
