# Agent Instructions

Every agent working in this repository must read this file first.

## Required First Steps

1. Read `AGENTS.md`.
2. Read `project.md` completely to get current project bearings.
3. Read any task-specific docs linked from `project.md` before changing related code.
4. Inspect the relevant source files directly before editing.

Do not rely only on memory, summaries, or old handoff notes. `project.md` is the living project map.

## Required Project Updates

Whenever you make a meaningful change, update `project.md` in the same task. This includes:

- New, moved, renamed, or deleted files.
- New, moved, renamed, or deleted functions/components/commands.
- Behavior changes in rewrite, capture, paste, auth, billing, settings, or bubble flows.
- New known gaps, resolved gaps, test findings, or release status changes.
- Changes to local data, config, environment variables, or build/development commands.

For small code-only changes, at least add or update a dated note in `Recent Updates` if the behavior changed.

## Working Style

- Prefer existing architecture and patterns over new abstractions.
- Keep changes scoped to the task.
- Do not revert user changes unless explicitly asked.
- Verify with the narrowest useful build/test command.
- If verification cannot be run, record that clearly in your final response and update `project.md` when it affects project status.

## Key Orientation Docs

- `project.md`: living source of truth for architecture, function index, and current status.
- `README.md`: high-level app overview and setup commands.
- `roadmap.md`: billing/SaaS roadmap.
- `roadmap-mac.md`: macOS roadmap.
- `v1.1.0-selection-bubble.md`: Windows passive selection bubble handoff and debugging history.
