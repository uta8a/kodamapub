use std::time::Duration;

use chrono::{Duration as ChronoDuration, Utc};
use kodamapub_activitypub::{
    ActivityPubError, accept_activity, activity_kind_for_payload, deliver_signed_activity,
    follow_activity, serialize_activity,
};
use kodamapub_db::Database;
use kodamapub_domain::{
    DeliveryJob, DeliveryKind, FollowRelation, FollowState, LocalActor, Post, RemoteActor,
};
use reqwest::Client;
use thiserror::Error;

#[derive(Debug, Clone)]
pub struct RetryPolicy {
    pub max_attempts: u32,
    pub backoff: Duration,
}

#[derive(Debug, Clone, Default)]
pub struct DeliveryRunSummary {
    pub delivered: u32,
    pub rescheduled: u32,
    pub failed: u32,
}

#[derive(Debug, Error)]
pub enum JobError {
    #[error(transparent)]
    Db(#[from] kodamapub_db::DbError),
    #[error(transparent)]
    ActivityPub(#[from] ActivityPubError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("local actor not found")]
    MissingLocalActor,
    #[error("remote actor missing inbox url")]
    MissingInboxUrl,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 10,
            backoff: Duration::from_secs(30),
        }
    }
}

fn build_remote_client() -> Client {
    let mut builder = Client::builder();

    if let Ok(ca_cert_path) = std::env::var("KODAMAPUB_REMOTE_CA_CERT_PATH") {
        if let Ok(ca_pem) = std::fs::read(&ca_cert_path) {
            if let Ok(ca_cert) = reqwest::Certificate::from_pem(&ca_pem) {
                builder = builder.add_root_certificate(ca_cert);
            }
        }
    }

    builder.build().unwrap_or_else(|_| Client::new())
}

pub async fn enqueue_follow_delivery(
    db: &Database,
    local_actor: &LocalActor,
    remote_actor: &RemoteActor,
    retry_policy: &RetryPolicy,
) -> Result<DeliveryJob, JobError> {
    let follow = FollowRelation::new(local_actor.id(), remote_actor);
    db.follows().upsert(&follow).await?;

    let inbox_url = remote_actor
        .profile
        .inbox_url
        .clone()
        .ok_or(JobError::MissingInboxUrl)?;
    let payload = serialize_activity(&follow_activity(local_actor, remote_actor))?;
    let job = DeliveryJob::new(
        local_actor.id(),
        inbox_url,
        DeliveryKind::Follow,
        payload,
        retry_policy.max_attempts,
    );

    db.delivery_jobs().create(&job).await?;
    Ok(job)
}

pub async fn enqueue_accept_delivery(
    db: &Database,
    local_actor: &LocalActor,
    remote_actor: &RemoteActor,
    follow_activity_id: &url::Url,
    retry_policy: &RetryPolicy,
) -> Result<DeliveryJob, JobError> {
    let follow = FollowRelation::new(local_actor.id(), remote_actor);
    db.follows().upsert(&follow).await?;

    let inbox_url = remote_actor
        .profile
        .inbox_url
        .clone()
        .ok_or(JobError::MissingInboxUrl)?;
    let payload = serialize_activity(&accept_activity(
        local_actor,
        remote_actor,
        follow_activity_id,
    ))?;
    let job = DeliveryJob::new(
        local_actor.id(),
        inbox_url,
        DeliveryKind::Accept,
        payload,
        retry_policy.max_attempts,
    );

    db.delivery_jobs().create(&job).await?;
    Ok(job)
}

pub async fn enqueue_existing_create_deliveries(
    db: &Database,
    local_actor: &LocalActor,
    remote_actor: &RemoteActor,
    retry_policy: &RetryPolicy,
) -> Result<u32, JobError> {
    let mut created = 0u32;
    let mut before = None;

    loop {
        let page = db.posts().list_public_by_actor(local_actor.id(), before, 100).await?;
        for post in &page.posts {
            let Some(inbox_url) = remote_actor.profile.inbox_url.clone() else {
                return Err(JobError::MissingInboxUrl);
            };
            let payload = serialize_activity(&kodamapub_activitypub::post_to_create_activity(
                post,
                &local_actor.profile,
            ))?;
            let job = DeliveryJob::new(
                local_actor.id(),
                inbox_url,
                DeliveryKind::Create,
                payload,
                retry_policy.max_attempts,
            );
            db.delivery_jobs().create(&job).await?;
            created += 1;
        }

        if let Some(next_before) = page.next_cursor {
            before = Some(next_before);
        } else {
            break;
        }
    }

    Ok(created)
}

