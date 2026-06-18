//! Minimal helpers around the GitLab CLI (`glab`).
//!
//! This module provides low-level access to the GitLab CLI for merge request
//! operations, mirroring the structure of the GitHub CLI wrapper.

use std::{
    ffi::{OsStr, OsString},
    path::Path,
    process::Command,
};

use chrono::{DateTime, Utc};
use db::models::merge::MergeStatus;
use serde::Deserialize;
use thiserror::Error;
use utils::shell::resolve_executable_path_blocking;

use crate::types::{CreatePrRequest, PullRequestDetail, UnifiedPrComment};

#[derive(Debug, Error)]
pub enum GlabCliError {
    #[error("GitLab CLI (`glab`) executable not found or not runnable")]
    NotAvailable,
    #[error("GitLab CLI command failed: {0}")]
    CommandFailed(String),
    #[error("GitLab CLI authentication failed: {0}")]
    AuthFailed(String),
    #[error("GitLab CLI returned unexpected output: {0}")]
    UnexpectedOutput(String),
}

#[derive(Debug, Clone, Default)]
pub struct GlabCli;

/// JSON response from `glab mr view --output json`.
#[derive(Deserialize)]
struct GlabMrResponse {
    iid: i64,
    web_url: String,
    state: String,
    merged_at: Option<DateTime<Utc>>,
    merge_commit_sha: Option<String>,
    source_branch: Option<String>,
    target_branch: Option<String>,
    title: Option<String>,
}

/// JSON response from `glab mr list --output json`.
#[derive(Deserialize)]
struct GlabMrListItem {
    iid: i64,
    web_url: String,
    state: String,
    merged_at: Option<DateTime<Utc>>,
    merge_commit_sha: Option<String>,
    source_branch: Option<String>,
    target_branch: Option<String>,
    title: Option<String>,
}

/// JSON response item for MR notes from `glab api`.
#[derive(Deserialize)]
struct GlabNoteResponse {
    id: i64,
    body: String,
    author: GlabNoteAuthor,
    created_at: Option<DateTime<Utc>>,
    system: Option<bool>,
    #[serde(rename = "type")]
    note_type: Option<String>,
    position: Option<GlabNotePosition>,
}

#[derive(Deserialize)]
struct GlabNoteAuthor {
    username: String,
}

#[derive(Deserialize)]
struct GlabNotePosition {
    new_path: Option<String>,
    new_line: Option<i64>,
    old_path: Option<String>,
}

impl GlabCli {
    pub fn new() -> Self {
        Self {}
    }

    fn ensure_available(&self) -> Result<(), GlabCliError> {
        resolve_executable_path_blocking("glab").ok_or(GlabCliError::NotAvailable)?;
        Ok(())
    }

    fn run<I, S>(&self, args: I, dir: Option<&Path>) -> Result<String, GlabCliError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.ensure_available()?;
        let glab = resolve_executable_path_blocking("glab").ok_or(GlabCliError::NotAvailable)?;
        let mut cmd = Command::new(&glab);
        if let Some(d) = dir {
            cmd.current_dir(d);
        }
        for arg in args {
            cmd.arg(arg);
        }

        tracing::debug!("Running GitLab CLI command: {:?} {:?}", glab, cmd.get_args());

        let output = cmd
            .output()
            .map_err(|err| GlabCliError::CommandFailed(err.to_string()))?;

        if output.status.success() {
            return Ok(String::from_utf8_lossy(&output.stdout).to_string());
        }

        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();

        let lower = stderr.to_ascii_lowercase();
        if lower.contains("auth")
            || lower.contains("unauthorized")
            || lower.contains("401")
            || lower.contains("glab auth login")
            || lower.contains("token")
        {
            return Err(GlabCliError::AuthFailed(stderr));
        }

