use std::{net::SocketAddr, sync::Arc};

use chrono::{Duration as ChronoDuration, Utc};
use axum::{
    Json, Router,
    body::{Body, Bytes},
    extract::{Path, Query, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode, Uri, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use kodamapub_activitypub::{
    IncomingActivity, fetch_remote_actor, is_publicly_visible, local_actor_to_object,
    ordered_collection, ordered_collection_page, parse_incoming_activity, post_to_create_activity,
    post_to_note, signature_key_id_actor_url, verify_incoming_activity_signature,
    webfinger_response,
};
use kodamapub_db::{Database, DbError};
use kodamapub_domain::{
    ContentFormat, ContentSource, DomainError, NewPost, Post, PostId, PublicBaseUrl, Username,
    Visibility,
};
use kodamapub_job::{JobError, RetryPolicy, enqueue_create_deliveries};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone)]
struct AppState {
    db: Database,
    public_base_url: PublicBaseUrl,
    remote_client: reqwest::Client,
}

const MAX_INBOX_BODY_BYTES: usize = 1_048_576;
const SESSION_COOKIE_NAME: &str = "kodamapub_session";

#[derive(Debug, Deserialize)]
struct CreatePostRequest {
    content_source: ContentSource,
    content_format: ContentFormat,
    visibility: Visibility,
    in_reply_to: Option<PostId>,
}

#[derive(Debug, Deserialize)]
struct LoginRequest {
    username: Username,
    password: String,
}

#[derive(Debug, Deserialize)]
struct ListPostsQuery {
    limit: Option<i64>,
    before: Option<PostId>,
}

