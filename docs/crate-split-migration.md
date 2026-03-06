# Crate Split Migration — v0.25.0

## What Changed

The single `seal` crate has been split into a 3-crate Cargo workspace:

```
botseal/
├── Cargo.toml              (workspace root)
├── crates/
│   ├── seal-core/          domain logic: events, storage, projection, SCM
│   ├── seal-cli/           CLI handlers, output formatting, binary entry point
│   └── seal-tui/           terminal UI (review TUI, thread viewer)
```

### Dependency graph

```
seal-cli ──► seal-core
    │
    └──────► seal-tui ──► seal-core
```

`seal-core` is the leaf crate with no internal dependencies.
`seal-tui` depends only on `seal-core`.
`seal-cli` depends on both and produces the `seal` binary.

## What Stays the Same

- **Binary name**: still `seal`
- **All CLI commands**: identical syntax and behavior
- **Output formats**: text, JSON, and pretty output unchanged
- **Event log format**: `.seal/reviews/{id}/events.jsonl` is the same v2 format
- **Projection database**: SQLite schema unchanged
- **Configuration**: `.sealignore`, `.seal/version`, agent identity — all the same
- **Feature flags**: `otel` feature still available on `seal-cli`

## For Users

No behavior change. The binary is still called `seal` and every command works exactly as before. Upgrade by building from the workspace root:

```bash
cargo install --path crates/seal-cli
```

Or from the workspace root:

```bash
cargo build --release
# binary at target/release/seal
```

## For Developers

### Import paths

Domain types now live in `seal_core`:

```rust
use seal_core::events::{Event, ReviewCreated, ThreadCreated};
use seal_core::projection::{ProjectionDb, ReviewState};
use seal_core::log::EventLog;
use seal_core::core::SealServices;
```

CLI-specific types live in `seal_cli`:

```rust
use seal_cli::cli::commands;
use seal_cli::output::Formatter;
```

TUI types live in `seal_tui`:

```rust
use seal_tui::review_tui::ReviewTui;
```

### Workspace dependencies

Shared dependencies are declared once in the workspace `Cargo.toml` under `[workspace.dependencies]` and referenced with `.workspace = true` in each crate. Add new deps to the workspace root first.

### Lints

Clippy lints are configured at the workspace level under `[workspace.lints.clippy]`. All crates inherit them with `[lints] workspace = true`.

### Adding a new crate

1. Create `crates/seal-newcrate/` with its `Cargo.toml` (use `version.workspace = true`, `edition.workspace = true`, `license.workspace = true`)
2. Add it to the `members` list in the workspace `Cargo.toml`
3. Add `[lints] workspace = true` to inherit lint config
