use std::{error::Error as StdError, time::Duration};

use aws_credential_types::Credentials;
use aws_sdk_s3::{
    Client,
    config::{Builder as S3ConfigBuilder, IdentityCache},
    presigning::PresigningConfig,
    primitives::ByteStream,
};
use chrono::{DateTime, Utc};
use secrecy::ExposeSecret;
use uuid::Uuid;

use crate::config::R2Config;

/// Well-known filename for the payload tarball stored in each review folder.
pub const PAYLOAD_FILENAME: &str = "payload.tar.gz";

#[derive(Clone)]
pub struct R2Service {
    client: Client,
    bucket: String,
    presign_expiry: Duration,
}

#[derive(Debug)]
pub struct PresignedUpload {
    pub upload_url: String,
    pub object_key: String,
    /// Folder path in R2 (e.g., "reviews/{review_id}") - this is stored in the database.
    pub folder_path: String,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug)]
pub struct AttachmentUpload {
    pub upload_url: String,
    pub blob_path: String,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug)]
pub struct BlobProperties {
    pub content_length: i64,
}

#[derive(Debug, thiserror::Error)]
pub enum R2Error {
    #[error("presign config error: {0}")]
    PresignConfig(String),
    #[error("presign error: {0}")]
    Presign(String),
    #[error("upload error: {0}")]
    Upload(String),
    #[error("storage error: {0}")]
    Storage(String),
}

fn format_error_chain<E: StdError>(err: &E) -> String {
    let mut message = err.to_string();
    let mut source = err.source();

    while let Some(cause) = source {
        message.push_str(": ");
        message.push_str(&cause.to_string());
        source = cause.source();
    }

    message
}

impl R2Service {
    pub fn new(config: &R2Config) -> Self {
        let credentials = Credentials::new(
            &config.access_key_id,
            config.secret_access_key.expose_secret(),
            None,
            None,
            "r2-static",
        );

        let s3_config =
            S3ConfigBuilder::new()
                .region(aws_sdk_s3::config::Region::new("auto"))
                .endpoint_url(&config.endpoint)
                .credentials_provider(credentials)
                .force_path_style(true)
                .stalled_stream_protection(
                    aws_sdk_s3::config::StalledStreamProtectionConfig::disabled(),
                )
                .identity_cache(IdentityCache::no_cache())
                .build();

        let client = Client::from_conf(s3_config);

        Self {
            client,
            bucket: config.bucket.clone(),
            presign_expiry: Duration::from_secs(config.presign_expiry_secs),
        }
    }

    pub async fn create_presigned_upload(
        &self,
        review_id: Uuid,
        content_type: Option<&str>,
    ) -> Result<PresignedUpload, R2Error> {
        let folder_path = format!("reviews/{review_id}");
        let object_key = format!("{folder_path}/{PAYLOAD_FILENAME}");

        let presigning_config = PresigningConfig::builder()
            .expires_in(self.presign_expiry)
            .build()
            .map_err(|e| R2Error::PresignConfig(e.to_string()))?;

        let mut request = self
            .client
            .put_object()
            .bucket(&self.bucket)
            .key(&object_key);

        if let Some(ct) = content_type {
            request = request.content_type(ct);
        }

        let presigned = request
            .presigned(presigning_config)
            .await
            .map_err(|e| R2Error::Presign(format_error_chain(&e)))?;

        let expires_at = Utc::now()
            + chrono::Duration::from_std(self.presign_expiry).unwrap_or(chrono::Duration::hours(1));

        Ok(PresignedUpload {
            upload_url: presigned.uri().to_string(),
            object_key,
            folder_path,
            expires_at,
        })
    }

    pub async fn create_upload_url(
        &self,
        blob_path: &str,
        content_type: Option<&str>,
    ) -> Result<AttachmentUpload, R2Error> {
        let presigning_config = PresigningConfig::builder()
            .expires_in(self.presign_expiry)
            .build()
            .map_err(|e| R2Error::PresignConfig(e.to_string()))?;

        let mut request = self
            .client
            .put_object()
            .bucket(&self.bucket)
            .key(blob_path);

        if let Some(ct) = content_type {
            request = request.content_type(ct);
        }

        let presigned = request
            .presigned(presigning_config)
            .await
            .map_err(|e| R2Error::Presign(format_error_chain(&e)))?;

        let expires_at = Utc::now()
            + chrono::Duration::from_std(self.presign_expiry).unwrap_or(chrono::Duration::hours(1));

        Ok(AttachmentUpload {
            upload_url: presigned.uri().to_string(),
            blob_path: blob_path.to_string(),
            expires_at,
        })
    }

    pub async fn create_read_url(&self, blob_path: &str) -> Result<String, R2Error> {
        let presigning_config = PresigningConfig::builder()
            .expires_in(self.presign_expiry)
            .build()
            .map_err(|e| R2Error::PresignConfig(e.to_string()))?;

        let presigned = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(blob_path)
            .presigned(presigning_config)
            .await
            .map_err(|e| R2Error::Presign(format_error_chain(&e)))?;

        Ok(presigned.uri().to_string())
    }

    pub async fn get_blob_properties(&self, blob_path: &str) -> Result<BlobProperties, R2Error> {
        let output = self
            .client
            .head_object()
            .bucket(&self.bucket)
            .key(blob_path)
            .send()
            .await
            .map_err(|e| R2Error::Storage(format_error_chain(&e)))?;

        Ok(BlobProperties {
            content_length: output.content_length().unwrap_or(0),
        })
    }

    pub async fn download_blob(&self, blob_path: &str) -> Result<Vec<u8>, R2Error> {
        let output = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(blob_path)
            .send()
            .await
            .map_err(|e| R2Error::Storage(format_error_chain(&e)))?;

        let body = output
            .body
            .collect()
            .await
            .map_err(|e| R2Error::Storage(format_error_chain(&e)))?;

        Ok(body.into_bytes().to_vec())
    }

    pub async fn upload_blob(
        &self,
        blob_path: &str,
        data: Vec<u8>,
        content_type: String,
    ) -> Result<(), R2Error> {
        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(blob_path)
            .body(ByteStream::from(data))
            .content_type(content_type)
            .send()
            .await
            .map_err(|e| R2Error::Upload(format_error_chain(&e)))?;

        Ok(())
    }

    pub async fn delete_blob(&self, blob_path: &str) -> Result<(), R2Error> {
        self.client
            .delete_object()
            .bucket(&self.bucket)
            .key(blob_path)
            .send()
            .await
            .map_err(|e| R2Error::Storage(format_error_chain(&e)))?;

        Ok(())
    }

    /// Upload bytes directly to R2 (for server-side uploads).
    ///
    /// Returns the folder path (e.g., "reviews/{review_id}") to store in the database.
    pub async fn upload_bytes(&self, review_id: Uuid, data: Vec<u8>) -> Result<String, R2Error> {
        let folder_path = format!("reviews/{review_id}");
        let object_key = format!("{folder_path}/{PAYLOAD_FILENAME}");

        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(&object_key)
            .body(ByteStream::from(data))
            .content_type("application/gzip")
            .send()
            .await
            .map_err(|e| R2Error::Upload(format_error_chain(&e)))?;

        Ok(folder_path)
    }
}