#[derive(Debug, Deserialize)]
struct OutboxQuery {
    page: Option<bool>,
    before: Option<PostId>,
    limit: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct WebFingerQuery {
    resource: String,
}

#[derive(Debug, Deserialize)]
struct UsernamePath {
    username: Username,
}

#[derive(Debug, Deserialize)]
struct PostIdPath {
    post_id: PostId,
}

#[derive(Debug, Serialize)]
struct HealthResponse {
    status: &'static str,
}

#[derive(Debug, Serialize)]
struct PostPageResponse {
    posts: Vec<Post>,
    next_cursor: Option<PostId>,
}

#[derive(Debug, Clone)]
struct ServerConfig {
    public_base_url: PublicBaseUrl,
}

#[derive(Debug)]
enum ApiError {
    NotFound(&'static str),
    Unauthorized(&'static str),
    Forbidden(&'static str),
    #[allow(dead_code)]
    BadRequest(String),
    Internal(anyhow::Error),
}

impl From<DbError> for ApiError {
    fn from(value: DbError) -> Self {
        Self::Internal(value.into())
    }
}

impl From<DomainError> for ApiError {
    fn from(value: DomainError) -> Self {
        match value {
            DomainError::InvalidPublicBaseUrl(_) => Self::Internal(value.into()),
            DomainError::NotFound => Self::NotFound("resource not found"),
        }
    }
}

impl From<JobError> for ApiError {
    fn from(value: JobError) -> Self {
        Self::Internal(value.into())
    }
}

impl From<kodamapub_db::AuthError> for ApiError {
    fn from(value: kodamapub_db::AuthError) -> Self {
        Self::Internal(value.into())
    }
}

impl From<kodamapub_activitypub::ActivityPubError> for ApiError {
    fn from(value: kodamapub_activitypub::ActivityPubError) -> Self {
        Self::Internal(value.into())
    }
}

impl AppState {
    fn remote_client(&self) -> &reqwest::Client {
        &self.remote_client
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        match self {
            ApiError::NotFound(message) => (StatusCode::NOT_FOUND, message).into_response(),
            ApiError::Unauthorized(message) => {
                (StatusCode::UNAUTHORIZED, message).into_response()
            }
            ApiError::Forbidden(message) => (StatusCode::FORBIDDEN, message).into_response(),
            ApiError::BadRequest(message) => (StatusCode::BAD_REQUEST, message).into_response(),
            ApiError::Internal(error) => {
                tracing::error!(error = %error, "request failed");
                (StatusCode::INTERNAL_SERVER_ERROR, "internal server error").into_response()
            }
        }
    }
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse { status: "ok" })
}

fn build_app(state: Arc<AppState>, config: ServerConfig) -> Router {
    let api = Router::new()
        .route("/.well-known/webfinger", get(get_webfinger))
        .route("/login", post(post_login))
        .route("/logout", post(post_logout))
        .route("/session", get(get_session))
        .route("/posts/{post_id}", get(get_post_activitypub))
        .route("/users/{username}", get(get_actor))
        .route("/users/{username}/outbox", get(get_outbox))
        .route("/users/{username}/inbox", post(post_inbox))
        .route(
            "/users/{username}/posts",
            get(list_user_posts).post(create_user_post),
        )
        .with_state((state.clone(), config.clone()));

    Router::new()
        .route("/health", get(health))
        .nest("/api", api)
        .with_state((state, config))
}

async fn list_user_posts(
    State((state, _config)): State<(Arc<AppState>, ServerConfig)>,
    Path(path): Path<UsernamePath>,
    Query(query): Query<ListPostsQuery>,
) -> Result<Json<PostPageResponse>, ApiError> {
    let actor = state
        .db
        .local_actors()
        .find_by_username(&path.username)
        .await?
        .ok_or(ApiError::NotFound("local actor not found"))?;
    let limit = query.limit.unwrap_or(20).clamp(1, 100);
    let page = state
        .db
        .posts()
        .list_by_actor(actor.id(), query.before, limit)
        .await?;
    Ok(Json(PostPageResponse {
        posts: page.posts,
        next_cursor: page.next_cursor,
    }))
}

async fn create_user_post(
    State((state, _config)): State<(Arc<AppState>, ServerConfig)>,
    Path(path): Path<UsernamePath>,
    headers: HeaderMap,
    Json(request): Json<CreatePostRequest>,
) -> Result<(StatusCode, Json<Post>), ApiError> {
    let session_actor = require_session_actor(&state, &headers).await?;
    if session_actor.profile.username != path.username {
        return Err(ApiError::Forbidden("cannot post as another user"));
    }

    let actor = state
        .db
        .local_actors()
        .find_by_username(&path.username)
        .await?
        .ok_or(ApiError::NotFound("local actor not found"))?;

    let post = Post::new(
        NewPost {
            actor_id: actor.id(),
            content_source: request.content_source,
            content_format: request.content_format,
            visibility: request.visibility,
            in_reply_to: request.in_reply_to,
        },
        &state.public_base_url,
    )?;

    state.db.posts().create(&post).await?;
    enqueue_create_deliveries(&state.db, &actor, &post, &RetryPolicy::default()).await?;
    Ok((StatusCode::CREATED, Json(post)))
}

async fn get_actor(
    State((state, _config)): State<(Arc<AppState>, ServerConfig)>,
    Path(path): Path<UsernamePath>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let actor = state
        .db
        .local_actors()
        .find_by_username(&path.username)
        .await?
        .ok_or(ApiError::NotFound("local actor not found"))?;

    if wants_activitypub(&headers) {
        return activitypub_response(&local_actor_to_object(&actor));
    }

    Ok(Json(actor.profile).into_response())
}

async fn get_post_activitypub(
    State((state, _config)): State<(Arc<AppState>, ServerConfig)>,
    Path(path): Path<PostIdPath>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let post = state
        .db
        .posts()
        .find(path.post_id)
        .await?
        .ok_or(ApiError::NotFound("post not found"))?;

    if !is_publicly_visible(&post.visibility) {
        return Err(ApiError::NotFound("post not found"));
    }

    let actor = find_local_actor_by_id(&state, post.actor_id).await?;

    if wants_activitypub(&headers) {
        return activitypub_response(&post_to_note(&post, &actor.profile));
    }

    Ok(Json(post).into_response())
}

async fn get_outbox(
    State((state, _config)): State<(Arc<AppState>, ServerConfig)>,
    Path(path): Path<UsernamePath>,
    Query(query): Query<OutboxQuery>,
) -> Result<Response, ApiError> {
    let actor = state
        .db
        .local_actors()
        .find_by_username(&path.username)
        .await?
        .ok_or(ApiError::NotFound("local actor not found"))?;
    let outbox_url = actor
        .profile
        .outbox_url
        .clone()
        .ok_or_else(|| ApiError::Internal(anyhow::anyhow!("local actor missing outbox url")))?;

    if query.page.unwrap_or(false) {
        let limit = query.limit.unwrap_or(20).clamp(1, 100);
        let page = state
            .db
            .posts()
            .list_public_by_actor(actor.id(), query.before, limit)
            .await?;

        let ordered_items = page
            .posts
            .iter()
            .map(|post| post_to_create_activity(post, &actor.profile))
            .collect();

        let mut page_id = outbox_url.clone();
        page_id.query_pairs_mut().append_pair("page", "true");
        if let Some(before) = query.before {
            page_id
                .query_pairs_mut()
                .append_pair("before", &before.to_string());
        }
        if let Some(limit) = query.limit {
            page_id
                .query_pairs_mut()
                .append_pair("limit", &limit.to_string());
        }

        let next = page.next_cursor.map(|cursor| {
            let mut next_url = outbox_url.clone();
            next_url.query_pairs_mut().append_pair("page", "true");
            next_url
                .query_pairs_mut()
                .append_pair("before", &cursor.to_string());
            if let Some(limit) = query.limit {
                next_url
                    .query_pairs_mut()
                    .append_pair("limit", &limit.to_string());
            }
            next_url
        });

        return activitypub_response(&ordered_collection_page(
            page_id,
            outbox_url,
            next,
            ordered_items,
        ));
    }

    let total_items = state.db.posts().count_public_by_actor(actor.id()).await? as u64;
    let mut first = actor.profile.outbox_url.clone();
    if let Some(url) = &mut first {
        url.query_pairs_mut().append_pair("page", "true");
    }

    activitypub_response(&ordered_collection(outbox_url, first, total_items))
}

async fn get_webfinger(
    State((state, config)): State<(Arc<AppState>, ServerConfig)>,
    Query(query): Query<WebFingerQuery>,
) -> Result<Response, ApiError> {
    let parsed = parse_webfinger_resource(&config.public_base_url, &query.resource)
        .ok_or_else(|| ApiError::BadRequest("invalid webfinger resource".to_string()))?;
    let actor = state
        .db
        .local_actors()
        .find_by_username(&parsed)
        .await?
        .ok_or(ApiError::NotFound("local actor not found"))?;

    jrd_response(&webfinger_response(query.resource, actor.profile.actor_url))
}

async fn post_login(
    State((state, _config)): State<(Arc<AppState>, ServerConfig)>,
    Json(request): Json<LoginRequest>,
) -> Result<Response, ApiError> {
    let actor = state
        .db
        .local_actors()
        .find_by_username(&request.username)
        .await?
        .ok_or(ApiError::Unauthorized("invalid credentials"))?;
    let Some((_, password_hash)) = state
        .db
        .local_actor_credentials()
        .find_password_hash_by_username(&request.username)
        .await?
    else {
        return Err(ApiError::Unauthorized("invalid credentials"));
    };

    if !kodamapub_db::verify_password(&request.password, &password_hash)? {
        return Err(ApiError::Unauthorized("invalid credentials"));
    }

    let token = Uuid::now_v7().to_string();
    let expires_at = Utc::now() + ChronoDuration::days(30);
    state
        .db
        .sessions()
        .create(&token, actor.id(), expires_at)
        .await?;

    let secure = state.public_base_url.as_str().starts_with("https://");
    session_response(
        actor.profile,
        Some(build_session_cookie(&token, expires_at, secure)?),
    )
}

async fn get_session(
    State((state, _config)): State<(Arc<AppState>, ServerConfig)>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let actor = require_session_actor(&state, &headers).await?;
    session_response(actor.profile, None)
}

async fn post_logout(
    State((state, _config)): State<(Arc<AppState>, ServerConfig)>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    if let Some(token) = session_token_from_headers(&headers) {
        state.db.sessions().delete(&token).await?;
    }

    let mut response = Response::new(Body::empty());
    response.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&clear_session_cookie(
            state.public_base_url.as_str().starts_with("https://"),
        ))
            .map_err(|error| ApiError::BadRequest(error.to_string()))?,
    );
    Ok(response)
}

async fn require_session_actor(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<kodamapub_domain::LocalActor, ApiError> {
    let token = session_token_from_headers(headers)
        .ok_or(ApiError::Unauthorized("session is required"))?;
    let Some(actor_id) = state.db.sessions().find_actor_id(&token).await? else {
        return Err(ApiError::Unauthorized("session is required"));
    };

    state
        .db
        .local_actors()
        .find_by_id(actor_id)
        .await?
        .ok_or(ApiError::Unauthorized("session is required"))
}

fn session_token_from_headers(headers: &HeaderMap) -> Option<String> {
    headers.get(header::COOKIE).and_then(|value| value.to_str().ok()).and_then(|cookies| {
        cookies
            .split(';')
            .map(str::trim)
            .find_map(|cookie| cookie.strip_prefix(&format!("{SESSION_COOKIE_NAME}=")))
            .map(str::to_string)
    })
}

fn session_response(actor: kodamapub_domain::ActorProfile, cookie: Option<String>) -> Result<Response, ApiError> {
    let mut response = Json(actor).into_response();
    if let Some(cookie) = cookie {
        response.headers_mut().insert(
            header::SET_COOKIE,
            HeaderValue::from_str(&cookie)
                .map_err(|error| ApiError::BadRequest(error.to_string()))?,
        );
    }
    Ok(response)
}

fn build_session_cookie(
    token: &str,
    expires_at: chrono::DateTime<Utc>,
    secure: bool,
) -> Result<String, ApiError> {
    let max_age = expires_at.signed_duration_since(Utc::now()).num_seconds().max(0);
    let secure_flag = if secure { "; Secure" } else { "" };
    Ok(format!(
        "{SESSION_COOKIE_NAME}={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={max_age}{secure_flag}"
    ))
}

fn clear_session_cookie(secure: bool) -> String {
    let secure_flag = if secure { "; Secure" } else { "" };
    format!(
        "{SESSION_COOKIE_NAME}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0{secure_flag}"
    )
}

async fn post_inbox(
    State((state, _config)): State<(Arc<AppState>, ServerConfig)>,
    Path(path): Path<UsernamePath>,
    headers: HeaderMap,
    method: Method,
    uri: Uri,
    body: Bytes,
) -> Result<StatusCode, ApiError> {
    let local_actor = state
        .db
        .local_actors()
        .find_by_username(&path.username)
        .await?
        .ok_or(ApiError::NotFound("local actor not found"))?;

    ensure_inbox_content_type(&headers)?;
    if body.len() > MAX_INBOX_BODY_BYTES {
        return Err(ApiError::BadRequest("inbox payload too large".to_string()));
    }

    let body_text = std::str::from_utf8(&body)
        .map_err(|_| ApiError::BadRequest("inbox payload must be utf-8".to_string()))?;
    let activity = parse_incoming_activity(body_text)?;
    let activity_id = incoming_activity_id(&activity);
    let activity_type = incoming_activity_type(&activity);

    let key_actor_url = signature_key_id_actor_url(&headers)?;
    let activity_actor_url = incoming_activity_actor_url(&activity);
    if strip_fragment(&key_actor_url) != strip_fragment(&activity_actor_url) {
        return Err(ApiError::BadRequest(
            "signature actor does not match activity actor".to_string(),
        ));
    }

    let remote_actor = fetch_remote_actor(state.remote_client(), &key_actor_url).await?;
    state.db.remote_actors().upsert(&remote_actor).await?;

    let signature_target = uri
        .path_and_query()
        .map(|value| value.as_str())
        .unwrap_or_else(|| uri.path());
    verify_incoming_activity_signature(
        &headers,
        method.as_str(),
        signature_target,
        body.as_ref(),
        remote_actor
            .public_key_pem
            .as_deref()
            .ok_or_else(|| ApiError::BadRequest("remote actor missing public key".to_string()))?,
    )?;

    if !state
        .db
        .inbox_dedup()
        .record(&activity_id, remote_actor.id(), &activity_type)
        .await?
    {
        return Ok(StatusCode::ACCEPTED);
    }

    match activity {
        IncomingActivity::Follow(follow) => {
            handle_follow_activity(&state, &local_actor, &remote_actor, follow).await?;
        }
        IncomingActivity::Create(create) => {
            handle_create_activity(&state, &remote_actor, create).await?;
        }
        IncomingActivity::Accept(_) | IncomingActivity::Undo(_) | IncomingActivity::Delete(_) => {}
    }

    Ok(StatusCode::ACCEPTED)
}

async fn find_local_actor_by_id(
    state: &AppState,
    actor_id: kodamapub_domain::ActorId,
) -> Result<kodamapub_domain::LocalActor, ApiError> {
    state
        .db
        .local_actors()
        .find_by_id(actor_id)
        .await?
        .ok_or(ApiError::NotFound("local actor not found"))
}

fn wants_activitypub(headers: &HeaderMap) -> bool {
    headers
        .get(header::ACCEPT)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|accept| {
            accept.contains("application/activity+json") || accept.contains("application/ld+json")
        })
}

fn parse_webfinger_resource(public_base_url: &PublicBaseUrl, resource: &str) -> Option<Username> {
    let authority = webfinger_authority(public_base_url)?;
    let expected_prefix = format!("acct:");
    let value = resource.strip_prefix(&expected_prefix)?;
    let (username, host) = value.split_once('@')?;
    if host != authority {
        return None;
    }

    username.parse().ok()
}

fn webfinger_authority(public_base_url: &PublicBaseUrl) -> Option<String> {
    let url = url::Url::parse(public_base_url.as_str()).ok()?;
    let host = url.host_str()?.to_string();
    let authority = match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host,
    };