pub async fn enqueue_create_deliveries(
    db: &Database,
    local_actor: &LocalActor,
    post: &Post,
    retry_policy: &RetryPolicy,
) -> Result<u32, JobError> {
    let active_remote_actors = db
        .follows()
        .list_active_remote_actors(local_actor.id())
        .await?;
    let payload = serialize_activity(&kodamapub_activitypub::post_to_create_activity(
        post,
        &local_actor.profile,
    ))?;

    let mut created = 0u32;
    for remote_actor in active_remote_actors {
        let Some(inbox_url) = remote_actor.profile.inbox_url.clone() else {
            continue;
        };

        let job = DeliveryJob::new(
            local_actor.id(),
            inbox_url,
            DeliveryKind::Create,
            payload.clone(),
            retry_policy.max_attempts,
        );
        db.delivery_jobs().create(&job).await?;
        created += 1;
    }

    Ok(created)
}

pub async fn retry_failed_deliveries(db: &Database) -> Result<u64, JobError> {
    Ok(db.delivery_jobs().reset_failed().await?)
}

pub async fn process_due_jobs(
    db: &Database,
    retry_policy: &RetryPolicy,
    limit: i64,
) -> Result<DeliveryRunSummary, JobError> {
    let client = build_remote_client();
    process_due_jobs_with_client(db, &client, retry_policy, limit).await
}

pub async fn process_due_jobs_with_client(
    db: &Database,
    client: &Client,
    retry_policy: &RetryPolicy,
    limit: i64,
) -> Result<DeliveryRunSummary, JobError> {
    let jobs = db.delivery_jobs().list_due(Utc::now(), limit).await?;
    let mut summary = DeliveryRunSummary::default();

    for job in jobs {
        db.delivery_jobs().mark_processing(job.id).await?;
        let local_actor = db
            .local_actors()
            .find_by_id(job.local_actor_id)
            .await?
            .ok_or(JobError::MissingLocalActor)?;

        match deliver_signed_activity(client, &local_actor, &job.target_inbox_url, &job.payload)
            .await
        {
            Ok(()) => {
                db.delivery_jobs().mark_delivered(job.id).await?;
                maybe_activate_follow_delivery(db, &job, &local_actor).await?;
                summary.delivered += 1;
            }
            Err(error) => {
                let attempts = job.attempts + 1;
                if attempts >= job.max_attempts {
                    db.delivery_jobs()
                        .mark_failed(job.id, attempts, error.to_string())
                        .await?;
                    summary.failed += 1;
                } else {
                    let backoff = ChronoDuration::from_std(retry_policy.backoff)
                        .unwrap_or_else(|_| ChronoDuration::seconds(30));
                    db.delivery_jobs()
                        .reschedule(job.id, attempts, Utc::now() + backoff, error.to_string())
                        .await?;
                    summary.rescheduled += 1;
                }
            }
        }
    }

    Ok(summary)
}

async fn maybe_activate_follow_delivery(
    db: &Database,
    job: &DeliveryJob,
    local_actor: &LocalActor,
) -> Result<(), JobError> {
    if job.kind != DeliveryKind::Follow {
        return Ok(());
    }

    let activity_type = activity_kind_for_payload(&job.payload)?;
    if activity_type != DeliveryKind::Follow {
        return Ok(());
    }

    let json: serde_json::Value = serde_json::from_str(&job.payload)?;
    let Some(remote_actor_url) = json.get("object").and_then(|value| value.as_str()) else {
        return Ok(());
    };
    let remote_actor_url = remote_actor_url
        .parse::<url::Url>()
        .map_err(|error| ActivityPubError::InvalidResource(error.to_string()))?;
    let Some(remote_actor) = db
        .remote_actors()
        .find_by_actor_url(&remote_actor_url)
        .await?
    else {
        return Ok(());
    };

    if let Some(existing_follow) = db
        .follows()
        .find(local_actor.id(), remote_actor.id())
        .await?
    {
        if existing_follow.state == FollowState::Active {
            return Ok(());
        }
    }

    activate_follow_and_backfill(db, local_actor, &remote_actor).await?;
    Ok(())
}

