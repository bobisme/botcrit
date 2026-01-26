//! crit - Agent-centric distributed code review tool for jj

use anyhow::Result;
use clap::Parser;
use std::env;

use crit::cli::commands::{
    run_agents_init, run_agents_show, run_comment, run_comments_add, run_comments_list, run_diff,
    run_doctor, run_init, run_reviews_abandon, run_reviews_approve, run_reviews_create,
    run_reviews_list, run_reviews_merge, run_reviews_request, run_reviews_show, run_status,
    run_threads_create, run_threads_list, run_threads_reopen, run_threads_resolve,
    run_threads_show,
};
use crit::cli::{
    AgentsCommands, Cli, Commands, CommentsCommands, ReviewsCommands, ThreadsCommands,
};
use crit::jj::resolve_repo_root;
use crit::output::OutputFormat;

fn main() -> Result<()> {
    let cli = Cli::parse();

    // We need two paths:
    // 1. crit_root: where .crit/ lives (main repo root, shared across workspaces)
    // 2. workspace_root: where jj commands run (current workspace, for @ resolution)
    let workspace_root = env::current_dir()?;
    let crit_root = resolve_repo_root(&workspace_root).unwrap_or_else(|_| workspace_root.clone());

    // Determine output format
    let format = if cli.json {
        OutputFormat::Json
    } else {
        OutputFormat::Toon
    };

    match cli.command {
        Commands::Init => {
            run_init(&crit_root)?;
        }

        Commands::Doctor => {
            run_doctor(&crit_root, format)?;
        }

        Commands::Agents(cmd) => match cmd {
            AgentsCommands::Init => {
                run_agents_init(&crit_root)?;
            }
            AgentsCommands::Show => {
                run_agents_show()?;
            }
        },

        Commands::Reviews(cmd) => match cmd {
            ReviewsCommands::Create { title, description } => {
                run_reviews_create(
                    &crit_root,
                    &workspace_root,
                    title,
                    description,
                    cli.author.as_deref(),
                    format,
                )?;
            }
            ReviewsCommands::List {
                status,
                author,
                needs_review,
                has_unresolved,
            } => {
                let status_str = status.map(|s| match s {
                    crit::cli::ReviewStatus::Open => "open",
                    crit::cli::ReviewStatus::Approved => "approved",
                    crit::cli::ReviewStatus::Merged => "merged",
                    crit::cli::ReviewStatus::Abandoned => "abandoned",
                });
                // For --needs-review, use the subcommand --author as identity (if provided),
                // falling back to global --author, then env vars.
                // When --needs-review is used, --author should NOT also filter by review author.
                let (author_filter, needs_reviewer) = if needs_review {
                    // Use --author for identity, not filtering
                    let identity = author.as_deref().or(cli.author.as_deref());
                    (None, Some(crit::events::get_agent_identity(identity)))
                } else {
                    // Normal case: --author filters by review author
                    (author.as_deref().map(String::from), None)
                };
                run_reviews_list(
                    &crit_root,
                    status_str,
                    author_filter.as_deref(),
                    needs_reviewer.as_deref(),
                    has_unresolved,
                    format,
                )?;
            }
            ReviewsCommands::Show { review_id } => {
                run_reviews_show(&crit_root, &review_id, format)?;
            }
            ReviewsCommands::Request {
                review_id,
                reviewers,
            } => {
                run_reviews_request(
                    &crit_root,
                    &review_id,
                    &reviewers,
                    cli.author.as_deref(),
                    format,
                )?;
            }
            ReviewsCommands::Approve { review_id } => {
                run_reviews_approve(&crit_root, &review_id, cli.author.as_deref(), format)?;
            }
            ReviewsCommands::Abandon { review_id, reason } => {
                run_reviews_abandon(
                    &crit_root,
                    &review_id,
                    reason,
                    cli.author.as_deref(),
                    format,
                )?;
            }
            ReviewsCommands::Merge { review_id, commit } => {
                run_reviews_merge(
                    &crit_root,
                    &workspace_root,
                    &review_id,
                    commit,
                    cli.author.as_deref(),
                    format,
                )?;
            }
        },

        Commands::Threads(cmd) => match cmd {
            ThreadsCommands::Create {
                review_id,
                file,
                lines,
            } => {
                run_threads_create(
                    &crit_root,
                    &workspace_root,
                    &review_id,
                    &file,
                    &lines,
                    cli.author.as_deref(),
                    format,
                )?;
            }
            ThreadsCommands::List {
                review_id,
                status,
                file,
            } => {
                let status_str = status.map(|s| match s {
                    crit::cli::ThreadStatus::Open => "open",
                    crit::cli::ThreadStatus::Resolved => "resolved",
                });
                run_threads_list(&crit_root, &review_id, status_str, file.as_deref(), format)?;
            }
            ThreadsCommands::Show {
                thread_id,
                context,
                no_context,
                current,
                conversation,
                no_color,
            } => {
                // --no-context overrides --context
                let context_lines = if no_context { 0 } else { context };
                run_threads_show(
                    &crit_root,
                    &workspace_root,
                    &thread_id,
                    context_lines,
                    current,
                    conversation,
                    !no_color, // use_color
                    format,
                )?;
            }
            ThreadsCommands::Resolve {
                thread_ids,
                all,
                file,
                reason,
            } => {
                run_threads_resolve(
                    &crit_root,
                    &thread_ids,
                    all,
                    file.as_deref(),
                    reason,
                    cli.author.as_deref(),
                    format,
                )?;
            }
            ThreadsCommands::Reopen { thread_id, reason } => {
                run_threads_reopen(
                    &crit_root,
                    &thread_id,
                    reason,
                    cli.author.as_deref(),
                    format,
                )?;
            }
        },

        Commands::Comments(cmd) => match cmd {
            CommentsCommands::Add {
                thread_id,
                message,
                message_positional,
            } => {
                // Support both --message and positional argument
                let msg = message.or(message_positional).ok_or_else(|| {
                    anyhow::anyhow!("Message is required (use --message or provide as argument)")
                })?;
                run_comments_add(&crit_root, &thread_id, &msg, cli.author.as_deref(), format)?;
            }
            CommentsCommands::List { thread_id } => {
                run_comments_list(&crit_root, &thread_id, format)?;
            }
        },

        Commands::Status {
            review_id,
            unresolved_only,
        } => {
            run_status(
                &crit_root,
                &workspace_root,
                review_id.as_deref(),
                unresolved_only,
                format,
            )?;
        }

        Commands::Diff { review_id } => {
            run_diff(&crit_root, &workspace_root, &review_id, format)?;
        }

        Commands::Ui => {
            crit::tui::run(&crit_root)?;
        }

        Commands::Comment {
            review_id,
            file,
            line,
            message,
        } => {
            run_comment(
                &crit_root,
                &workspace_root,
                &review_id,
                &file,
                &line,
                &message,
                cli.author.as_deref(),
                format,
            )?;
        }
    }

    Ok(())
}
