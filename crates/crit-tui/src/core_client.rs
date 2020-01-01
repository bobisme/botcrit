//! `CritClient` implementation backed by `CritServices` (direct library calls).

use std::collections::HashMap;

use anyhow::Result;

use crit_core::core::{CoreContext, CritServices};
use crit_core::events::CodeSelection;

use crate::db::{
    Comment, CritClient, ReviewData, ReviewDetail, ReviewSummary, ThreadSummary,
};

/// Client that calls crit-core services directly (no subprocess).
pub struct CoreClient {
    ctx: CoreContext,
}

impl CoreClient {
    pub fn new(ctx: CoreContext) -> Self {
        Self { ctx }
    }

    /// Re-sync and get fresh services.
    fn services(&self) -> Result<CritServices> {
        self.ctx
            .services()
            .map_err(|e| anyhow::anyhow!("{e}"))
    }

    fn comment_agent() -> String {
        std::env::var("USER")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "unknown".to_string())
    }
}

// -- Conversions from crit-core types to UI types --

fn convert_review_summary(r: &crit_core::projection::ReviewSummary) -> ReviewSummary {
    ReviewSummary {
        review_id: r.review_id.clone(),
        title: r.title.clone(),
        author: r.author.clone(),
        status: r.status.clone(),
        thread_count: r.thread_count,
        open_thread_count: r.open_thread_count,
        reviewers: r.reviewers.clone(),
    }
}

fn convert_review_detail(r: &crit_core::projection::ReviewDetail) -> ReviewDetail {
    ReviewDetail {
        review_id: r.review_id.clone(),
        jj_change_id: r.jj_change_id.clone(),
        initial_commit: r.initial_commit.clone(),
        final_commit: r.final_commit.clone(),
        title: r.title.clone(),
        description: r.description.clone(),
        author: r.author.clone(),
        created_at: r.created_at.clone(),
        status: r.status.clone(),
        status_changed_at: r.status_changed_at.clone(),
        status_changed_by: r.status_changed_by.clone(),
        abandon_reason: r.abandon_reason.clone(),
        thread_count: r.thread_count,
        open_thread_count: r.open_thread_count,
    }
}

fn convert_thread_summary(t: &crit_core::projection::ThreadSummary) -> ThreadSummary {
    ThreadSummary {
        thread_id: t.thread_id.clone(),
        file_path: t.file_path.clone(),
        selection_start: t.selection_start,
        selection_end: t.selection_end,
        status: t.status.clone(),
        comment_count: t.comment_count,
    }
}

fn convert_comment(c: &crit_core::projection::Comment) -> Comment {
    Comment {
        comment_id: c.comment_id.clone(),
        author: c.author.clone(),
        body: c.body.clone(),
        created_at: c.created_at.clone(),
    }
}

impl CritClient for CoreClient {
    fn list_reviews(&self, status: Option<&str>) -> Result<Vec<ReviewSummary>> {
        let services = self.services()?;
        let reviews = services
            .reviews()
            .list(status, None)
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        Ok(reviews.iter().map(convert_review_summary).collect())
    }

    fn load_review_data(&self, review_id: &str) -> Result<Option<ReviewData>> {
        let services = self.services()?;

        let detail = match services
            .reviews()
            .get_optional(review_id)
            .map_err(|e| anyhow::anyhow!("{e}"))?
        {
            Some(d) => d,
            None => return Ok(None),
        };

        let core_threads = services
            .threads()
            .list(review_id, None, None)
            .map_err(|e| anyhow::anyhow!("{e}"))?;

        let mut threads = Vec::with_capacity(core_threads.len());
        let mut comments: HashMap<String, Vec<Comment>> = HashMap::new();

        for t in &core_threads {
            threads.push(convert_thread_summary(t));

            let core_comments = services
                .comments()
                .list(&t.thread_id)
                .map_err(|e| anyhow::anyhow!("{e}"))?;

            if !core_comments.is_empty() {
                comments.insert(
                    t.thread_id.clone(),
                    core_comments.iter().map(convert_comment).collect(),
                );
            }
        }

        // No diff data available from direct services — diffs come from VCS
        // The UI will handle the absence of files gracefully
        let files = Vec::new();

        Ok(Some(ReviewData {
            detail: convert_review_detail(&detail),
            threads,
            comments,
            files,
        }))
    }

    fn comment(
        &self,
        review_id: &str,
        file_path: &str,
        start_line: i64,
        end_line: Option<i64>,
        body: &str,
    ) -> Result<()> {
        let services = self.services()?;
        let agent = Self::comment_agent();

        // Get the review's initial commit for thread creation
        let review = services
            .reviews()
            .get(review_id)
            .map_err(|e| anyhow::anyhow!("{e}"))?;

        #[allow(clippy::cast_sign_loss)]
        let selection = match end_line {
            Some(end) if end != start_line => {
                CodeSelection::range(start_line as u32, end as u32)
            }
            _ => CodeSelection::line(start_line as u32),
        };

        services
            .comments()
            .add_to_review(
                review_id,
                file_path,
                selection,
                body,
                review.initial_commit.clone(),
                Some(&agent),
            )
            .map_err(|e| anyhow::anyhow!("{e}"))?;

        Ok(())
    }

    fn reply(&self, thread_id: &str, body: &str) -> Result<()> {
        let services = self.services()?;
        let agent = Self::comment_agent();

        services
            .comments()
            .add_to_thread(thread_id, body, Some(&agent))
            .map_err(|e| anyhow::anyhow!("{e}"))?;

        Ok(())
    }
}