async fn activate_follow_and_backfill(
    db: &Database,
    local_actor: &LocalActor,
    remote_actor: &RemoteActor,
) -> Result<(), JobError> {
    if let Some(existing_follow) = db
        .follows()
        .find(local_actor.id(), remote_actor.id())
        .await?
    {
        if existing_follow.state == FollowState::Active {
            return Ok(());
        }
    }

    db.follows()
        .set_state(local_actor.id(), remote_actor.id(), FollowState::Active)
        .await?;

    enqueue_existing_create_deliveries(db, local_actor, remote_actor, &RetryPolicy::default())
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Router, response::IntoResponse, routing::post};
    use kodamapub_activitypub::generate_local_actor_keypair_pem;
    use kodamapub_domain::{ActorProfile, ContentFormat, NewPost, Visibility};
    use std::sync::Arc;
    use tokio::sync::Mutex;
    use url::Url;

    async fn memory_db() -> Database {
        let db = Database::connect("sqlite::memory:")
            .await
            .expect("connect sqlite memory db");
        db.migrate().await.expect("run migrations");
        db
    }

    fn sample_local_actor() -> LocalActor {
        let (public_key_pem, private_key_pem) =
            generate_local_actor_keypair_pem().expect("generate keypair");
        LocalActor {
            profile: ActorProfile::new(
                "alice".parse().expect("username"),
                "Alice".parse().expect("display name"),
                Some("local actor".parse().expect("summary")),
                Url::parse("https://example.invalid/users/alice").expect("actor url"),
                Some(Url::parse("https://example.invalid/users/alice/inbox").expect("inbox url")),
                Some(Url::parse("https://example.invalid/users/alice/outbox").expect("outbox url")),
            ),
            public_key_pem,
            private_key_pem,
        }
    }

    fn sample_remote_actor(inbox_url: &str) -> RemoteActor {
        RemoteActor {
            profile: ActorProfile::new(
                "bob".parse().expect("username"),
                "Bob".parse().expect("display name"),
                Some("remote actor".parse().expect("summary")),
                Url::parse("https://remote.example/users/bob").expect("actor url"),
                Some(inbox_url.parse().expect("inbox url")),
                Some(Url::parse("https://remote.example/users/bob/outbox").expect("outbox url")),
            ),
            public_key_pem: Some("REMOTE PUBLIC KEY".to_string()),
            fetched_at: Utc::now(),
        }
    }

    #[tokio::test]
    async fn enqueue_follow_delivery_creates_follow_and_job() {
        let db = memory_db().await;
        let local_actor = sample_local_actor();
        let remote_actor = sample_remote_actor("https://remote.example/users/bob/inbox");

        db.local_actors()
            .create(&local_actor)
            .await
            .expect("create local actor");
        db.remote_actors()
            .upsert(&remote_actor)
            .await
            .expect("upsert remote actor");

        let job =
            enqueue_follow_delivery(&db, &local_actor, &remote_actor, &RetryPolicy::default())
                .await
                .expect("enqueue follow delivery");

        let follow = db
            .follows()
            .find(local_actor.id(), remote_actor.id())
            .await
            .expect("find follow")
            .expect("follow exists");
        assert_eq!(follow.state, FollowState::Pending);

        let found_job = db
            .delivery_jobs()
            .find(job.id)
            .await
            .expect("find job")
            .expect("job exists");
        assert_eq!(found_job.kind, DeliveryKind::Follow);
    }

    #[tokio::test]
    async fn process_due_jobs_delivers_and_activates_follow() {
        #[derive(Clone, Default)]
        struct InboxState {
            calls: Arc<Mutex<u32>>,
        }

        async fn inbox(
            axum::extract::State(state): axum::extract::State<InboxState>,
        ) -> impl IntoResponse {
            *state.calls.lock().await += 1;
            axum::http::StatusCode::ACCEPTED
        }

        let inbox_state = InboxState::default();
        let app = Router::new()
            .route("/users/bob/inbox", post(inbox))
            .with_state(inbox_state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:38911")
            .await
            .expect("bind test server");
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve");
        });

        let db = memory_db().await;
        let local_actor = sample_local_actor();
        let remote_actor = sample_remote_actor("http://127.0.0.1:38911/users/bob/inbox");

        db.local_actors()
            .create(&local_actor)
            .await
            .expect("create local actor");
        db.remote_actors()
            .upsert(&remote_actor)
            .await
            .expect("upsert remote actor");

        enqueue_follow_delivery(&db, &local_actor, &remote_actor, &RetryPolicy::default())
            .await
            .expect("enqueue follow delivery");

        let summary = process_due_jobs(&db, &RetryPolicy::default(), 10)
            .await
            .expect("process due jobs");
        assert_eq!(summary.delivered, 1);
        assert_eq!(*inbox_state.calls.lock().await, 1);

        let follow = db
            .follows()
            .find(local_actor.id(), remote_actor.id())
            .await
            .expect("find follow")
            .expect("follow exists");
        assert_eq!(follow.state, FollowState::Active);

        server.abort();
    }

    #[tokio::test]
    async fn process_due_jobs_backfills_existing_public_posts_after_follow_accept() {
        #[derive(Clone, Default)]
        struct InboxState {
            calls: Arc<Mutex<u32>>,
        }

        async fn inbox(
            axum::extract::State(state): axum::extract::State<InboxState>,
        ) -> impl IntoResponse {
            *state.calls.lock().await += 1;
            axum::http::StatusCode::ACCEPTED
        }

        let inbox_state = InboxState::default();
        let app = Router::new()
            .route("/users/bob/inbox", post(inbox))
            .with_state(inbox_state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:38912")
            .await
            .expect("bind test server");
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve");
        });

        let db = memory_db().await;
        let local_actor = sample_local_actor();
        let remote_actor = sample_remote_actor("http://127.0.0.1:38912/users/bob/inbox");

        db.local_actors()
            .create(&local_actor)
            .await
            .expect("create local actor");
        db.remote_actors()
            .upsert(&remote_actor)
            .await
            .expect("upsert remote actor");

        for content in ["first public post", "second public post"] {
            let post = Post::new(
                NewPost {
                    actor_id: local_actor.id(),
                    content_source: content.parse().expect("content source"),
                    content_format: ContentFormat::Plaintext,
                    visibility: Visibility::Public,
                    in_reply_to: None,
                },
                &"https://example.invalid".parse().expect("public base url"),
            )
            .expect("post");
            db.posts().create(&post).await.expect("insert post");
        }

        enqueue_follow_delivery(&db, &local_actor, &remote_actor, &RetryPolicy::default())
            .await
            .expect("enqueue follow delivery");

        let first = process_due_jobs(&db, &RetryPolicy::default(), 10)
            .await
            .expect("process follow delivery");
        assert_eq!(first.delivered, 1);
        assert_eq!(*inbox_state.calls.lock().await, 1);

        let second = process_due_jobs(&db, &RetryPolicy::default(), 10)
            .await
            .expect("process backfill deliveries");
        assert_eq!(second.delivered, 2);
        assert_eq!(*inbox_state.calls.lock().await, 3);

        let follow = db
            .follows()
            .find(local_actor.id(), remote_actor.id())
            .await
            .expect("find follow")
            .expect("follow exists");
        assert_eq!(follow.state, FollowState::Active);

        server.abort();
    }

    #[tokio::test]
    async fn enqueue_create_deliveries_targets_active_follows() {
        let db = memory_db().await;
        let local_actor = sample_local_actor();
        let remote_actor = sample_remote_actor("https://remote.example/users/bob/inbox");

        db.local_actors()
            .create(&local_actor)
            .await
            .expect("create local actor");
        db.remote_actors()
            .upsert(&remote_actor)
            .await
            .expect("upsert remote actor");

        let mut follow = FollowRelation::new(local_actor.id(), &remote_actor);
        follow.state = FollowState::Active;
        db.follows().upsert(&follow).await.expect("upsert follow");

        let post = Post::new(
            NewPost {
                actor_id: local_actor.id(),
                content_source: "hello federation".parse().expect("content source"),
                content_format: ContentFormat::Plaintext,
                visibility: Visibility::Public,
                in_reply_to: None,
            },
            &"https://example.invalid".parse().expect("public base url"),
        )
        .expect("post");

        let count = enqueue_create_deliveries(&db, &local_actor, &post, &RetryPolicy::default())
            .await
            .expect("enqueue create deliveries");
        assert_eq!(count, 1);

        let jobs = db
            .delivery_jobs()
            .list_due(Utc::now(), 10)
            .await
            .expect("list due jobs");
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].kind, DeliveryKind::Create);
    }
}
