# Crate Split Migration — v0.25.0

## What Changed

The single `crit` crate has been split into a 3-crate Cargo workspace:

```
botcrit/
├── Cargo.toml              (workspace root)
├── crates/
│   ├── crit-core/          domain logic: events, storage, projection, SCM
│   ├── crit-cli/           CLI handlers, output formatting, binary entry point
│   └── crit-tui/           terminal UI (review TUI, thread viewer)
```

### Dependency graph

```
crit-cli ──► crit-core
    │
    └──────► crit-tui ──► crit-core
```

`crit-core` is the leaf crate with no internal dependencies.
`crit-tui` depends only on `crit-core`.
`crit-cli` depends on both and produces the `crit` binary.

## What Stays the Same

- **Binary name**: still `crit`
- **All CLI commands**: identical syntax and behavior
- **Output formats**: text, JSON, and pretty output unchanged
- **Event log format**: `.crit/reviews/{id}/events.jsonl` is the same v2 format
- **Projection database**: SQLite schema unchanged
- **Configuration**: `.critignore`, `.crit/version`, agent identity — all the same
- **Feature flags**: `otel` feature still available on `crit-cli`

## For Users

No behavior change. The binary is still called `crit` and every command works exactly as before. Upgrade by building from the workspace root:

```bash
cargo install --path crates/crit-cli
```

Or from the workspace root:

```bash
cargo build --release
# binary at target/release/crit
```

## For Developers

### Import paths

Domain types now live in `crit_core`:

```rust
use crit_core::events::{Event, ReviewCreated, ThreadCreated};
use crit_core::projection::{ProjectionDb, ReviewState};
use crit_core::log::EventLog;
use crit_core::core::CritServices;
```

CLI-specific types live in `crit_cli`:

```rust
use crit_cli::cli::commands;
use crit_cli::output::Formatter;
```

TUI types live in `crit_tui`:

```rust
use crit_tui::review_tui::ReviewTui;
```

### Workspace dependencies

Shared dependencies are declared once in the workspace `Cargo.toml` under `[workspace.dependencies]` and referenced with `.workspace = true` in each crate. Add new deps to the workspace root first.

### Lints

Clippy lints are configured at the workspace level under `[workspace.lints.clippy]`. All crates inherit them with `[lints] workspace = true`.

### Adding a new crate

1. Create `crates/crit-newcrate/` with its `Cargo.toml` (use `version.workspace = true`, `edition.workspace = true`, `license.workspace = true`)
2. Add it to the `members` list in the workspace `Cargo.toml`
3. Add `[lints] workspace = true` to inherit lint config
