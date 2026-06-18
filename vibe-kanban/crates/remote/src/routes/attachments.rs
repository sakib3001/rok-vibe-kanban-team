use api_types::{
    AttachmentUrlResponse, AttachmentWithBlob, AttachmentWithUrl, ListAttachmentsResponse,
};
use axum::{
    Json, Router,
    body::Bytes,
    extract::{Extension, Path, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{delete, get, post, put},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tracing::instrument;
use ts_rs::TS;
use uuid::Uuid;

use super::organization_members::{
    ensure_comment_access, ensure_issue_access, ensure_project_access,
};
use crate::{
    AppState,
    attachments::thumbnail::ThumbnailService,
    auth::RequestContext,
    r2::R2Error,
    db::{
        attachments::{AttachmentError, AttachmentRepository},
        blobs::{BlobError, BlobRepository},
        pending_uploads::{PendingUploadError, PendingUploadRepository},
    },
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/attachments/init", post(init_upload))
        .route("/attachments/confirm", post(confirm_upload))
        .route("/attachments/{id}/file", get(get_attachment_file))
        .route("/attachments/{id}/thumbnail", get(get_attachment_thumbnail))
        .route("/attachments/{id}", delete(delete_attachment))
        .route(
            "/issues/{issue_id}/attachments",
            get(list_issue_attachments),
        )
        .route(
            "/issues/{issue_id}/attachments/commit",
            post(commit_issue_attachments),
        )
        .route(
            "/comments/{comment_id}/attachments",
            get(list_comment_attachments),
        )
        .route(
            "/comments/{comment_id}/attachments/commit",
            post(commit_comment_attachments),
        )
}

pub fn public_router() -> Router<AppState> {
    Router::new().route("/attachments/upload/{upload_id}", put(upload_pending_upload))
}

#[derive(Debug, Serialize, Deserialize, TS)]
pub struct InitUploadRequest {
    pub project_id: Uuid,
    pub filename: String,
    #[ts(type = "number")]
    pub size_bytes: i64,
    pub hash: String,
}

#[derive(Debug, Serialize, Deserialize, TS)]
pub struct InitUploadResponse {
    pub upload_url: String,
    pub upload_id: Uuid,
    pub expires_at: DateTime<Utc>,
    pub skip_upload: bool,
    pub existing_blob_id: Option<Uuid>,
}

#[derive(Debug, Serialize, Deserialize, TS)]
pub struct ConfirmUploadRequest {
    pub project_id: Uuid,
    pub upload_id: Uuid,
    pub filename: String,
    #[ts(optional)]
    pub content_type: Option<String>,
    #[ts(type = "number")]
    pub size_bytes: i64,
    pub hash: String,
    #[ts(optional)]
    pub issue_id: Option<Uuid>,
    #[ts(optional)]
    pub comment_id: Option<Uuid>,
}

#[derive(Debug, Serialize, Deserialize, TS)]
pub struct CommitAttachmentsRequest {
    pub attachment_ids: Vec<Uuid>,
}

#[derive(Debug, Serialize, Deserialize, TS)]
pub struct CommitAttachmentsResponse {
    pub attachments: Vec<AttachmentWithBlob>,
}

#[derive(Debug, thiserror::Error)]
pub enum RouteError {
    #[error("R2 storage not configured")]
    NotConfigured,
    #[error("R2 storage error: {0}")]
    R2(#[from] R2Error),
    #[error("attachment error: {0}")]
    Attachment(#[from] AttachmentError),
    #[error("blob error: {0}")]
    Blob(#[from] BlobError),
    #[error("attachment not found")]
    NotFound,
    #[error("no thumbnail available")]
    NoThumbnail,
    #[error("access denied")]
    AccessDenied,
    #[error("file too large (max 20MB)")]
    FileTooLarge,
    #[error("upload not found or expired")]
    UploadNotFound,
    #[error("pending upload error: {0}")]
    PendingUpload(#[from] PendingUploadError),
    #[error("thumbnail generation failed: {0}")]
    ThumbnailError(String),
}

impl IntoResponse for RouteError {
    fn into_response(self) -> Response {
        let (status, message) = match &self {
            RouteError::NotConfigured => (
                StatusCode::SERVICE_UNAVAILABLE,
                "Attachment storage not available",
            ),
            RouteError::R2(e) => {
                tracing::error!(error = %e, "R2 storage error");
                (StatusCode::INTERNAL_SERVER_ERROR, "Storage error")
            }
            RouteError::Attachment(e) => {
                tracing::error!(error = %e, "Attachment error");
                (StatusCode::INTERNAL_SERVER_ERROR, "Database error")
            }
            RouteError::Blob(e) => {
                tracing::error!(error = %e, "Blob error");
                (StatusCode::INTERNAL_SERVER_ERROR, "Database error")
            }
            RouteError::NotFound => (StatusCode::NOT_FOUND, "Attachment not found"),
            RouteError::NoThumbnail => (StatusCode::NOT_FOUND, "No thumbnail available"),
            RouteError::AccessDenied => (StatusCode::FORBIDDEN, "Access denied"),
            RouteError::FileTooLarge => {
                (StatusCode::PAYLOAD_TOO_LARGE, "File too large (max 20MB)")
            }
            RouteError::UploadNotFound => (StatusCode::NOT_FOUND, "Upload not found or expired"),
            RouteError::PendingUpload(e) => {
                tracing::error!(error = %e, "Pending upload error");
                (StatusCode::INTERNAL_SERVER_ERROR, "Database error")
            }
            RouteError::ThumbnailError(e) => {
                tracing::error!(error = %e, "Thumbnail generation failed");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Thumbnail generation failed",
                )
            }
        };

        let body = serde_json::json!({ "error": message });
        (status, Json(body)).into_response()
    }
}

const MAX_FILE_SIZE: i64 = 20 * 1024 * 1024;

#[instrument(name = "attachments.init_upload", skip(state, ctx, payload), fields(project_id = %payload.project_id, user_id = %ctx.user.id))]
async fn init_upload(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Json(payload): Json<InitUploadRequest>,
) -> Result<Json<InitUploadResponse>, RouteError> {
    ensure_project_access(state.pool(), ctx.user.id, payload.project_id)
        .await
        .map_err(|_| RouteError::AccessDenied)?;

    if payload.size_bytes > MAX_FILE_SIZE {
        return Err(RouteError::FileTooLarge);
    }

    if let Some(existing) =
        BlobRepository::find_by_hash(state.pool(), payload.project_id, &payload.hash).await?
    {
        let r2 = state.r2().ok_or(RouteError::NotConfigured)?;
        let read_url = r2.create_read_url(&existing.blob_path).await?;

        return Ok(Json(InitUploadResponse {
            upload_url: read_url,
            upload_id: existing.id,
            expires_at: Utc::now() + chrono::Duration::minutes(5),
            skip_upload: true,
            existing_blob_id: Some(existing.id),
        }));
    }

    state.r2().ok_or(RouteError::NotConfigured)?;
    let sanitized_filename = sanitize_filename(&payload.filename);
    let blob_path = format!(
        "attachments/{}/{}_{}",
        payload.project_id,
        Uuid::new_v4(),
        sanitized_filename
    );

    let expires_at = Utc::now() + chrono::Duration::minutes(5);

    let pending = PendingUploadRepository::create(
        state.pool(),
        payload.project_id,
        blob_path,
        payload.hash.clone(),
        expires_at,
    )
    .await?;

    let upload_url = format!(
        "{}/v1/attachments/upload/{}",
        state.server_public_base_url.trim_end_matches('/'),
        pending.id
    );

    Ok(Json(InitUploadResponse {
        upload_url,
        upload_id: pending.id,
        expires_at,
        skip_upload: false,
        existing_blob_id: None,
    }))
}

#[instrument(name = "attachments.upload_pending", skip(state, headers, body), fields(upload_id = %upload_id))]
async fn upload_pending_upload(
    State(state): State<AppState>,
    Path(upload_id): Path<Uuid>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<StatusCode, RouteError> {
    if body.len() as i64 > MAX_FILE_SIZE {
        return Err(RouteError::FileTooLarge);
    }

    let pending = PendingUploadRepository::find_by_id(state.pool(), upload_id)
        .await?
        .ok_or(RouteError::UploadNotFound)?;

    if pending.expires_at < Utc::now() {
        return Err(RouteError::UploadNotFound);
    }

    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .filter(|v| !v.is_empty())
        .unwrap_or("application/octet-stream")
        .to_string();

    let r2 = state.r2().ok_or(RouteError::NotConfigured)?;
    r2.upload_blob(&pending.blob_path, body.to_vec(), content_type)
        .await?;

    // Keep legacy client compatibility: existing uploader treats 201 as success.
    Ok(StatusCode::CREATED)
}

#[instrument(name = "attachments.confirm_upload", skip(state, ctx, payload), fields(project_id = %payload.project_id, user_id = %ctx.user.id))]
async fn confirm_upload(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Json(payload): Json<ConfirmUploadRequest>,
) -> Result<Json<AttachmentWithBlob>, RouteError> {
    ensure_project_access(state.pool(), ctx.user.id, payload.project_id)
        .await
        .map_err(|_| RouteError::AccessDenied)?;

    if let Some(issue_id) = payload.issue_id {
        ensure_issue_access(state.pool(), ctx.user.id, issue_id)
            .await
            .map_err(|_| RouteError::AccessDenied)?;
    }
    if let Some(comment_id) = payload.comment_id {
        ensure_comment_access(state.pool(), ctx.user.id, comment_id)
            .await
            .map_err(|_| RouteError::AccessDenied)?;
    }

    let r2 = state.r2().ok_or(RouteError::NotConfigured)?;

    let blob = if let Some(existing) =
        BlobRepository::find_by_hash(state.pool(), payload.project_id, &payload.hash).await?
    {
        existing
    } else {
        let pending = PendingUploadRepository::find_by_id(state.pool(), payload.upload_id)
            .await?
            .ok_or(RouteError::UploadNotFound)?;

        if pending.expires_at < Utc::now() {
            return Err(RouteError::UploadNotFound);
        }

        let blob_path = &pending.blob_path;
        let props = r2.get_blob_properties(blob_path).await?;
        if props.content_length > MAX_FILE_SIZE {
            let _ = r2.delete_blob(blob_path).await;
            return Err(RouteError::FileTooLarge);
        }

        let blob_data = r2.download_blob(blob_path).await?;
        let thumbnail_result =
            ThumbnailService::generate(&blob_data, payload.content_type.as_deref())
                .map_err(|e| RouteError::ThumbnailError(e.to_string()))?;

        let _ = PendingUploadRepository::delete(state.pool(), pending.id).await;

        let (thumbnail_blob_path, width, height) = match thumbnail_result {
            Some(thumb) => {
                let thumb_path = format!("thumbnails/{}", blob_path);
                r2
                    .upload_blob(&thumb_path, thumb.bytes, thumb.mime_type)
                    .await?;
                (
                    Some(thumb_path),
                    Some(thumb.original_width as i32),
                    Some(thumb.original_height as i32),
                )
            }
            None => (None, None, None),
        };

        BlobRepository::create(
            state.pool(),
            None,
            payload.project_id,
            blob_path.clone(),
            thumbnail_blob_path,
            payload.filename.clone(),
            payload.content_type.clone(),
            payload.size_bytes,
            payload.hash.clone(),
            width,
            height,
        )
        .await?
    };

    let expires_at = if payload.issue_id.is_some() || payload.comment_id.is_some() {
        None
    } else {
        Some(Utc::now() + chrono::Duration::hours(24))
    };

    let attachment = AttachmentRepository::create(
        state.pool(),
        None,
        blob.id,
        payload.issue_id,
        payload.comment_id,
        expires_at,
    )
    .await?;

    let result = AttachmentRepository::find_by_id_with_blob(state.pool(), attachment.id)
        .await?
        .ok_or(RouteError::NotFound)?;

    Ok(Json(result))
}

#[instrument(name = "attachments.commit_issue", skip(state, ctx, payload), fields(issue_id = %issue_id, user_id = %ctx.user.id))]
async fn commit_issue_attachments(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path(issue_id): Path<Uuid>,
    Json(payload): Json<CommitAttachmentsRequest>,
) -> Result<Json<CommitAttachmentsResponse>, RouteError> {
    ensure_issue_access(state.pool(), ctx.user.id, issue_id)
        .await
        .map_err(|_| RouteError::AccessDenied)?;

    let attachments =
        AttachmentRepository::commit_to_issue(state.pool(), &payload.attachment_ids, issue_id)
            .await?;
    Ok(Json(CommitAttachmentsResponse { attachments }))
}

#[instrument(name = "attachments.commit_comment", skip(state, ctx, payload), fields(comment_id = %comment_id, user_id = %ctx.user.id))]
async fn commit_comment_attachments(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path(comment_id): Path<Uuid>,
    Json(payload): Json<CommitAttachmentsRequest>,
) -> Result<Json<CommitAttachmentsResponse>, RouteError> {
    ensure_comment_access(state.pool(), ctx.user.id, comment_id)
        .await
        .map_err(|_| RouteError::AccessDenied)?;

    let attachments =
        AttachmentRepository::commit_to_comment(state.pool(), &payload.attachment_ids, comment_id)
            .await?;
    Ok(Json(CommitAttachmentsResponse { attachments }))
}

#[instrument(name = "attachments.list_issue", skip(state, ctx), fields(issue_id = %issue_id, user_id = %ctx.user.id))]
async fn list_issue_attachments(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path(issue_id): Path<Uuid>,
) -> Result<Json<ListAttachmentsResponse>, RouteError> {
    ensure_issue_access(state.pool(), ctx.user.id, issue_id)
        .await
        .map_err(|_| RouteError::AccessDenied)?;

    let r2 = state.r2();
    let mut attachments = Vec::new();
    for attachment in AttachmentRepository::find_by_issue_id(state.pool(), issue_id).await? {
        let file_url = if let Some(service) = r2 {
            service.create_read_url(&attachment.blob_path).await.ok()
        } else {
            None
        };
        attachments.push(AttachmentWithUrl {
            attachment,
            file_url,
        });
    }
    Ok(Json(ListAttachmentsResponse { attachments }))
}

#[instrument(name = "attachments.list_comment", skip(state, ctx), fields(comment_id = %comment_id, user_id = %ctx.user.id))]
async fn list_comment_attachments(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path(comment_id): Path<Uuid>,
) -> Result<Json<ListAttachmentsResponse>, RouteError> {
    ensure_comment_access(state.pool(), ctx.user.id, comment_id)
        .await
        .map_err(|_| RouteError::AccessDenied)?;

    let r2 = state.r2();
    let mut attachments = Vec::new();
    for attachment in AttachmentRepository::find_by_comment_id(state.pool(), comment_id).await? {
        let file_url = if let Some(service) = r2 {
            service.create_read_url(&attachment.blob_path).await.ok()
        } else {
            None
        };
        attachments.push(AttachmentWithUrl {
            attachment,
            file_url,
        });
    }
    Ok(Json(ListAttachmentsResponse { attachments }))
}

#[instrument(name = "attachments.get_file", skip(state, ctx), fields(attachment_id = %id, user_id = %ctx.user.id))]
async fn get_attachment_file(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path(id): Path<Uuid>,
) -> Result<Json<AttachmentUrlResponse>, RouteError> {
    let attachment = AttachmentRepository::find_by_id_with_blob(state.pool(), id)
        .await?
        .ok_or(RouteError::NotFound)?;

    ensure_attachment_access(&state, ctx.user.id, &attachment).await?;

    let r2 = state.r2().ok_or(RouteError::NotConfigured)?;
    let url = r2.create_read_url(&attachment.blob_path).await?;
    Ok(Json(AttachmentUrlResponse { url }))
}

#[instrument(name = "attachments.get_thumbnail", skip(state, ctx), fields(attachment_id = %id, user_id = %ctx.user.id))]
async fn get_attachment_thumbnail(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path(id): Path<Uuid>,
) -> Result<Json<AttachmentUrlResponse>, RouteError> {
    let attachment = AttachmentRepository::find_by_id_with_blob(state.pool(), id)
        .await?
        .ok_or(RouteError::NotFound)?;

    ensure_attachment_access(&state, ctx.user.id, &attachment).await?;

    let thumbnail_path = attachment
        .thumbnail_blob_path
        .ok_or(RouteError::NoThumbnail)?;
    let r2 = state.r2().ok_or(RouteError::NotConfigured)?;
    let url = r2.create_read_url(&thumbnail_path).await?;
    Ok(Json(AttachmentUrlResponse { url }))
}

#[instrument(name = "attachments.delete", skip(state, ctx), fields(attachment_id = %id, user_id = %ctx.user.id))]
async fn delete_attachment(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, RouteError> {
    let attachment = AttachmentRepository::find_by_id_with_blob(state.pool(), id)
        .await?
        .ok_or(RouteError::NotFound)?;

    ensure_attachment_access(&state, ctx.user.id, &attachment).await?;

    let blob_id = attachment.blob_id;
    AttachmentRepository::delete(state.pool(), id).await?;

    let remaining = AttachmentRepository::count_by_blob_id(state.pool(), blob_id).await?;
    if remaining == 0
        && let Some(blob) = BlobRepository::delete(state.pool(), blob_id).await?
    {
        let r2 = state.r2().ok_or(RouteError::NotConfigured)?;
        if let Err(e) = r2.delete_blob(&blob.blob_path).await {
            tracing::warn!(error = %e, blob_path = %blob.blob_path, "Failed to delete blob");
        }
        if let Some(thumb_path) = blob.thumbnail_blob_path
            && let Err(e) = r2.delete_blob(&thumb_path).await
        {
            tracing::warn!(error = %e, blob_path = %thumb_path, "Failed to delete thumbnail");
        }
    }

    Ok(StatusCode::NO_CONTENT)
}

async fn ensure_attachment_access(
    state: &AppState,
    user_id: Uuid,
    attachment: &AttachmentWithBlob,
) -> Result<(), RouteError> {
    if let Some(issue_id) = attachment.issue_id {
        ensure_issue_access(state.pool(), user_id, issue_id)
            .await
            .map_err(|_| RouteError::AccessDenied)?;
    } else if let Some(comment_id) = attachment.comment_id {
        ensure_comment_access(state.pool(), user_id, comment_id)
            .await
            .map_err(|_| RouteError::AccessDenied)?;
    } else if let Some(project_id) =
        AttachmentRepository::project_id(state.pool(), attachment.id).await?
    {
        ensure_project_access(state.pool(), user_id, project_id)
            .await
            .map_err(|_| RouteError::AccessDenied)?;
    } else {
        return Err(RouteError::AccessDenied);
    }
    Ok(())
}

fn sanitize_filename(filename: &str) -> String {
    filename
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '.' || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .take(100)
        .collect()
}
