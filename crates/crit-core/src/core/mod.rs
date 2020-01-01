//! Service layer for crit-core.
//!
//! Provides typed, high-level APIs for review, thread, comment, inbox, and sync
//! operations. The service layer encapsulates projection database management and
//! event log appends behind a clean interface.
//!
//! # Usage
//!
//! ```no_run
//! use std::path::Path;
//! use crit_core::core::{CoreContext, CritServices};
//!
//! let ctx = CoreContext::new(
//!     Path::new("/repo"),
//!     Path::new("/repo/.crit/index.db"),
//! ).unwrap();
//!
//! let services = ctx.services().unwrap();
//! let reviews = services.reviews().list(None, None).unwrap();
//! ```

pub mod comments;
pub mod errors;
pub mod inbox;
pub mod reviews;
pub mod sync;
pub mod threads;

pub use errors::{CoreError, CoreResult};

use std::path::{Path, PathBuf};

use crate::projection::{sync_from_review_logs, ProjectionDb};
use crate::version::require_v2;

/// Context for crit-core services.
///
/// Holds the paths needed to locate event logs and the projection database.
/// Create one per operation or hold for the duration of a session.
#[derive(Debug, Clone)]
pub struct CoreContext {
    /// Path to the repository root (parent of `.crit/`).
    crit_root: PathBuf,
    /// Path to the projection database file.
    db_path: PathBuf,
}

impl CoreContext {
    /// Create a new core context.
    ///
    /// Validates that the crit repository is initialized and uses v2 format.
    ///
    /// # Arguments
    /// * `crit_root` - Path to the repository root (parent of `.crit/`)
    /// * `db_path` - Path to the SQLite projection database
    pub fn new(crit_root: &Path, db_path: &Path) -> CoreResult<Self> {
        let crit_dir = crit_root.join(".crit");
        if !crit_dir.exists() {
            return Err(CoreError::NotInitialized {
                path: crit_root.display().to_string(),
            });
        }

        require_v2(crit_root).map_err(|_| CoreError::V1NeedsMigration)?;

        Ok(Self {
            crit_root: crit_root.to_path_buf(),
            db_path: db_path.to_path_buf(),
        })
    }

    /// Path to the repository root.
    #[must_use]
    pub fn crit_root(&self) -> &Path {
        &self.crit_root
    }

    /// Path to the projection database.
    #[must_use]
    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    /// Open the projection database, initialize its schema, and sync from event logs.
    ///
    /// This is the standard way to get a ready-to-query projection.
    pub fn open_and_sync(&self) -> CoreResult<ProjectionDb> {
        let db = ProjectionDb::open(&self.db_path).map_err(CoreError::Internal)?;
        db.init_schema().map_err(CoreError::Internal)?;
        sync_from_review_logs(&db, &self.crit_root).map_err(CoreError::Internal)?;
        Ok(db)
    }

    /// Create a `CritServices` instance backed by this context.
    ///
    /// Opens and syncs the projection database.
    pub fn services(&self) -> CoreResult<CritServices> {
        let db = self.open_and_sync()?;
        Ok(CritServices {
            ctx: self.clone(),
            db,
        })
    }
}

/// Facade providing all crit service APIs.
///
/// Owns a synced projection database and provides access to domain-specific
/// service objects for reviews, threads, comments, inbox, and sync.
pub struct CritServices {
    ctx: CoreContext,
    db: ProjectionDb,
}

impl CritServices {
    /// Access review operations.
    #[must_use]
    pub fn reviews(&self) -> reviews::ReviewService<'_> {
        reviews::ReviewService::new(&self.ctx, &self.db)
    }

    /// Access thread operations.
    #[must_use]
    pub fn threads(&self) -> threads::ThreadService<'_> {
        threads::ThreadService::new(&self.ctx, &self.db)
    }

    /// Access comment operations.
    #[must_use]
    pub fn comments(&self) -> comments::CommentService<'_> {
        comments::CommentService::new(&self.ctx, &self.db)
    }

    /// Access inbox operations.
    #[must_use]
    pub fn inbox(&self) -> inbox::InboxService<'_> {
        inbox::InboxService::new(&self.db)
    }

    /// Access sync operations.
    #[must_use]
    pub fn sync(&self) -> sync::SyncService<'_> {
        sync::SyncService::new(&self.ctx, &self.db)
    }

    /// Get a reference to the underlying projection database.
    ///
    /// Useful for advanced queries not covered by the service layer.
    #[must_use]
    pub fn db(&self) -> &ProjectionDb {
        &self.db
    }

    /// Get a reference to the core context.
    #[must_use]
    pub fn context(&self) -> &CoreContext {
        &self.ctx
    }
}
