//! GitLab hosting service implementation.

mod cli;

use std::{path::Path, time::Duration};

use async_trait::async_trait;
use backon::{ExponentialBuilder, Retryable};
pub use cli::GlabCli;
use cli::GlabCliError;
use tokio::task;
use tracing::info;

use crate::{
    GitHostProvider,
    types::{CreatePrRequest, GitHostError, PullRequestDetail, ProviderKind, UnifiedPrComment},
};

#[derive(Debug, Clone)]
pub struct GitLabProvider {
    glab_cli: GlabCli,
}

impl GitLabProvider {
    pub fn new() -> Result<Self, GitHostError> {
        Ok(Self {
            glab_cli: GlabCli::new(),
        })
    }
}

impl From<GlabCliError> for GitHostError {
    fn from(error: GlabCliError) -> Self {
        match &error {
            GlabCliError::AuthFailed(msg) => GitHostError::AuthFailed(msg.clone()),
            GlabCliError::NotAvailable => GitHostError::CliNotInstalled {
                provider: ProviderKind::GitLab,
            },
            GlabCliError::CommandFailed(msg) => {
                let lower = msg.to_ascii_lowercase();
                if lower.contains("403") || lower.contains("forbidden") {
                    GitHostError::InsufficientPermissions(msg.clone())
                } else if lower.contains("404") || lower.contains("not found") {
                    GitHostError::RepoNotFoundOrNoAccess(msg.clone())
                } else if lower.contains("not a git repository") {
                    GitHostError::NotAGitRepository(msg.clone())
                } else {
                    GitHostError::PullRequest(msg.clone())
                }
            }
            GlabCliError::UnexpectedOutput(msg) => GitHostError::UnexpectedOutput(msg.clone()),
        }
    }
}

#[async_trait]
impl GitHostProvider for GitLabProvider {
    async fn create_pr(
        &self,
        repo_path: &Path,
        _remote_url: &str,
        request: &CreatePrRequest,
    ) -> Result<PullRequestDetail, GitHostError> {
        if let Some(head_url) = &request.head_repo_url
            && head_url != _remote_url
        {
            return Err(GitHostError::PullRequest(
                "Cross-fork merge requests are not supported for GitLab via CLI".to_string(),
            ));
        }

        let head_branch = request.head_branch.clone();

        (|| async {
            let cli = self.glab_cli.clone();
            let request = request.clone();
            let repo_path = repo_path.to_path_buf();

            let cli_result =
                task::spawn_blocking(move || cli.create_mr(&request, &repo_path))
                    .await
                    .map_err(|err| {
                        GitHostError::PullRequest(format!(
                            "Failed to execute GitLab CLI for MR creation: {err}"
                        ))
                    })?
                    .map_err(GitHostError::from)?;

            info!(
                "Created GitLab MR !{} for branch {}",
                cli_result.number, head_branch
            );

            Ok(cli_result)
        })
        .retry(
            &ExponentialBuilder::default()
                .with_min_delay(Duration::from_secs(1))
                .with_max_delay(Duration::from_secs(30))
                .with_max_times(3)
                .with_jitter(),
        )
        .when(|e: &GitHostError| e.should_retry())
        .notify(|err: &GitHostError, dur: Duration| {
            tracing::warn!(
                "GitLab API call failed, retrying after {:.2}s: {}",
                dur.as_secs_f64(),
                err
            );
        })
        .await
    }

    async fn get_pr_status(&self, pr_url: &str) -> Result<PullRequestDetail, GitHostError> {
        // We pass the full MR URL to glab mr view so it can resolve the MR directly.
        (|| async {
            let cli = self.glab_cli.clone();
            let url = pr_url.to_string();

            // glab mr view can accept the MR URL directly
            let pr = task::spawn_blocking(move || {
                // Try with a temp path — glab uses the repo from the URL context
                // We need to find a valid repo path. Since we don't have one here,
                // we'll use the current directory and pass the URL as the ID.
                cli.view_mr(&url, Path::new("."))
            })
            .await
            .map_err(|err| {
                GitHostError::PullRequest(format!(
                    "Failed to execute GitLab CLI for viewing MR: {err}"
                ))
            })?;
            pr.map_err(GitHostError::from)
        })
        .retry(
            &ExponentialBuilder::default()
                .with_min_delay(Duration::from_secs(1))
                .with_max_delay(Duration::from_secs(30))
                .with_max_times(3)
                .with_jitter(),
        )
        .when(|err: &GitHostError| err.should_retry())
        .notify(|err: &GitHostError, dur: Duration| {
            tracing::warn!(
                "GitLab API call failed, retrying after {:.2}s: {}",
                dur.as_secs_f64(),
                err
            );
        })
        .await
    }