        Err(GlabCliError::CommandFailed(stderr))
    }

    /// Create a merge request using `glab mr create`.
    pub fn create_mr(
        &self,
        request: &CreatePrRequest,
        repo_path: &Path,
    ) -> Result<PullRequestDetail, GlabCliError> {
        let body = request.body.as_deref().unwrap_or("");

        let mut args: Vec<OsString> = Vec::with_capacity(14);
        args.push(OsString::from("mr"));
        args.push(OsString::from("create"));
        args.push(OsString::from("--source-branch"));
        args.push(OsString::from(&request.head_branch));
        args.push(OsString::from("--target-branch"));
        args.push(OsString::from(&request.base_branch));
        args.push(OsString::from("--title"));
        args.push(OsString::from(&request.title));
        args.push(OsString::from("--description"));
        args.push(OsString::from(body));
        args.push(OsString::from("--no-editor"));
        args.push(OsString::from("--yes"));

        if request.draft.unwrap_or(false) {
            args.push(OsString::from("--draft"));
        }

        let raw = self.run(args, Some(repo_path))?;
        Self::parse_mr_create_text(&raw, &request.title, &request.head_branch, &request.base_branch)
    }

    /// View a merge request by its URL using `glab mr view`.
    pub fn view_mr(
        &self,
        mr_id: &str,
        repo_path: &Path,
    ) -> Result<PullRequestDetail, GlabCliError> {
        let raw = self.run(
            ["mr", "view", mr_id, "--output", "json"],
            Some(repo_path),
        )?;
        Self::parse_mr_view(&raw)
    }

    /// List merge requests for a branch using `glab mr list`.
    pub fn list_mrs_for_branch(
        &self,
        branch: &str,
        repo_path: &Path,
    ) -> Result<Vec<PullRequestDetail>, GlabCliError> {
        let raw = self.run(
            [
                "mr",
                "list",
                "--source-branch",
                branch,
                "--all",
                "--output",
                "json",
            ],
            Some(repo_path),
        )?;
        Self::parse_mr_list(&raw)
    }

    /// List open merge requests using `glab mr list`.
    pub fn list_open_mrs(
        &self,
        repo_path: &Path,
    ) -> Result<Vec<PullRequestDetail>, GlabCliError> {
        let raw = self.run(
            ["mr", "list", "--output", "json"],
            Some(repo_path),
        )?;
        Self::parse_open_mr_list(&raw)
    }

    /// Fetch MR notes (comments) via `glab api`.
    pub fn get_mr_notes(
        &self,
        mr_number: i64,
        repo_path: &Path,
    ) -> Result<Vec<UnifiedPrComment>, GlabCliError> {
        let raw = self.run(
            [
                "api",
                &format!("projects/:id/merge_requests/{}/notes?sort=asc&per_page=100", mr_number),
            ],
            Some(repo_path),
        )?;
        Self::parse_mr_notes(&raw)
    }
}

// Parsing helpers
impl GlabCli {
    /// Parse `glab mr create` text output to extract the MR URL.
    /// glab outputs a line like: `https://gitlab.com/owner/repo/-/merge_requests/123`
    fn parse_mr_create_text(
        raw: &str,
        title: &str,
        head_branch: &str,
        base_branch: &str,
    ) -> Result<PullRequestDetail, GlabCliError> {
        let mr_url = raw
            .lines()
            .rev()
            .flat_map(|line| line.split_whitespace())
            .map(|token| token.trim_matches(|c: char| c == '<' || c == '>'))
            .find(|token| token.starts_with("http") && token.contains("merge_requests"))
            .ok_or_else(|| {
                GlabCliError::UnexpectedOutput(format!(
                    "glab mr create did not return a merge request URL; raw output: {raw}"
                ))
            })?
            .trim_end_matches(['.', ',', ';'])
            .to_string();

        let number = mr_url
            .rsplit('/')
            .next()
            .ok_or_else(|| {
                GlabCliError::UnexpectedOutput(format!(
                    "Failed to extract MR number from URL '{mr_url}'"
                ))
            })?
            .trim_end_matches(|c: char| !c.is_ascii_digit())
            .parse::<i64>()
            .map_err(|err| {
                GlabCliError::UnexpectedOutput(format!(
                    "Failed to parse MR number from URL '{mr_url}': {err}"
                ))
            })?;

        Ok(PullRequestDetail {
            number,
            url: mr_url,
            status: MergeStatus::Open,
            merged_at: None,
            merge_commit_sha: None,
            title: title.to_string(),
            head_branch: head_branch.to_string(),
            base_branch: base_branch.to_string(),
        })
    }

    fn parse_mr_view(raw: &str) -> Result<PullRequestDetail, GlabCliError> {
        let mr: GlabMrResponse = serde_json::from_str(raw.trim()).map_err(|err| {
            GlabCliError::UnexpectedOutput(format!(
                "Failed to parse glab mr view response: {err}; raw: {raw}"
            ))
        })?;
        Ok(Self::mr_response_to_info(
            mr.iid,
            &mr.web_url,
            &mr.state,
            mr.merged_at,
            mr.merge_commit_sha,
            mr.title,
            mr.source_branch,
            mr.target_branch,
        ))
    }

