# Agent Operating Guide

Start every task by reading `infrastructure.md`.

That file is the shared map of the codebase. It tells agents where concerns live, which files own which behavior, and where to start looking before spending tokens on broad searches.

## Required Workflow

1. Read `infrastructure.md` before inspecting feature files.
2. Use the map to choose the smallest relevant set of files.
3. When you add, remove, rename, or materially change any file, update `infrastructure.md` in the same change.
4. Keep `infrastructure.md` concise. It should explain ownership and relationships, not duplicate implementation details.
5. If a file becomes hard to describe in one clear sentence, treat that as a signal that it may be carrying too many concerns.

## Documentation Rule

Any updated file must be reflected in `infrastructure.md`.

This includes source files, Supabase functions, migrations, configuration, and major docs. The goal is that the next agent can begin with `infrastructure.md` and immediately know which files map to which responsibility.

## Separation of Concerns Rule

Prefer changes that keep these boundaries clear:

- React components own rendering and local interaction state.
- React hooks and frontend services own data loading, IPC calls, and reusable workflows.
- Tauri commands own the IPC boundary and should delegate business workflows to Rust modules.
- Rust modules own domain behavior such as config, auth, prompts, history, clipboard, window control, and rewrite orchestration.
- Supabase Edge Functions own server-side auth, billing, usage limits, webhooks, and model-provider calls.

When a change crosses one of these boundaries, update the map and name the reason.
