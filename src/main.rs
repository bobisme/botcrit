//! crit - Agent-centric distributed code review tool for jj

use anyhow::Result;
use clap::Parser;
use std::env;

use crit::cli::commands::{
    run_agents_init, run_agents_show, run_comments_add, run_comments_list, run_diff, run_doctor,
    run_init, run_reviews_abandon, run_reviews_approve, run_reviews_create, run_reviews_list,
    run_reviews_request, run_reviews_show, run_status, run_threads_create, run_threads_list,
    run_threads_reopen, run_threads_resolve, run_threads_show,
};
use crit::cli::{
    AgentsCommands, Cli, Commands, CommentsCommands, ReviewsCommands, ThreadsCommands,
};
use crit::output::OutputFormat;

fn main() -> Result<()> {
    let cli = Cli::parse();

    // Get current working directory as repo root
    let repo_root = env::current_dir()?;

    // Determine output format
    let format = if cli.json {
        OutputFormat::Json
    } else {
        OutputFormat::Toon
    };

    match cli.command {
        Commands::Init => {
            run_init(&repo_root)?;
        }

        Commands::Doctor => {
            run_doctor(&repo_root, format)?;
        }

        Commands::Agents(cmd) => match cmd {
            AgentsCommands::Init => {
                run_agents_init(&repo_root)?;
            }
            AgentsCommands::Show => {
                run_agents_show()?;
            }
        },

        Commands::Reviews(cmd) => match cmd {
            ReviewsCommands::Create { title, desc } => {
                run_reviews_create(&repo_root, title, desc, cli.author.as_deref(), format)?;
            }
            ReviewsCommands::List { status, author } => {
                let status_str = status.map(|s| match s {
                    crit::cli::ReviewStatus::Open => "open",
                    crit::cli::ReviewStatus::Approved => "approved",
                    crit::cli::ReviewStatus::Merged => "merged",
                    crit::cli::ReviewStatus::Abandoned => "abandoned",
                });
                run_reviews_list(&repo_root, status_str, author.as_deref(), format)?;
            }
            ReviewsCommands::Show { review_id } => {
                run_reviews_show(&repo_root, &review_id, format)?;
            }
            ReviewsCommands::Request {
                review_id,
                reviewers,
            } => {
                run_reviews_request(
                    &repo_root,
                    &review_id,
                    &reviewers,
                    cli.author.as_deref(),
                    format,
                )?;
            }
            ReviewsCommands::Approve { review_id } => {
                run_reviews_approve(&repo_root, &review_id, cli.author.as_deref(), format)?;
            }
            ReviewsCommands::Abandon { review_id, reason } => {
                run_reviews_abandon(
                    &repo_root,
                    &review_id,
                    reason,
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
                    &repo_root,
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
                run_threads_list(&repo_root, &review_id, status_str, file.as_deref(), format)?;
            }
            ThreadsCommands::Show { thread_id, context } => {
                run_threads_show(&repo_root, &thread_id, context, format)?;
            }
            ThreadsCommands::Resolve {
                thread_id,
                all,
                file,
                reason,
            } => {
                run_threads_resolve(
                    &repo_root,
                    thread_id.as_deref(),
                    all,
                    file.as_deref(),
                    reason,
                    cli.author.as_deref(),
                    format,
                )?;
            }
            ThreadsCommands::Reopen { thread_id, reason } => {
                run_threads_reopen(
                    &repo_root,
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
                request_id,
                expected_hash,
            } => {
                run_comments_add(
                    &repo_root,
                    &thread_id,
                    &message,
                    request_id,
                    expected_hash,
                    cli.author.as_deref(),
                    format,
                )?;
            }
            CommentsCommands::List { thread_id } => {
                run_comments_list(&repo_root, &thread_id, format)?;
            }
        },

        Commands::Status {
            review_id,
            unresolved_only,
        } => {
            run_status(&repo_root, review_id.as_deref(), unresolved_only, format)?;
        }

        Commands::Diff { review_id } => {
            run_diff(&repo_root, &review_id, format)?;
        }
    }

    Ok(())
}