    Some(authority)
}

fn activitypub_response<T: serde::Serialize>(value: &T) -> Result<Response, ApiError> {
    let body =
        serde_json::to_vec(value).map_err(|error| ApiError::Internal(anyhow::Error::new(error)))?;
    let mut response = Response::new(Body::from(body));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/activity+json"),
    );
    Ok(response)
}

fn jrd_response<T: serde::Serialize>(value: &T) -> Result<Response, ApiError> {
    let body =
        serde_json::to_vec(value).map_err(|error| ApiError::Internal(anyhow::Error::new(error)))?;
    let mut response = Response::new(Body::from(body));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/jrd+json"),
    );
    Ok(response)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _ = dotenvy::dotenv();
    tracing_subscriber::fmt::init();

    let database_url =
        std::env::var("DATABASE_URL").unwrap_or_else(|_| "sqlite://kodamapub.db".to_string());
    let public_base_url = std::env::var("PUBLIC_BASE_URL")
        .unwrap_or_else(|_| "http://127.0.0.1:3000".to_string())
        .parse()?;
    let bind_addr = std::env::var("BIND_ADDR").unwrap_or_else(|_| "127.0.0.1:3000".to_string());

    let db = Database::connect(&database_url).await?;
    db.migrate().await?;

    let state = Arc::new(AppState {
        db,
        public_base_url,
        remote_client: reqwest::Client::new(),
    });
    let config = ServerConfig {
        public_base_url: state.public_base_url.clone(),
    };
    let app = build_app(state, config);

    let listener = tokio::net::TcpListener::bind(bind_addr.parse::<SocketAddr>()?).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

