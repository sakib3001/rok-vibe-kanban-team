mod credential;
mod handoff;
mod jwt;
mod local;
mod middleware;
mod oauth_token_validator;
mod provider;

pub(crate) use credential::{
    CredentialAuthError, change_password as credential_change_password,
    complete_password_reset as credential_complete_password_reset,
    login as credential_login, provision_member as credential_provision_member,
    request_password_reset as credential_request_password_reset,
};
pub(crate) use handoff::{CallbackResult, HandoffError, OAuthHandoffService};
pub(crate) use jwt::{JwtError, JwtService};
pub(crate) use local::{LocalAuthError, auth_methods_response, is_local_provider, login};
pub(crate) use middleware::{RequestContext, require_session};
pub(crate) use oauth_token_validator::{OAuthTokenValidationError, OAuthTokenValidator};
pub(crate) use provider::{
    GitHubOAuthProvider, GoogleOAuthProvider, ProviderRegistry, ProviderTokenDetails,
    ZohoOAuthProvider,
};
