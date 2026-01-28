//! crit - Agent-centric distributed code review tool for jj

use anyhow::Result;
use clap::Parser;
use std::env;

use crit::cli::commands::{
    run_agents_init, run_agents_show, run_block, run_comment, run_comments_add, run_comments_list,
    run_diff, run_doctor, run_inbox, run_init, run_lgtm, run_review, run_reviews_abandon,
    run_reviews_approve, run_reviews_create, run_reviews_list, run_reviews_merge,
    run_reviews_request, run_reviews_show, run_status, run_threads_create, run_threads_list,
    run_threads_reopen, run_threads_resolve, run_threads_show,
};
use crit::cli::{
    AgentsCommands, Cli, Commands, CommentsCommands, ReviewsCommands, ThreadsCommands,
};
use crit::events::{get_agent_identity, get_user_identity};
use crit::jj::resolve_repo_root;
use crit::output::OutputFormat;

/// Resolve identity based on CLI flags.
/// Priority: --author > --user > CRIT_AGENT/BOTBUS_AGENT (required)
fn resolve_identity(cli: &Cli) -> Result<Option<String>> {
    if let Some(ref author) = cli.author {
        return Ok(Some(author.clone()));
    }
    if cli.user {
        return Ok(Some(get_user_identity()?));
    }
    // Will be resolved lazily by get_agent_identity when needed
    Ok(None)
}

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

    // Resolve identity (--author or --user override, otherwise deferred to env vars)
    let identity = resolve_identity(&cli)?;

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
                    identity.as_deref(),
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
                // falling back to resolved identity.
                // When --needs-review is used, --author should NOT also filter by review author.
                let (author_filter, needs_reviewer) = if needs_review {
                    // Use --author for identity, not filtering
                    let id = author.as_deref().or(identity.as_deref());
                    (None, Some(get_agent_identity(id)?))
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
                    identity.as_deref(),
                    format,
                )?;
            }
            ReviewsCommands::Approve { review_id } => {
                run_reviews_approve(&crit_root, &review_id, identity.as_deref(), format)?;
            }
            ReviewsCommands::Abandon { review_id, reason } => {
                run_reviews_abandon(&crit_root, &review_id, reason, identity.as_deref(), format)?;
            }
            ReviewsCommands::Merge {
                review_id,
                commit,
                self_approve,
            } => {
                run_reviews_merge(
                    &crit_root,
                    &workspace_root,
                    &review_id,
                    commit,
                    self_approve,
                    identity.as_deref(),
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
                    identity.as_deref(),
                    format,
                )?;
            }
            ThreadsCommands::List {
                review_id,
                status,
                file,
                verbose,
                since,
            } => {
                let status_str = status.map(|s| match s {
                    crit::cli::ThreadStatus::Open => "open",
                    crit::cli::ThreadStatus::Resolved => "resolved",
                });
                let since_dt = since
                    .map(|s| crit::cli::commands::reviews::parse_since(&s))
                    .transpose()?;
                run_threads_list(
                    &crit_root,
                    &review_id,
                    status_str,
                    file.as_deref(),
                    verbose,
                    since_dt,
                    format,
                )?;
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
                    identity.as_deref(),
                    format,
                )?;
            }
            ThreadsCommands::Reopen { thread_id, reason } => {
                run_threads_reopen(&crit_root, &thread_id, reason, identity.as_deref(), format)?;
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
                run_comments_add(&crit_root, &thread_id, &msg, identity.as_deref(), format)?;
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
                identity.as_deref(),
                format,
            )?;
        }

        Commands::Lgtm { review_id, message } => {
            run_lgtm(&crit_root, &review_id, message, identity.as_deref(), format)?;
        }

        Commands::Block { review_id, reason } => {
            run_block(&crit_root, &review_id, reason, identity.as_deref(), format)?;
        }

        Commands::Review {
            review_id,
            context,
            no_context,
            since,
        } => {
            let context_lines = if no_context { 0 } else { context };
            let since_dt = since
                .map(|s| crit::cli::commands::reviews::parse_since(&s))
                .transpose()?;
            run_review(
                &crit_root,
                &workspace_root,
                &review_id,
                context_lines,
                since_dt,
                format,
            )?;
        }

        Commands::Inbox => {
            let agent = get_agent_identity(identity.as_deref())?;
            run_inbox(&crit_root, &agent, format)?;
        }
    }

    Ok(())
}