async fn handle_follow_activity(
    state: &AppState,
    local_actor: &kodamapub_domain::LocalActor,
    remote_actor: &kodamapub_domain::RemoteActor,
    follow: kodamapub_activitypub::IncomingFollowActivity,
) -> Result<(), ApiError> {
    if remote_actor.profile.inbox_url.is_none() {
        tracing::warn!(
            remote_actor = %remote_actor.profile.actor_url,
            "received follow from remote actor without inbox url"
        );
        return Ok(());
    }

    let job = kodamapub_job::enqueue_accept_delivery(
        &state.db,
        local_actor,
        remote_actor,
        &follow.id,
        &RetryPolicy::default(),
    )
    .await?;
    tracing::info!(job_id = %job.id.0, "queued accept delivery for inbound follow");
    Ok(())
}

async fn handle_create_activity(
    state: &AppState,
    remote_actor: &kodamapub_domain::RemoteActor,
    create: kodamapub_activitypub::IncomingCreateActivity,
) -> Result<(), ApiError> {
    let in_reply_to = match create.object.in_reply_to {
        Some(reply_to) => state
            .db
            .posts()
            .find_by_url(&reply_to)
            .await?
            .map(|post| post.id),
        None => None,
    };
    let visibility = visibility_from_audience(&create.object.to, &create.object.cc);
    let content_source = create
        .object
        .content
        .parse::<ContentSource>()
        .map_err(|error| ApiError::BadRequest(error.to_string()))?;
    let post = Post {
        id: kodamapub_domain::PostId::new(),
        actor_id: remote_actor.id(),
        url: create.object.id,
        content_source,
        content_format: ContentFormat::Plaintext,
        content_html: create.object.content,
        visibility,
        in_reply_to,
        created_at: create.object.published,
    };

    state.db.posts().upsert_remote(&post).await?;
    Ok(())
}

