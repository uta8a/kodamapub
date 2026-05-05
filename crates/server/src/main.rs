use std::{net::SocketAddr, sync::Arc};

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
};
use kodamapub_db::{Database, DbError};
use kodamapub_domain::{
    ContentFormat, ContentSource, DomainError, NewPost, Post, PostId, PublicBaseUrl, Username,
    Visibility,
};
use serde::{Deserialize, Serialize};

#[derive(Clone)]
struct AppState {
    db: Database,
    public_base_url: PublicBaseUrl,
}

#[derive(Debug, Deserialize)]
struct CreatePostRequest {
    content_source: ContentSource,
    content_format: ContentFormat,
    visibility: Visibility,
    in_reply_to: Option<PostId>,
}

#[derive(Debug, Deserialize)]
struct ListPostsQuery {
    limit: Option<i64>,
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

#[derive(Debug)]
enum ApiError {
    NotFound(&'static str),
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

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        match self {
            ApiError::NotFound(message) => (StatusCode::NOT_FOUND, message).into_response(),
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

async fn get_user(
    State(state): State<Arc<AppState>>,
    Path(path): Path<UsernamePath>,
) -> Result<Json<kodamapub_domain::ActorProfile>, ApiError> {
    let actor = state
        .db
        .local_actors()
        .find_by_username(&path.username)
        .await?
        .ok_or(ApiError::NotFound("local actor not found"))?;

    Ok(Json(actor.profile))
}

async fn list_user_posts(
    State(state): State<Arc<AppState>>,
    Path(path): Path<UsernamePath>,
    Query(query): Query<ListPostsQuery>,
) -> Result<Json<Vec<Post>>, ApiError> {
    let actor = state
        .db
        .local_actors()
        .find_by_username(&path.username)
        .await?
        .ok_or(ApiError::NotFound("local actor not found"))?;
    let limit = query.limit.unwrap_or(20).clamp(1, 100);
    let posts = state.db.posts().list_by_actor(actor.id(), limit).await?;
    Ok(Json(posts))
}

async fn create_user_post(
    State(state): State<Arc<AppState>>,
    Path(path): Path<UsernamePath>,
    Json(request): Json<CreatePostRequest>,
) -> Result<(StatusCode, Json<Post>), ApiError> {
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
    Ok((StatusCode::CREATED, Json(post)))
}

async fn get_post(
    State(state): State<Arc<AppState>>,
    Path(path): Path<PostIdPath>,
) -> Result<Json<Post>, ApiError> {
    let post = state
        .db
        .posts()
        .find(path.post_id)
        .await?
        .ok_or(ApiError::NotFound("post not found"))?;

    Ok(Json(post))
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
    });

    let app = Router::new()
        .route("/health", get(health))
        .route("/posts/{post_id}", get(get_post))
        .route("/users/{username}", get(get_user))
        .route(
            "/users/{username}/posts",
            get(list_user_posts).post(create_user_post),
        )
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(bind_addr.parse::<SocketAddr>()?).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