    async fn list_prs_for_branch(
        &self,
        repo_path: &Path,
        _remote_url: &str,
        branch_name: &str,
    ) -> Result<Vec<PullRequestDetail>, GitHostError> {
        (|| async {
            let cli = self.glab_cli.clone();
            let branch = branch_name.to_string();
            let repo_path = repo_path.to_path_buf();

            let prs = task::spawn_blocking(move || cli.list_mrs_for_branch(&branch, &repo_path))
                .await
                .map_err(|err| {
                    GitHostError::PullRequest(format!(
                        "Failed to execute GitLab CLI for listing MRs: {err}"
                    ))
                })?;
            prs.map_err(GitHostError::from)
        })
        .retry(
            &ExponentialBuilder::default()
                .with_min_delay(Duration::from_secs(1))
                .with_max_delay(Duration::from_secs(30))
                .with_max_times(3)
                .with_jitter(),
        )
        .when(|e: &GitHostError| e.should_retry())
        .notify(|err: &GitHostError, dur: Duration| {
            tracing::warn!(
                "GitLab API call failed, retrying after {:.2}s: {}",
                dur.as_secs_f64(),
                err
            );
        })
        .await
    }

    async fn get_pr_comments(
        &self,
        repo_path: &Path,
        _remote_url: &str,
        pr_number: i64,
    ) -> Result<Vec<UnifiedPrComment>, GitHostError> {
        (|| async {
            let cli = self.glab_cli.clone();
            let repo_path = repo_path.to_path_buf();

            let comments =
                task::spawn_blocking(move || cli.get_mr_notes(pr_number, &repo_path))
                    .await
                    .map_err(|err| {
                        GitHostError::PullRequest(format!(
                            "Failed to execute GitLab CLI for fetching MR comments: {err}"
                        ))
                    })?;
            comments.map_err(GitHostError::from)
        })
        .retry(
            &ExponentialBuilder::default()
                .with_min_delay(Duration::from_secs(1))
                .with_max_delay(Duration::from_secs(30))
                .with_max_times(3)
                .with_jitter(),
        )
        .when(|e: &GitHostError| e.should_retry())
        .notify(|err: &GitHostError, dur: Duration| {
            tracing::warn!(
                "GitLab API call failed, retrying after {:.2}s: {}",
                dur.as_secs_f64(),
                err
            );
        })
        .await
    }

    async fn list_open_prs(
        &self,
        repo_path: &Path,
        _remote_url: &str,
    ) -> Result<Vec<PullRequestDetail>, GitHostError> {
        (|| async {
            let cli = self.glab_cli.clone();
            let repo_path = repo_path.to_path_buf();

            let prs = task::spawn_blocking(move || cli.list_open_mrs(&repo_path))
                .await
                .map_err(|err| {
                    GitHostError::PullRequest(format!(
                        "Failed to execute GitLab CLI for listing open MRs: {err}"
                    ))
                })?;
            prs.map_err(GitHostError::from)
        })
        .retry(
            &ExponentialBuilder::default()
                .with_min_delay(Duration::from_secs(1))
                .with_max_delay(Duration::from_secs(30))
                .with_max_times(3)
                .with_jitter(),
        )
        .when(|e: &GitHostError| e.should_retry())
        .notify(|err: &GitHostError, dur: Duration| {
            tracing::warn!(
                "GitLab API call failed, retrying after {:.2}s: {}",
                dur.as_secs_f64(),
                err
            );
        })
        .await
    }

    fn provider_kind(&self) -> ProviderKind {
        ProviderKind::GitLab
    }
}
