//! CLI command definitions and handlers.

use clap::{Parser, Subcommand};

pub mod commands;

/// Agent-centric distributed code review tool for jj
#[derive(Parser, Debug)]
#[command(name = "crit")]
#[command(author, version, about, long_about = None)]
pub struct Cli {
    /// Output format: TOON (default) or JSON
    #[arg(long, global = true)]
    pub json: bool,

    /// Override agent identity (default: $CRIT_AGENT or $BOTBUS_AGENT or $USER)
    #[arg(long, global = true)]
    pub author: Option<String>,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Initialize a new .crit directory in the current repository
    Init,

    /// Health check - verify jj, .crit/, and sync status
    Doctor,

    /// Manage AGENTS.md integration
    #[command(subcommand)]
    Agents(AgentsCommands),

    /// Manage code reviews
    #[command(subcommand)]
    Reviews(ReviewsCommands),

    /// Manage comment threads
    #[command(subcommand)]
    Threads(ThreadsCommands),

    /// Manage comments
    #[command(subcommand)]
    Comments(CommentsCommands),

    /// Show status of reviews
    Status {
        /// Review ID (optional - shows all if omitted)
        review_id: Option<String>,

        /// Show only unresolved threads
        #[arg(long)]
        unresolved_only: bool,
    },

    /// Show diff for a review
    Diff {
        /// Review ID
        review_id: String,
    },

    /// Interactive UI for browsing reviews
    Ui,
}

// ============================================================================
// Agents subcommands
// ============================================================================

#[derive(Subcommand, Debug)]
pub enum AgentsCommands {
    /// Insert crit instructions into AGENTS.md
    Init,
    /// Print crit instructions to stdout
    Show,
}

// ============================================================================
// Reviews subcommands
// ============================================================================

#[derive(Subcommand, Debug)]
pub enum ReviewsCommands {
    /// Create a new review for the current change
    Create {
        /// Review title
        #[arg(long)]
        title: String,

        /// Optional description
        #[arg(long = "description", visible_alias = "desc")]
        description: Option<String>,
    },

    /// List reviews
    List {
        /// Filter by status
        #[arg(long)]
        status: Option<ReviewStatus>,

        /// Filter by author
        #[arg(long)]
        author: Option<String>,

        /// Show only reviews where I am a requested reviewer
        #[arg(long)]
        needs_review: bool,

        /// Show only reviews with unresolved threads
        #[arg(long)]
        has_unresolved: bool,
    },

    /// Show review details
    Show {
        /// Review ID
        review_id: String,
    },

    /// Request reviewers for a review
    Request {
        /// Review ID
        review_id: String,

        /// Comma-separated list of reviewers
        #[arg(long = "reviewers", visible_alias = "reviewer")]
        reviewers: String,
    },

    /// Approve a review
    Approve {
        /// Review ID
        review_id: String,
    },

    /// Abandon a review
    Abandon {
        /// Review ID
        review_id: String,

        /// Reason for abandoning
        #[arg(long)]
        reason: Option<String>,
    },

    /// Mark a review as merged
    Merge {
        /// Review ID
        review_id: String,

        /// Final commit hash (auto-detected from @ if not provided)
        #[arg(long)]
        commit: Option<String>,
    },
}

#[derive(Debug, Clone, clap::ValueEnum)]
pub enum ReviewStatus {
    Open,
    Approved,
    Merged,
    Abandoned,
}

// ============================================================================
// Threads subcommands
// ============================================================================

#[derive(Subcommand, Debug)]
pub enum ThreadsCommands {
    /// Create a new comment thread
    Create {
        /// Review ID
        review_id: String,

        /// File path
        #[arg(long)]
        file: String,

        /// Line or range (e.g., "42" or "10-20")
        #[arg(long)]
        lines: String,
    },

    /// List threads for a review
    List {
        /// Review ID
        review_id: String,

        /// Filter by status
        #[arg(long)]
        status: Option<ThreadStatus>,

        /// Filter by file path
        #[arg(long)]
        file: Option<String>,
    },

    /// Show thread details with context
    Show {
        /// Thread ID
        thread_id: String,

        /// Number of context lines (default: 3)
        #[arg(long, default_value = "3")]
        context: u32,

        /// Hide code context (shorthand for --context 0)
        #[arg(long)]
        no_context: bool,

        /// Show context at current commit instead of original
        #[arg(long)]
        current: bool,

        /// Display as human-readable conversation with timestamps
        #[arg(long)]
        conversation: bool,

        /// Disable colored output
        #[arg(long)]
        no_color: bool,
    },

    /// Resolve a thread
    Resolve {
        /// Thread IDs (can specify multiple, or use --all)
        thread_ids: Vec<String>,

        /// Resolve all threads matching criteria
        #[arg(long)]
        all: bool,

        /// Filter by file (with --all)
        #[arg(long)]
        file: Option<String>,

        /// Reason for resolving
        #[arg(long)]
        reason: Option<String>,
    },

    /// Reopen a resolved thread
    Reopen {
        /// Thread ID
        thread_id: String,

        /// Reason for reopening
        #[arg(long)]
        reason: Option<String>,
    },
}

#[derive(Debug, Clone, clap::ValueEnum)]
pub enum ThreadStatus {
    Open,
    Resolved,
}

// ============================================================================
// Comments subcommands
// ============================================================================

#[derive(Subcommand, Debug)]
pub enum CommentsCommands {
    /// Add a comment to a thread
    Add {
        /// Thread ID
        thread_id: String,

        /// Comment message (positional or use --message)
        #[arg(long = "message", visible_alias = "msg")]
        message: Option<String>,

        /// Comment message (positional argument)
        #[arg(value_name = "MESSAGE")]
        message_positional: Option<String>,
    },

    /// List comments in a thread
    List {
        /// Thread ID
        thread_id: String,
    },
}
