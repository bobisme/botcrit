# botcrit-ui Repository Transition Plan

**Decision date:** 2026-02-27
**Status:** Approved
**Owner:** botcrit-dev

## Summary

botcrit-ui continues as the **primary interactive TUI** for crit reviews. crit's built-in TUI
is deprecated and will be removed. The crate split (crit-core/crit-cli/crit-tui) is deferred
until botcrit-ui's needs drive it.

## Current State

| Component | Description |
|-----------|-------------|
| **crit** (this repo) | Monolithic binary: CLI + built-in ratatui TUI (~800 LOC). Agent-first tool. |
| **botcrit-ui** (separate repo) | Standalone TUI binary (`crit-ui`). ftui-based, Elm architecture, GitHub-style diff rendering, themes, tests. Shells out to `crit --json` for data. |

Both are actively maintained. The built-in TUI duplicates effort and receives less investment
than botcrit-ui's richer UX.

## Decision: Keep botcrit-ui as Primary UI

### Rationale

1. **botcrit-ui has the better UX.** Side-by-side diffs, syntax highlighting, theme support,
   and Elm-style architecture make it more maintainable and feature-rich.

2. **Shelling out to `crit --json` works.** The subprocess boundary adds latency but is
   functionally correct. Premature optimization (crate split) failed — the attempted
   crit-core extraction was never merged and the workspaces were destroyed.

3. **Separation enables independent release cadence.** botcrit-ui can ship UI improvements
   without waiting for crit releases, and vice versa.

4. **Agent workflows don't need TUI.** Agents use `crit` CLI exclusively. The TUI serves
   human reviewers. Keeping it separate doesn't affect agent workflows.

### What changes

| Action | Timeline | Details |
|--------|----------|---------|
| Deprecate crit's built-in TUI | Now | Add deprecation notice to `crit ui` command output. Point users to `crit-ui` (botcrit-ui). |
| Remove built-in TUI | Next minor release | Remove `src/tui/`, ratatui/crossterm deps. Keeps crit lean. |
| Add `crit ui` launcher stub | Same release | `crit ui` checks for `crit-ui` binary on PATH, runs it if found, prints install instructions if not. |
| Document canonical path | Now | README, AGENTS.md updated to point to botcrit-ui for interactive use. |

### What stays the same

- botcrit-ui remains a separate repository with its own release cycle.
- botcrit-ui shells out to `crit --json` for data access (no crate split needed).
- crit focuses on CLI, event store, projection, and agent-facing features.

## Future: Crate Split (Deferred)

The crit-core/crit-cli/crit-tui split remains a valid long-term architecture goal but is
**not a prerequisite** for the transition. It should only be pursued when:

1. botcrit-ui's shell-out latency becomes a measurable bottleneck (profile first).
2. Multiple consumers need programmatic access to crit's projection/event store.
3. A dedicated effort with proper merge-to-default discipline is available.

The previous attempt (bn-2gdf through bn-fjm9) completed in workspaces but was never merged
due to workspace staleness. Lessons learned:
- Merge early and often — don't let workspace epochs drift.
- Test the merged result, not just individual crates.
- The split is mechanical but touches every module; budget accordingly.

## Canonical Contribution Path

| Want to... | Go to... |
|------------|----------|
| Report a crit CLI bug | botcrit (this repo) |
| Report a TUI bug | botcrit-ui repo |
| Add a new CLI command | botcrit (this repo) |
| Improve review UX | botcrit-ui repo |
| Change event format | botcrit (this repo) — coordinate with botcrit-ui |
| Add a new output format | botcrit (this repo) — botcrit-ui consumes `--json` |

## Tracking: Deferred botcrit-ui Work

The following work items live in botcrit-ui's scope and should be tracked there:

1. **Theme system stabilization** — botcrit-ui has themes; ensure they work with crit's output.
2. **Offline review rendering** — render reviews without live crit process.
3. **Keyboard shortcut parity** — match crit's built-in TUI keybindings for migration ease.
4. **Install instructions** — document how to install crit-ui alongside crit.

## Risk Assessment

- **Low risk.** This is a documentation/planning decision. No code changes beyond deprecation
  notices and eventual TUI removal (which reduces maintenance burden).
- The TUI removal is additive-safe: agents don't use TUI, humans can install botcrit-ui.
- Reversible: if botcrit-ui development stalls, the built-in TUI can be restored from git history.