fn visibility_from_audience(to: &[url::Url], cc: &[url::Url]) -> Visibility {
    let public = "https://www.w3.org/ns/activitystreams#Public";
    let followers = to
        .iter()
        .chain(cc.iter())
        .any(|url| url.path().ends_with("/followers"));

    if to.iter().chain(cc.iter()).any(|url| url.as_str() == public) {
        Visibility::Public
    } else if followers {
        Visibility::Followers
    } else if !to.is_empty() || !cc.is_empty() {
        Visibility::Direct
    } else {
        Visibility::Unlisted
    }
}

fn incoming_activity_id(activity: &IncomingActivity) -> String {
    match activity {
        IncomingActivity::Follow(value) => value.id.to_string(),
        IncomingActivity::Create(value) => value.id.to_string(),
        IncomingActivity::Accept(value) => value.id.to_string(),
        IncomingActivity::Undo(value) => value.id.to_string(),
        IncomingActivity::Delete(value) => value.id.to_string(),
    }
}

fn incoming_activity_type(activity: &IncomingActivity) -> String {
    match activity {
        IncomingActivity::Follow(_) => "Follow",
        IncomingActivity::Create(_) => "Create",
        IncomingActivity::Accept(_) => "Accept",
        IncomingActivity::Undo(_) => "Undo",
        IncomingActivity::Delete(_) => "Delete",
    }
    .to_string()
}

