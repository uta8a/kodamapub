use std::{
    collections::{HashMap, VecDeque},
    net::SocketAddr,
    sync::Arc,
    time::{Duration, Instant},
};

use axum::{
    Json, Router,
    body::{Body, Bytes},
    extract::{Path, Query, State},
    http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, Uri, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use chrono::{Duration as ChronoDuration, Utc};
use kodamapub_activitypub::{
    IncomingActivity, discover_remote_actor, fetch_remote_actor, is_publicly_visible,
    local_actor_to_object, ordered_collection, ordered_collection_page, parse_incoming_activity,
    post_to_create_activity, post_to_note, signature_key_id_actor_url,
    verify_incoming_activity_signature, webfinger_response,
};
use kodamapub_db::{Database, DbError};
use kodamapub_domain::{
    ActorProfile, ContentFormat, ContentSource, DomainError, FollowRelation, LocalActor, NewPost,
    Post, PostId, PublicBaseUrl, Username, Visibility,
};
use kodamapub_job::{
    JobError, RetryPolicy, activate_follow_and_backfill, enqueue_create_deliveries,
};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tower::ServiceBuilder;
use tower_http::set_header::SetResponseHeaderLayer;
use url::Url;
use uuid::Uuid;

#[derive(Clone)]
struct AppState {
    db: Database,
    public_base_url: PublicBaseUrl,
    allowed_origins: Vec<String>,
    remote_client: reqwest::Client,
    rate_limiter: RateLimiter,
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
struct FollowRequest {
    resource: String,
}

#[derive(Debug, Serialize)]
struct FollowResponse {
    remote_actor: ActorProfile,
    state: kodamapub_domain::FollowState,
    job_id: String,
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
struct SessionResponse {
    actor: kodamapub_domain::ActorProfile,
    csrf_token: String,
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

#[derive(Clone)]
struct RateLimiter {
    buckets: Arc<Mutex<HashMap<String, VecDeque<Instant>>>>,
}

#[derive(Clone, Copy)]
struct RateLimitSpec {
    limit: usize,
    window: Duration,
}

impl RateLimiter {
    fn new() -> Self {
        Self {
            buckets: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    async fn check(&self, key: String, spec: RateLimitSpec) -> Result<(), ApiError> {
        let now = Instant::now();
        let mut buckets = self.buckets.lock().await;
        let bucket = buckets.entry(key).or_default();

        while bucket
            .front()
            .is_some_and(|timestamp| now.duration_since(*timestamp) >= spec.window)
        {
            bucket.pop_front();
        }

        if bucket.len() >= spec.limit {
            let retry_after = bucket
                .front()
                .map(|timestamp| spec.window.saturating_sub(now.duration_since(*timestamp)))
                .unwrap_or(spec.window);
            return Err(ApiError::TooManyRequests {
                message: "too many requests".to_string(),
                retry_after: Some(retry_after),
            });
        }

        bucket.push_back(now);
        Ok(())
    }
}

#[derive(Debug)]
enum ApiError {
    NotFound(&'static str),
    Unauthorized(&'static str),
    Forbidden(&'static str),
    TooManyRequests {
        message: String,
        retry_after: Option<Duration>,
    },
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
            ApiError::Unauthorized(message) => (StatusCode::UNAUTHORIZED, message).into_response(),
            ApiError::Forbidden(message) => (StatusCode::FORBIDDEN, message).into_response(),
            ApiError::TooManyRequests {
                message,
                retry_after,
            } => {
                let mut response = (StatusCode::TOO_MANY_REQUESTS, message).into_response();
                if let Some(retry_after) = retry_after {
                    if let Ok(value) =
                        HeaderValue::from_str(&retry_after.as_secs().max(1).to_string())
                    {
                        response.headers_mut().insert(header::RETRY_AFTER, value);
                    }
                }
                response
            }
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
            "/users/{username}/follows",
            post(post_follow).delete(delete_follow),
        )
        .route(
            "/users/{username}/posts",
            get(list_user_posts).post(create_user_post),
        )
        .route("/users/{username}/timeline", get(list_home_timeline))
        .with_state((state.clone(), config.clone()));

    let security_headers = ServiceBuilder::new()
        .layer(SetResponseHeaderLayer::if_not_present(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::X_FRAME_OPTIONS,
            HeaderValue::from_static("DENY"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::REFERRER_POLICY,
            HeaderValue::from_static("same-origin"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            HeaderName::from_static("permissions-policy"),
            HeaderValue::from_static("camera=(), microphone=(), geolocation=()"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static("default-src 'none'; frame-ancestors 'none'"),
        ));

    Router::new()
        .route("/health", get(health))
        .nest("/api", api)
        .layer(security_headers)
        .with_state((state, config))
}

async fn list_user_posts(
    State((state, _config)): State<(Arc<AppState>, ServerConfig)>,
    Path(path): Path<UsernamePath>,
    Query(query): Query<ListPostsQuery>,
) -> Result<Json<PostPageResponse>, ApiError> {
    let actor = find_local_actor_by_username(&state, &path.username).await?;
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

async fn list_home_timeline(
    State((state, _config)): State<(Arc<AppState>, ServerConfig)>,
    Path(path): Path<UsernamePath>,
    headers: HeaderMap,
    Query(query): Query<ListPostsQuery>,
) -> Result<Json<PostPageResponse>, ApiError> {
    let session = require_authenticated_session(&state, &headers).await?;
    let session_actor = find_local_actor_by_id(&state, session.actor_id).await?;
    if session_actor.profile.username != path.username {
        return Err(ApiError::Forbidden("cannot read another user's timeline"));
    }

    let limit = query.limit.unwrap_or(20).clamp(1, 100);
    let page = state
        .db
        .posts()
        .list_home_timeline(session_actor.id(), query.before, limit)
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
    let session = require_authenticated_session(&state, &headers).await?;
    let session_actor = find_local_actor_by_id(&state, session.actor_id).await?;
    if session_actor.profile.username != path.username {
        return Err(ApiError::Forbidden("cannot post as another user"));
    }
    ensure_same_origin(&headers, &state.public_base_url, &state.allowed_origins)?;
    state
        .rate_limiter
        .check(
            format!(
                "create-post:{}:{}",
                client_rate_key(&headers),
                path.username
            ),
            RateLimitSpec {
                limit: 60,
                window: Duration::from_secs(60),
            },
        )
        .await?;
    ensure_csrf_token(&headers, &session.csrf_token)?;

    let actor = find_local_actor_by_username(&state, &path.username).await?;

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

async fn post_follow(
    State((state, _config)): State<(Arc<AppState>, ServerConfig)>,
    Path(path): Path<UsernamePath>,
    headers: HeaderMap,
    Json(request): Json<FollowRequest>,
) -> Result<(StatusCode, Json<FollowResponse>), ApiError> {
    let local_actor = require_actor_for_mutation(&state, &headers, &path.username).await?;
    state
        .rate_limiter
        .check(
            format!(
                "follow:{}:{}",
                client_rate_key(&headers),
                local_actor.profile.username
            ),
            RateLimitSpec {
                limit: 30,
                window: Duration::from_secs(60),
            },
        )
        .await?;

    let discovery = discover_remote_actor(state.remote_client(), &request.resource).await?;
    state.db.remote_actors().upsert(&discovery.actor).await?;
    let job = kodamapub_job::enqueue_follow_delivery(
        &state.db,
        &local_actor,
        &discovery.actor,
        &RetryPolicy::default(),
    )
    .await?;

    Ok((
        StatusCode::ACCEPTED,
        Json(FollowResponse {
            remote_actor: discovery.actor.profile,
            state: kodamapub_domain::FollowState::Pending,
            job_id: job.id.0.to_string(),
        }),
    ))
}

async fn delete_follow(
    State((state, _config)): State<(Arc<AppState>, ServerConfig)>,
    Path(path): Path<UsernamePath>,
    headers: HeaderMap,
    Json(request): Json<FollowRequest>,
) -> Result<(StatusCode, Json<FollowResponse>), ApiError> {
    let local_actor = require_actor_for_mutation(&state, &headers, &path.username).await?;
    state
        .rate_limiter
        .check(
            format!(
                "unfollow:{}:{}",
                client_rate_key(&headers),
                local_actor.profile.username
            ),
            RateLimitSpec {
                limit: 30,
                window: Duration::from_secs(60),
            },
        )
        .await?;

    let discovery = discover_remote_actor(state.remote_client(), &request.resource).await?;
    state.db.remote_actors().upsert(&discovery.actor).await?;
    let job = kodamapub_job::enqueue_unfollow_delivery(
        &state.db,
        &local_actor,
        &discovery.actor,
        &RetryPolicy::default(),
    )
    .await?;

    Ok((
        StatusCode::ACCEPTED,
        Json(FollowResponse {
            remote_actor: discovery.actor.profile,
            state: kodamapub_domain::FollowState::Rejected,
            job_id: job.id.0.to_string(),
        }),
    ))
}

async fn get_actor(
    State((state, _config)): State<(Arc<AppState>, ServerConfig)>,
    Path(path): Path<UsernamePath>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let actor = find_local_actor_by_username(&state, &path.username).await?;

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
    let actor = find_local_actor_by_username(&state, &path.username).await?;
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
    headers: HeaderMap,
    Query(query): Query<WebFingerQuery>,
) -> Result<Response, ApiError> {
    state
        .rate_limiter
        .check(
            format!("webfinger:{}", client_rate_key(&headers)),
            RateLimitSpec {
                limit: 60,
                window: Duration::from_secs(60),
            },
        )
        .await?;
    let parsed = parse_webfinger_resource(&config.public_base_url, &query.resource)
        .ok_or_else(|| ApiError::BadRequest("invalid webfinger resource".to_string()))?;
    let actor = find_local_actor_by_username(&state, &parsed).await?;

    jrd_response(&webfinger_response(query.resource, actor.profile.actor_url))
}

async fn post_login(
    State((state, _config)): State<(Arc<AppState>, ServerConfig)>,
    headers: HeaderMap,
    Json(request): Json<LoginRequest>,
) -> Result<Response, ApiError> {
    ensure_same_origin(&headers, &state.public_base_url, &state.allowed_origins)?;
    state
        .rate_limiter
        .check(
            format!("login:{}:{}", client_rate_key(&headers), request.username),
            RateLimitSpec {
                limit: 10,
                window: Duration::from_secs(300),
            },
        )
        .await?;

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
    let csrf_token = Uuid::now_v7().to_string();
    let expires_at = Utc::now() + ChronoDuration::days(30);
    state
        .db
        .sessions()
        .create(&token, actor.id(), &csrf_token, expires_at)
        .await?;

    let secure = state.public_base_url.as_str().starts_with("https://");
    session_response(
        canonical_local_actor_profile(&actor, &state.public_base_url)?,
        csrf_token,
        Some(build_session_cookie(&token, expires_at, secure)?),
    )
}

async fn get_session(
    State((state, _config)): State<(Arc<AppState>, ServerConfig)>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let session = require_authenticated_session(&state, &headers).await?;
    let actor = find_local_actor_by_id(&state, session.actor_id).await?;
    session_response(
        canonical_local_actor_profile(&actor, &state.public_base_url)?,
        session.csrf_token,
        None,
    )
}

async fn post_logout(
    State((state, _config)): State<(Arc<AppState>, ServerConfig)>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    if let Some(token) = session_token_from_headers(&headers) {
        if let Some(session) = state.db.sessions().find(&token).await? {
            ensure_same_origin(&headers, &state.public_base_url, &state.allowed_origins)?;
            ensure_csrf_token(&headers, &session.csrf_token)?;
            state.db.sessions().delete(&token).await?;
        }
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

async fn require_authenticated_session(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<kodamapub_db::SessionRecord, ApiError> {
    let token =
        session_token_from_headers(headers).ok_or(ApiError::Unauthorized("session is required"))?;
    let Some(session) = state.db.sessions().find(&token).await? else {
        return Err(ApiError::Unauthorized("session is required"));
    };

    Ok(session)
}

fn session_token_from_headers(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::COOKIE)
        .and_then(|value| value.to_str().ok())
        .and_then(|cookies| {
            cookies
                .split(';')
                .map(str::trim)
                .find_map(|cookie| cookie.strip_prefix(&format!("{SESSION_COOKIE_NAME}=")))
                .map(str::to_string)
        })
}

fn session_response(
    actor: kodamapub_domain::ActorProfile,
    csrf_token: String,
    cookie: Option<String>,
) -> Result<Response, ApiError> {
    let mut response = Json(SessionResponse { actor, csrf_token }).into_response();
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
    let max_age = expires_at
        .signed_duration_since(Utc::now())
        .num_seconds()
        .max(0);
    let secure_flag = if secure { "; Secure" } else { "" };
    Ok(format!(
        "{SESSION_COOKIE_NAME}={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={max_age}{secure_flag}"
    ))
}

fn ensure_csrf_token(headers: &HeaderMap, expected: &str) -> Result<(), ApiError> {
    let provided = headers
        .get("x-csrf-token")
        .and_then(|value| value.to_str().ok())
        .ok_or(ApiError::Forbidden("csrf token is required"))?;

    if provided != expected {
        return Err(ApiError::Forbidden("csrf token is invalid"));
    }

    Ok(())
}

async fn require_actor_for_mutation(
    state: &AppState,
    headers: &HeaderMap,
    username: &Username,
) -> Result<kodamapub_domain::LocalActor, ApiError> {
    let session = require_authenticated_session(state, headers).await?;
    let session_actor = find_local_actor_by_id(state, session.actor_id).await?;
    if session_actor.profile.username != *username {
        return Err(ApiError::Forbidden("cannot mutate another user"));
    }
    ensure_same_origin(headers, &state.public_base_url, &state.allowed_origins)?;
    Ok(session_actor)
}

fn ensure_same_origin(
    headers: &HeaderMap,
    public_base_url: &PublicBaseUrl,
    allowed_origins: &[String],
) -> Result<(), ApiError> {
    let Some(origin) = headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
    else {
        return Err(ApiError::Forbidden("origin is required"));
    };

    if !same_origin(origin, public_base_url.as_str())
        && !allowed_origins
            .iter()
            .any(|allowed| same_origin(origin, allowed))
    {
        return Err(ApiError::Forbidden("origin is not allowed"));
    }

    Ok(())
}

fn same_origin(candidate: &str, expected: &str) -> bool {
    let Ok(candidate) = url::Url::parse(candidate) else {
        return false;
    };
    let Ok(expected) = url::Url::parse(expected) else {
        return false;
    };

    candidate.scheme() == expected.scheme()
        && candidate.host_str() == expected.host_str()
        && candidate.port_or_known_default() == expected.port_or_known_default()
}

fn parse_allowed_origins(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|origin| !origin.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn client_rate_key(headers: &HeaderMap) -> String {
    headers
        .get("x-forwarded-for")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(',').next())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("unknown")
        .to_string()
}

fn clear_session_cookie(secure: bool) -> String {
    let secure_flag = if secure { "; Secure" } else { "" };
    format!("{SESSION_COOKIE_NAME}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0{secure_flag}")
}

async fn post_inbox(
    State((state, _config)): State<(Arc<AppState>, ServerConfig)>,
    Path(path): Path<UsernamePath>,
    headers: HeaderMap,
    method: Method,
    uri: Uri,
    body: Bytes,
) -> Result<StatusCode, ApiError> {
    state
        .rate_limiter
        .check(
            format!("inbox:{}:{}", client_rate_key(&headers), path.username),
            RateLimitSpec {
                limit: 120,
                window: Duration::from_secs(60),
            },
        )
        .await?;

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

    let signature_target = headers
        .get("x-original-uri")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_else(|| {
            uri.path_and_query()
                .map(|value| value.as_str())
                .unwrap_or_else(|| uri.path())
        });
    eprintln!(
        "verifying inbound activity signature original_host={} forwarded_host={} host={} original_uri={} request_uri={}",
        headers
            .get("x-original-host")
            .and_then(|value| value.to_str().ok())
            .unwrap_or("-"),
        headers
            .get("x-forwarded-host")
            .and_then(|value| value.to_str().ok())
            .unwrap_or("-"),
        headers
            .get("Host")
            .and_then(|value| value.to_str().ok())
            .unwrap_or("-"),
        headers
            .get("x-original-uri")
            .and_then(|value| value.to_str().ok())
            .unwrap_or("-"),
        signature_target
    );
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
        IncomingActivity::Accept(accept) => {
            handle_accept_activity(&state, &local_actor, &remote_actor, accept).await?;
        }
        IncomingActivity::Undo(_) | IncomingActivity::Delete(_) => {}
    }

    Ok(StatusCode::ACCEPTED)
}

async fn find_local_actor_by_id(
    state: &AppState,
    actor_id: kodamapub_domain::ActorId,
) -> Result<kodamapub_domain::LocalActor, ApiError> {
    let actor = state
        .db
        .local_actors()
        .find_by_id(actor_id)
        .await?
        .ok_or(ApiError::NotFound("local actor not found"))?;
    canonicalize_local_actor(&actor, &state.public_base_url)
}

async fn find_local_actor_by_username(
    state: &AppState,
    username: &Username,
) -> Result<kodamapub_domain::LocalActor, ApiError> {
    let actor = state
        .db
        .local_actors()
        .find_by_username(username)
        .await?
        .ok_or(ApiError::NotFound("local actor not found"))?;
    canonicalize_local_actor(&actor, &state.public_base_url)
}

fn canonical_local_actor_profile(
    actor: &LocalActor,
    public_base_url: &PublicBaseUrl,
) -> Result<ActorProfile, ApiError> {
    Ok(canonicalize_local_actor(actor, public_base_url)?.profile)
}

fn canonicalize_local_actor(
    actor: &LocalActor,
    public_base_url: &PublicBaseUrl,
) -> Result<LocalActor, ApiError> {
    let base = public_base_url.as_str().trim_end_matches('/');
    let actor_url = Url::parse(&format!("{base}/users/{}", actor.profile.username))
        .map_err(|error| ApiError::Internal(anyhow::Error::new(error)))?;
    let inbox_url = Url::parse(&format!("{base}/users/{}/inbox", actor.profile.username))
        .map_err(|error| ApiError::Internal(anyhow::Error::new(error)))?;
    let outbox_url = Url::parse(&format!("{base}/users/{}/outbox", actor.profile.username))
        .map_err(|error| ApiError::Internal(anyhow::Error::new(error)))?;

    Ok(LocalActor {
        profile: ActorProfile {
            id: actor.profile.id,
            username: actor.profile.username.clone(),
            display_name: actor.profile.display_name.clone(),
            summary: actor.profile.summary.clone(),
            actor_url,
            inbox_url: Some(inbox_url),
            outbox_url: Some(outbox_url),
        },
        public_key_pem: actor.public_key_pem.clone(),
        private_key_pem: actor.private_key_pem.clone(),
    })
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

fn build_remote_client() -> reqwest::Client {
    let mut builder = reqwest::Client::builder();

    if let Ok(ca_cert_path) = std::env::var("KODAMAPUB_REMOTE_CA_CERT_PATH") {
        if let Ok(ca_pem) = std::fs::read(&ca_cert_path) {
            if let Ok(ca_cert) = reqwest::Certificate::from_pem(&ca_pem) {
                builder = builder.add_root_certificate(ca_cert);
            }
        }
    }

    builder.build().unwrap_or_else(|_| reqwest::Client::new())
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
    let allowed_origins = std::env::var("KODAMAPUB_ALLOWED_ORIGINS")
        .map(|value| parse_allowed_origins(&value))
        .unwrap_or_default();
    let bind_addr = std::env::var("BIND_ADDR").unwrap_or_else(|_| "127.0.0.1:3000".to_string());

    let db = Database::connect(&database_url).await?;
    db.migrate().await?;

    let state = Arc::new(AppState {
        db,
        public_base_url,
        allowed_origins,
        remote_client: build_remote_client(),
        rate_limiter: RateLimiter::new(),
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

    state
        .db
        .follows()
        .upsert(&FollowRelation::new(local_actor.id(), remote_actor))
        .await?;

    let job = kodamapub_job::enqueue_accept_delivery(
        &state.db,
        local_actor,
        remote_actor,
        &follow.id,
        &RetryPolicy::default(),
    )
    .await?;
    tracing::info!(
        job_id = %job.id.0,
        follow_id = %follow.id,
        local_actor = %local_actor.profile.actor_url,
        remote_actor = %remote_actor.profile.actor_url,
        "queued accept delivery for inbound follow"
    );
    Ok(())
}

async fn handle_accept_activity(
    state: &AppState,
    local_actor: &kodamapub_domain::LocalActor,
    remote_actor: &kodamapub_domain::RemoteActor,
    accept: kodamapub_activitypub::IncomingAcceptActivity,
) -> Result<(), ApiError> {
    tracing::info!(
        local_actor = %local_actor.profile.actor_url,
        remote_actor = %remote_actor.profile.actor_url,
        accept_id = %accept.id,
        object = %accept.object,
        "received inbound accept activity"
    );

    activate_follow_and_backfill(&state.db, local_actor, remote_actor).await?;
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
        sample_local_actor_at("https://example.invalid")
    }

    fn sample_local_actor_at(base: &str) -> LocalActor {
        LocalActor {
            profile: ActorProfile::new(
                "alice".parse().expect("username"),
                "Alice".parse().expect("display name"),
                Some("summary".parse().expect("summary")),
                Url::parse(&format!("{base}/users/alice")).expect("actor url"),
                Some(Url::parse(&format!("{base}/users/alice/inbox")).expect("inbox url")),
                Some(Url::parse(&format!("{base}/users/alice/outbox")).expect("outbox url")),
            ),
            public_key_pem: "PUBLIC KEY".to_string(),
            private_key_pem: "PRIVATE KEY".to_string(),
        }
    }

    async fn test_app() -> Router {
        test_app_with_actor(sample_local_actor()).await
    }

    async fn test_app_with_actor(actor: LocalActor) -> Router {
        let db = test_db().await;
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
            allowed_origins: Vec::new(),
            remote_client: build_remote_client(),
            rate_limiter: RateLimiter::new(),
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

    #[tokio::test]
    async fn webfinger_normalizes_stale_local_actor_urls() {
        let app = test_app_with_actor(sample_local_actor_at("http://127.0.0.1:3000")).await;

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
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let json: serde_json::Value = serde_json::from_slice(&body).expect("json");
        assert_eq!(
            json["links"][0]["href"],
            "https://example.invalid/users/alice"
        );
    }
}