    fn parse_mr_list(raw: &str) -> Result<Vec<PullRequestDetail>, GlabCliError> {
        let mrs: Vec<GlabMrListItem> = serde_json::from_str(raw.trim()).map_err(|err| {
            GlabCliError::UnexpectedOutput(format!(
                "Failed to parse glab mr list response: {err}; raw: {raw}"
            ))
        })?;
        Ok(mrs
            .into_iter()
            .map(|mr| {
                Self::mr_response_to_info(
                    mr.iid,
                    &mr.web_url,
                    &mr.state,
                    mr.merged_at,
                    mr.merge_commit_sha,
                    mr.title,
                    mr.source_branch,
                    mr.target_branch,
                )
            })
            .collect())
    }

    fn parse_open_mr_list(raw: &str) -> Result<Vec<PullRequestDetail>, GlabCliError> {
        let mrs: Vec<GlabMrListItem> = serde_json::from_str(raw.trim()).map_err(|err| {
            GlabCliError::UnexpectedOutput(format!(
                "Failed to parse glab mr list response: {err}; raw: {raw}"
            ))
        })?;
        Ok(mrs
            .into_iter()
            .map(|mr| {
                Self::mr_response_to_info(
                    mr.iid,
                    &mr.web_url,
                    &mr.state,
                    mr.merged_at,
                    mr.merge_commit_sha,
                    mr.title,
                    mr.source_branch,
                    mr.target_branch,
                )
            })
            .collect())
    }

    fn mr_response_to_info(
        iid: i64,
        web_url: &str,
        state: &str,
        merged_at: Option<DateTime<Utc>>,
        merge_commit_sha: Option<String>,
        title: Option<String>,
        head_branch: Option<String>,
        base_branch: Option<String>,
    ) -> PullRequestDetail {
        PullRequestDetail {
            number: iid,
            url: web_url.to_string(),
            status: Self::map_gitlab_state(state),
            merged_at,
            merge_commit_sha,
            title: title.unwrap_or_default(),
            head_branch: head_branch.unwrap_or_default(),
            base_branch: base_branch.unwrap_or_default(),
        }
    }

    fn map_gitlab_state(state: &str) -> MergeStatus {
        match state.to_lowercase().as_str() {
            "opened" => MergeStatus::Open,
            "merged" => MergeStatus::Merged,
            "closed" => MergeStatus::Closed,
            _ => MergeStatus::Unknown,
        }
    }

    fn parse_mr_notes(raw: &str) -> Result<Vec<UnifiedPrComment>, GlabCliError> {
        let notes: Vec<GlabNoteResponse> = serde_json::from_str(raw.trim()).map_err(|err| {
            GlabCliError::UnexpectedOutput(format!(
                "Failed to parse MR notes response: {err}; raw: {raw}"
            ))
        })?;

        let mut comments = Vec::new();
        for note in notes {
            // Skip system-generated notes (e.g., "assigned to", "changed the description")
            if note.system.unwrap_or(false) {
                continue;
            }

            let created_at = note.created_at.unwrap_or_else(Utc::now);

            if let Some(pos) = note.position {
                // This is a diff/review comment
                let path = pos
                    .new_path
                    .or(pos.old_path)
                    .unwrap_or_else(|| String::new());
                comments.push(UnifiedPrComment::Review {
                    id: note.id,
                    author: note.author.username,
                    author_association: None,
                    body: note.body,
                    created_at,
                    url: None,
                    path,
                    line: pos.new_line,
                    side: None,
                    diff_hunk: None,
                });
            } else if note.note_type.as_deref() == Some("DiffNote") {
                // DiffNote without position data — treat as review comment
                comments.push(UnifiedPrComment::Review {
                    id: note.id,
                    author: note.author.username,
                    author_association: None,
                    body: note.body,
                    created_at,
                    url: None,
                    path: String::new(),
                    line: None,
                    side: None,
                    diff_hunk: None,
                });
            } else {
                // General comment
                comments.push(UnifiedPrComment::General {
                    id: note.id.to_string(),
                    author: note.author.username,
                    author_association: None,
                    body: note.body,
                    created_at,
                    url: None,
                });
            }
        }

        Ok(comments)
    }
}