fn incoming_activity_actor_url(activity: &IncomingActivity) -> url::Url {
    match activity {
        IncomingActivity::Follow(value) => value.actor.clone(),
        IncomingActivity::Create(value) => value.actor.clone(),
        IncomingActivity::Accept(value) => value.actor.clone(),
        IncomingActivity::Undo(value) => value.actor.clone(),
        IncomingActivity::Delete(value) => value.actor.clone(),
    }
}

fn strip_fragment(url: &url::Url) -> String {
    let mut value = url.clone();
    value.set_fragment(None);
    value.as_str().to_string()
}

fn ensure_inbox_content_type(headers: &HeaderMap) -> Result<(), ApiError> {
    let Some(content_type) = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
    else {
        return Err(ApiError::BadRequest("missing content-type".to_string()));
    };

    if content_type.contains("application/activity+json")
        || content_type.contains("application/ld+json")
        || content_type.contains("application/json")
    {
        Ok(())
    } else {
        Err(ApiError::BadRequest(
            "unsupported content-type for inbox".to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::to_bytes, http::Request};
    use kodamapub_domain::{ActorProfile, LocalActor};
    use tower::ServiceExt;
    use url::Url;

    async fn test_db() -> Database {
        let db = Database::connect("sqlite::memory:")
            .await
            .expect("connect sqlite memory db");
        db.migrate().await.expect("run migrations");
        db
    }

    fn sample_local_actor() -> LocalActor {
        LocalActor {
            profile: ActorProfile::new(
                "alice".parse().expect("username"),
                "Alice".parse().expect("display name"),
                Some("summary".parse().expect("summary")),
                Url::parse("https://example.invalid/users/alice").expect("actor url"),
                Some(Url::parse("https://example.invalid/users/alice/inbox").expect("inbox url")),
                Some(Url::parse("https://example.invalid/users/alice/outbox").expect("outbox url")),
            ),
            public_key_pem: "PUBLIC KEY".to_string(),
            private_key_pem: "PRIVATE KEY".to_string(),
        }
    }

    async fn test_app() -> Router {
        let db = test_db().await;
        let actor = sample_local_actor();
        db.local_actors()
            .create(&actor)
            .await
            .expect("create actor");

        for visibility in [Visibility::Public, Visibility::Unlisted, Visibility::Direct] {
            let post = Post::new(
                NewPost {
                    actor_id: actor.id(),
                    content_source: format!("{visibility:?} post")
                        .parse()
                        .expect("content source"),
                    content_format: ContentFormat::Plaintext,
                    visibility,
                    in_reply_to: None,
                },
                &"https://example.invalid".parse().expect("public base url"),
            )
            .expect("post");
            db.posts().create(&post).await.expect("insert post");
        }

        let state = Arc::new(AppState {
            db,
            public_base_url: "https://example.invalid".parse().expect("public base url"),
            remote_client: reqwest::Client::new(),
        });

        build_app(
            state.clone(),
            ServerConfig {
                public_base_url: state.public_base_url.clone(),
            },
        )
    }

    #[tokio::test]
    async fn actor_endpoint_returns_activitypub_object_for_activity_accept() {
        let app = test_app().await;

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/users/alice")
                    .header(header::ACCEPT, "application/activity+json")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "application/activity+json"
        );

        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let json: serde_json::Value = serde_json::from_slice(&body).expect("json");
        assert_eq!(json["type"], "Person");
        assert_eq!(json["preferredUsername"], "alice");
        assert_eq!(
            json["publicKey"]["owner"],
            "https://example.invalid/users/alice"
        );
    }

    #[tokio::test]
    async fn post_endpoint_returns_note_for_public_post() {
        let app = test_app().await;
        let post_id = {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri("/api/users/alice/posts?limit=10")
                        .body(Body::empty())
                        .expect("request"),
                )
                .await
                .expect("response");
            let body = to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("body");
            let json: serde_json::Value = serde_json::from_slice(&body).expect("json");
            json["posts"]
                .as_array()
                .expect("posts")
                .iter()
                .find(|post| post["visibility"] == "Public")
                .and_then(|post| post["id"].as_str())
                .expect("public post id")
                .to_string()
        };

        let response = app
            .oneshot(
                Request::builder()
                    .uri(format!("/api/posts/{post_id}"))
                    .header(header::ACCEPT, "application/activity+json")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let json: serde_json::Value = serde_json::from_slice(&body).expect("json");
        assert_eq!(json["type"], "Note");
    }

    #[tokio::test]
    async fn outbox_collection_and_page_expose_only_public_items() {
        let app = test_app().await;

        let collection_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/users/alice/outbox")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(collection_response.status(), StatusCode::OK);
        let collection_body = to_bytes(collection_response.into_body(), usize::MAX)
            .await
            .expect("body");
        let collection_json: serde_json::Value =
            serde_json::from_slice(&collection_body).expect("json");
        assert_eq!(collection_json["type"], "OrderedCollection");
        assert_eq!(collection_json["totalItems"], 2);

        let page_response = app
            .oneshot(
                Request::builder()
                    .uri("/api/users/alice/outbox?page=true&limit=10")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(page_response.status(), StatusCode::OK);
        let page_body = to_bytes(page_response.into_body(), usize::MAX)
            .await
            .expect("body");
        let page_json: serde_json::Value = serde_json::from_slice(&page_body).expect("json");
        assert_eq!(page_json["type"], "OrderedCollectionPage");
        assert_eq!(
            page_json["orderedItems"]
                .as_array()
                .expect("ordered items")
                .len(),
            2
        );
        for item in page_json["orderedItems"].as_array().expect("ordered items") {
            assert_eq!(item["type"], "Create");
            assert_eq!(item["object"]["type"], "Note");
        }
    }

    #[tokio::test]
    async fn webfinger_returns_actor_link() {
        let app = test_app().await;

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/.well-known/webfinger?resource=acct:alice@example.invalid")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "application/jrd+json"
        );

        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let json: serde_json::Value = serde_json::from_slice(&body).expect("json");
        assert_eq!(json["subject"], "acct:alice@example.invalid");
        assert_eq!(
            json["links"][0]["href"],
            "https://example.invalid/users/alice"
        );
    }
}
