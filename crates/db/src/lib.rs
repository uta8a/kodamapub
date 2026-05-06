use argon2::{
    Argon2,
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString, rand_core::OsRng},
};
use chrono::{DateTime, Utc};
use kodamapub_domain::{
    ActorId, ActorProfile, ContentFormat, DeliveryJob, DeliveryJobId, DeliveryKind, DeliveryState,
    FollowRelation, FollowState, LocalActor, Post, PostId, RemoteActor, Summary, TextValueError,
    Username, UsernameError, Visibility,
};
use sqlx::{Row, SqlitePool, migrate::Migrator};
use thiserror::Error;
use url::Url;
use uuid::Uuid;

pub static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

#[derive(Clone)]
pub struct Database {
    pool: SqlitePool,
}

impl Database {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn connect(database_url: &str) -> Result<Self, DbError> {
        let pool = SqlitePool::connect(database_url).await?;
        Ok(Self::new(pool))
    }

    pub async fn migrate(&self) -> Result<(), DbError> {
        MIGRATOR.run(&self.pool).await?;
        Ok(())
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    pub fn local_actors(&self) -> LocalActorRepository<'_> {
        LocalActorRepository { pool: &self.pool }
    }

    pub fn remote_actors(&self) -> RemoteActorRepository<'_> {
        RemoteActorRepository { pool: &self.pool }
    }

    pub fn follows(&self) -> FollowRepository<'_> {
        FollowRepository { pool: &self.pool }
    }

    pub fn delivery_jobs(&self) -> DeliveryJobRepository<'_> {
        DeliveryJobRepository { pool: &self.pool }
    }

    pub fn inbox_dedup(&self) -> InboxDedupRepository<'_> {
        InboxDedupRepository { pool: &self.pool }
    }

    pub fn local_actor_credentials(&self) -> LocalActorCredentialRepository<'_> {
        LocalActorCredentialRepository { pool: &self.pool }
    }

    pub fn sessions(&self) -> SessionRepository<'_> {
        SessionRepository { pool: &self.pool }
    }

    pub fn posts(&self) -> PostRepository<'_> {
        PostRepository { pool: &self.pool }
    }
}

pub struct LocalActorRepository<'a> {
    pool: &'a SqlitePool,
}

impl<'a> LocalActorRepository<'a> {
    pub async fn create(&self, actor: &LocalActor) -> Result<(), DbError> {
        let mut tx = self.pool.begin().await?;

        sqlx::query(
            r#"
            insert into actors (
                id, username, display_name, summary, actor_url, inbox_url, outbox_url, created_at
            ) values ($1, $2, $3, $4, $5, $6, $7, current_timestamp)
            "#,
        )
        .bind(actor.id().0)
        .bind(actor.profile.username.as_str())
        .bind(actor.profile.display_name.as_str())
        .bind(actor.profile.summary.as_ref().map(Summary::as_str))
        .bind(actor.profile.actor_url.as_str())
        .bind(opt_url_str(&actor.profile.inbox_url))
        .bind(opt_url_str(&actor.profile.outbox_url))
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            r#"
            insert into local_actor_secrets (
                actor_id, public_key_pem, private_key_pem
            ) values ($1, $2, $3)
            "#,
        )
        .bind(actor.id().0)
        .bind(&actor.public_key_pem)
        .bind(&actor.private_key_pem)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(())
    }

    pub async fn create_with_password(
        &self,
        actor: &LocalActor,
        password_hash: &str,
    ) -> Result<(), DbError> {
        let mut tx = self.pool.begin().await?;

        sqlx::query(
            r#"
            insert into actors (
                id, username, display_name, summary, actor_url, inbox_url, outbox_url, created_at
            ) values ($1, $2, $3, $4, $5, $6, $7, current_timestamp)
            "#,
        )
        .bind(actor.id().0)
        .bind(actor.profile.username.as_str())
        .bind(actor.profile.display_name.as_str())
        .bind(actor.profile.summary.as_ref().map(Summary::as_str))
        .bind(actor.profile.actor_url.as_str())
        .bind(opt_url_str(&actor.profile.inbox_url))
        .bind(opt_url_str(&actor.profile.outbox_url))
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            r#"
            insert into local_actor_secrets (
                actor_id, public_key_pem, private_key_pem
            ) values ($1, $2, $3)
            "#,
        )
        .bind(actor.id().0)
        .bind(&actor.public_key_pem)
        .bind(&actor.private_key_pem)
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            r#"
            insert into local_actor_credentials (
                actor_id, password_hash
            ) values ($1, $2)
            "#,
        )
        .bind(actor.id().0)
        .bind(password_hash)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(())
    }

    pub async fn find_by_username(
        &self,
        username: &Username,
    ) -> Result<Option<LocalActor>, DbError> {
        let row = sqlx::query(
            r#"
            select
                a.id,
                a.username,
                a.display_name,
                a.summary,
                a.actor_url,
                a.inbox_url,
                a.outbox_url,
                s.public_key_pem,
                s.private_key_pem
            from actors a
            join local_actor_secrets s on s.actor_id = a.id
            where a.username = $1
            "#,
        )
        .bind(username.as_str())
        .fetch_optional(self.pool)
        .await?;

        row.map(local_actor_from_row).transpose()
    }

    pub async fn find_by_id(&self, actor_id: ActorId) -> Result<Option<LocalActor>, DbError> {
        let row = sqlx::query(
            r#"
            select
                a.id,
                a.username,
                a.display_name,
                a.summary,
                a.actor_url,
                a.inbox_url,
                a.outbox_url,
                s.public_key_pem,
                s.private_key_pem
            from actors a
            join local_actor_secrets s on s.actor_id = a.id
            where a.id = $1
            "#,
        )
        .bind(actor_id.0)
        .fetch_optional(self.pool)
        .await?;

        row.map(local_actor_from_row).transpose()
    }
}

pub struct RemoteActorRepository<'a> {
    pool: &'a SqlitePool,
}

pub struct LocalActorCredentialRepository<'a> {
    pool: &'a SqlitePool,
}

pub struct SessionRepository<'a> {
    pool: &'a SqlitePool,
}

#[derive(Debug, Clone)]
pub struct SessionRecord {
    pub actor_id: ActorId,
    pub csrf_token: String,
}

impl<'a> RemoteActorRepository<'a> {
    pub async fn upsert(&self, actor: &RemoteActor) -> Result<(), DbError> {
        let mut tx = self.pool.begin().await?;

        sqlx::query(
            r#"
            insert into actors (
                id, username, display_name, summary, actor_url, inbox_url, outbox_url, created_at
            ) values ($1, $2, $3, $4, $5, $6, $7, current_timestamp)
            on conflict (actor_url) do update set
                username = excluded.username,
                display_name = excluded.display_name,
                summary = excluded.summary,
                inbox_url = excluded.inbox_url,
                outbox_url = excluded.outbox_url
            "#,
        )
        .bind(actor.id().0)
        .bind(actor.profile.username.as_str())
        .bind(actor.profile.display_name.as_str())
        .bind(actor.profile.summary.as_ref().map(Summary::as_str))
        .bind(actor.profile.actor_url.as_str())
        .bind(opt_url_str(&actor.profile.inbox_url))
        .bind(opt_url_str(&actor.profile.outbox_url))
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            r#"
            insert into remote_actor_state (actor_id, public_key_pem, fetched_at)
            values ($1, $2, $3)
            on conflict (actor_id) do update set
                public_key_pem = excluded.public_key_pem,
                fetched_at = excluded.fetched_at
            "#,
        )
        .bind(actor.id().0)
        .bind(&actor.public_key_pem)
        .bind(actor.fetched_at)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(())
    }

    pub async fn find_by_actor_url(&self, actor_url: &Url) -> Result<Option<RemoteActor>, DbError> {
        let row = sqlx::query(
            r#"
            select
                a.id,
                a.username,
                a.display_name,
                a.summary,
                a.actor_url,
                a.inbox_url,
                a.outbox_url,
                s.public_key_pem,
                s.fetched_at
            from actors a
            join remote_actor_state s on s.actor_id = a.id
            where a.actor_url = $1
            "#,
        )
        .bind(actor_url.as_str())
        .fetch_optional(self.pool)
        .await?;

        row.map(remote_actor_from_row).transpose()
    }

    pub async fn find_by_inbox_url(&self, inbox_url: &Url) -> Result<Option<RemoteActor>, DbError> {
        let row = sqlx::query(
            r#"
            select
                a.id,
                a.username,
                a.display_name,
                a.summary,
                a.actor_url,
                a.inbox_url,
                a.outbox_url,
                s.public_key_pem,
                s.fetched_at
            from actors a
            join remote_actor_state s on s.actor_id = a.id
            where a.inbox_url = $1
            "#,
        )
        .bind(inbox_url.as_str())
        .fetch_optional(self.pool)
        .await?;

        row.map(remote_actor_from_row).transpose()
    }
}

impl<'a> LocalActorCredentialRepository<'a> {
    pub async fn set_password_hash(
        &self,
        actor_id: ActorId,
        password_hash: &str,
    ) -> Result<(), DbError> {
        sqlx::query(
            r#"
            insert into local_actor_credentials (actor_id, password_hash)
            values ($1, $2)
            on conflict (actor_id) do update set
                password_hash = excluded.password_hash
            "#,
        )
        .bind(actor_id.0)
        .bind(password_hash)
        .execute(self.pool)
        .await?;

        Ok(())
    }

    pub async fn find_password_hash_by_username(
        &self,
        username: &Username,
    ) -> Result<Option<(ActorId, String)>, DbError> {
        let row = sqlx::query(
            r#"
            select a.id, c.password_hash
            from actors a
            join local_actor_credentials c on c.actor_id = a.id
            where a.username = $1
            "#,
        )
        .bind(username.as_str())
        .fetch_optional(self.pool)
        .await?;

        row.map(|row| {
            Ok((
                ActorId(row.try_get::<Uuid, _>("id")?),
                row.try_get::<String, _>("password_hash")?,
            ))
        })
        .transpose()
    }
}

impl<'a> SessionRepository<'a> {
    pub async fn create(
        &self,
        token: &str,
        actor_id: ActorId,
        csrf_token: &str,
        expires_at: DateTime<Utc>,
    ) -> Result<(), DbError> {
        sqlx::query(
            r#"
            insert into login_sessions (token, actor_id, csrf_token, created_at, expires_at)
            values ($1, $2, $3, $4, $5)
            "#,
        )
        .bind(token)
        .bind(actor_id.0)
        .bind(csrf_token)
        .bind(Utc::now())
        .bind(expires_at)
        .execute(self.pool)
        .await?;

        Ok(())
    }

    pub async fn find_actor_id(&self, token: &str) -> Result<Option<ActorId>, DbError> {
        self.find(token)
            .await
            .map(|session| session.map(|value| value.actor_id))
    }

    pub async fn find(&self, token: &str) -> Result<Option<SessionRecord>, DbError> {
        let row = sqlx::query(
            r#"
            select actor_id, csrf_token, expires_at
            from login_sessions
            where token = $1
            "#,
        )
        .bind(token)
        .fetch_optional(self.pool)
        .await?;

        let Some(row) = row else {
            return Ok(None);
        };

        let expires_at: DateTime<Utc> = row.try_get("expires_at")?;
        if expires_at <= Utc::now() {
            self.delete(token).await?;
            return Ok(None);
        }

        let csrf_token = row.try_get::<String, _>("csrf_token")?;
        if csrf_token.is_empty() {
            self.delete(token).await?;
            return Ok(None);
        }

        Ok(Some(SessionRecord {
            actor_id: ActorId(row.try_get::<Uuid, _>("actor_id")?),
            csrf_token,
        }))
    }

    pub async fn delete(&self, token: &str) -> Result<(), DbError> {
        sqlx::query(
            r#"
            delete from login_sessions
            where token = $1
            "#,
        )
        .bind(token)
        .execute(self.pool)
        .await?;

        Ok(())
    }
}

pub struct PostRepository<'a> {
    pool: &'a SqlitePool,
}

pub struct FollowRepository<'a> {
    pool: &'a SqlitePool,
}

pub struct DeliveryJobRepository<'a> {
    pool: &'a SqlitePool,
}

pub struct InboxDedupRepository<'a> {
    pool: &'a SqlitePool,
}

pub struct PostPage {
    pub posts: Vec<Post>,
    pub next_cursor: Option<PostId>,
}

impl<'a> PostRepository<'a> {
    pub async fn create(&self, post: &Post) -> Result<(), DbError> {
        sqlx::query(
            r#"
            insert into posts (
                id, actor_id, url, content_source, content_format, content_html,
                visibility, in_reply_to, created_at
            ) values ($1, $2, $3, $4, $5, $6, $7, $8, $9)
            "#,
        )
        .bind(post.id.0)
        .bind(post.actor_id.0)
        .bind(post.url.as_str())
        .bind(post.content_source.as_str())
        .bind(content_format_to_db(&post.content_format))
        .bind(&post.content_html)
        .bind(visibility_to_db(&post.visibility))
        .bind(post.in_reply_to.as_ref().map(|id| id.0))
        .bind(post.created_at)
        .execute(self.pool)
        .await?;

        Ok(())
    }

    pub async fn find(&self, id: PostId) -> Result<Option<Post>, DbError> {
        let row = sqlx::query(
            r#"
            select
                id, actor_id, url, content_source, content_format,
                content_html, visibility, in_reply_to, created_at
            from posts
            where id = $1
            "#,
        )
        .bind(id.0)
        .fetch_optional(self.pool)
        .await?;

        row.map(post_from_row).transpose()
    }

    pub async fn find_by_url(&self, url: &Url) -> Result<Option<Post>, DbError> {
        let row = sqlx::query(
            r#"
            select
                id, actor_id, url, content_source, content_format,
                content_html, visibility, in_reply_to, created_at
            from posts
            where url = $1
            "#,
        )
        .bind(url.as_str())
        .fetch_optional(self.pool)
        .await?;

        row.map(post_from_row).transpose()
    }

    pub async fn upsert_remote(&self, post: &Post) -> Result<(), DbError> {
        sqlx::query(
            r#"
            insert into posts (
                id, actor_id, url, content_source, content_format, content_html,
                visibility, in_reply_to, created_at
            ) values ($1, $2, $3, $4, $5, $6, $7, $8, $9)
            on conflict (url) do update set
                actor_id = excluded.actor_id,
                content_source = excluded.content_source,
                content_format = excluded.content_format,
                content_html = excluded.content_html,
                visibility = excluded.visibility,
                in_reply_to = excluded.in_reply_to,
                created_at = excluded.created_at
            "#,
        )
        .bind(post.id.0)
        .bind(post.actor_id.0)
        .bind(post.url.as_str())
        .bind(post.content_source.as_str())
        .bind(content_format_to_db(&post.content_format))
        .bind(&post.content_html)
        .bind(visibility_to_db(&post.visibility))
        .bind(post.in_reply_to.as_ref().map(|id| id.0))
        .bind(post.created_at)
        .execute(self.pool)
        .await?;

        Ok(())
    }

    pub async fn list_by_actor(
        &self,
        actor_id: ActorId,
        before: Option<PostId>,
        limit: i64,
    ) -> Result<PostPage, DbError> {
        let rows = match before {
            Some(before) => {
                let before_post = self.find(before).await?.ok_or(sqlx::Error::RowNotFound)?;

                sqlx::query(
                    r#"
                    select
                        id, actor_id, url, content_source, content_format,
                        content_html, visibility, in_reply_to, created_at
                    from posts
                    where actor_id = $1
                      and (
                        created_at < $2
                        or (created_at = $2 and id < $3)
                      )
                    order by created_at desc, id desc
                    limit $4
                    "#,
                )
                .bind(actor_id.0)
                .bind(before_post.created_at)
                .bind(before_post.id.0)
                .bind(limit + 1)
                .fetch_all(self.pool)
                .await?
            }
            None => {
                sqlx::query(
                    r#"
                    select
                        id, actor_id, url, content_source, content_format,
                        content_html, visibility, in_reply_to, created_at
                    from posts
                    where actor_id = $1
                    order by created_at desc, id desc
                    limit $2
                    "#,
                )
                .bind(actor_id.0)
                .bind(limit + 1)
                .fetch_all(self.pool)
                .await?
            }
        };

        let mut posts: Vec<Post> = rows
            .into_iter()
            .map(post_from_row)
            .collect::<Result<_, _>>()?;
        let next_cursor = if posts.len() > limit as usize {
            posts.pop().expect("extra post for pagination");
            posts.last().map(|post| post.id)
        } else {
            None
        };

        Ok(PostPage { posts, next_cursor })
    }

    pub async fn list_public_by_actor(
        &self,
        actor_id: ActorId,
        before: Option<PostId>,
        limit: i64,
    ) -> Result<PostPage, DbError> {
        let rows = match before {
            Some(before) => {
                let before_post = self.find(before).await?.ok_or(sqlx::Error::RowNotFound)?;

                if before_post.actor_id != actor_id
                    || !is_public_visibility(&before_post.visibility)
                {
                    return Err(DbError::Sqlx(sqlx::Error::RowNotFound));
                }

                sqlx::query(
                    r#"
                    select
                        id, actor_id, url, content_source, content_format,
                        content_html, visibility, in_reply_to, created_at
                    from posts
                    where actor_id = $1
                      and visibility in ('public', 'unlisted')
                      and (
                        created_at < $2
                        or (created_at = $2 and id < $3)
                      )
                    order by created_at desc, id desc
                    limit $4
                    "#,
                )
                .bind(actor_id.0)
                .bind(before_post.created_at)
                .bind(before_post.id.0)
                .bind(limit + 1)
                .fetch_all(self.pool)
                .await?
            }
            None => {
                sqlx::query(
                    r#"
                    select
                        id, actor_id, url, content_source, content_format,
                        content_html, visibility, in_reply_to, created_at
                    from posts
                    where actor_id = $1
                      and visibility in ('public', 'unlisted')
                    order by created_at desc, id desc
                    limit $2
                    "#,
                )
                .bind(actor_id.0)
                .bind(limit + 1)
                .fetch_all(self.pool)
                .await?
            }
        };

        let mut posts: Vec<Post> = rows
            .into_iter()
            .map(post_from_row)
            .collect::<Result<_, _>>()?;
        let next_cursor = if posts.len() > limit as usize {
            posts.pop().expect("extra post for pagination");
            posts.last().map(|post| post.id)
        } else {
            None
        };

        Ok(PostPage { posts, next_cursor })
    }

    pub async fn count_public_by_actor(&self, actor_id: ActorId) -> Result<i64, DbError> {
        let count = sqlx::query_scalar(
            r#"
            select count(*)
            from posts
            where actor_id = $1
              and visibility in ('public', 'unlisted')
            "#,
        )
        .bind(actor_id.0)
        .fetch_one(self.pool)
        .await?;

        Ok(count)
    }
}

impl<'a> FollowRepository<'a> {
    pub async fn upsert(&self, follow: &FollowRelation) -> Result<(), DbError> {
        sqlx::query(
            r#"
            insert into follows (
                local_actor_id, remote_actor_id, remote_actor_url, state, created_at, updated_at
            ) values ($1, $2, $3, $4, $5, $6)
            on conflict (local_actor_id, remote_actor_id) do update set
                remote_actor_url = excluded.remote_actor_url,
                state = excluded.state,
                updated_at = excluded.updated_at
            "#,
        )
        .bind(follow.local_actor_id.0)
        .bind(follow.remote_actor_id.0)
        .bind(follow.remote_actor_url.as_str())
        .bind(follow_state_to_db(&follow.state))
        .bind(follow.created_at)
        .bind(follow.updated_at)
        .execute(self.pool)
        .await?;

        Ok(())
    }

    pub async fn find(
        &self,
        local_actor_id: ActorId,
        remote_actor_id: ActorId,
    ) -> Result<Option<FollowRelation>, DbError> {
        let row = sqlx::query(
            r#"
            select
                local_actor_id, remote_actor_id, remote_actor_url, state, created_at, updated_at
            from follows
            where local_actor_id = $1 and remote_actor_id = $2
            "#,
        )
        .bind(local_actor_id.0)
        .bind(remote_actor_id.0)
        .fetch_optional(self.pool)
        .await?;

        row.map(follow_from_row).transpose()
    }

    pub async fn set_state(
        &self,
        local_actor_id: ActorId,
        remote_actor_id: ActorId,
        state: FollowState,
    ) -> Result<(), DbError> {
        sqlx::query(
            r#"
            update follows
            set state = $3, updated_at = $4
            where local_actor_id = $1 and remote_actor_id = $2
            "#,
        )
        .bind(local_actor_id.0)
        .bind(remote_actor_id.0)
        .bind(follow_state_to_db(&state))
        .bind(Utc::now())
        .execute(self.pool)
        .await?;

        Ok(())
    }

    pub async fn list_active_remote_actors(
        &self,
        local_actor_id: ActorId,
    ) -> Result<Vec<RemoteActor>, DbError> {
        let rows = sqlx::query(
            r#"
            select
                a.id,
                a.username,
                a.display_name,
                a.summary,
                a.actor_url,
                a.inbox_url,
                a.outbox_url,
                s.public_key_pem,
                s.fetched_at
            from follows f
            join actors a on a.id = f.remote_actor_id
            join remote_actor_state s on s.actor_id = a.id
            where f.local_actor_id = $1
              and f.state = 'active'
            order by f.created_at asc
            "#,
        )
        .bind(local_actor_id.0)
        .fetch_all(self.pool)
        .await?;

        rows.into_iter().map(remote_actor_from_row).collect()
    }
}

impl<'a> DeliveryJobRepository<'a> {
    pub async fn create(&self, job: &DeliveryJob) -> Result<(), DbError> {
        sqlx::query(
            r#"
            insert into delivery_jobs (
                id, local_actor_id, target_inbox_url, kind, payload, state, attempts,
                max_attempts, next_attempt_at, last_error, created_at, delivered_at
            ) values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
            "#,
        )
        .bind(job.id.0)
        .bind(job.local_actor_id.0)
        .bind(job.target_inbox_url.as_str())
        .bind(delivery_kind_to_db(&job.kind))
        .bind(&job.payload)
        .bind(delivery_state_to_db(&job.state))
        .bind(job.attempts as i64)
        .bind(job.max_attempts as i64)
        .bind(job.next_attempt_at)
        .bind(&job.last_error)
        .bind(job.created_at)
        .bind(job.delivered_at)
        .execute(self.pool)
        .await?;

        Ok(())
    }

    pub async fn list_due(
        &self,
        now: DateTime<Utc>,
        limit: i64,
    ) -> Result<Vec<DeliveryJob>, DbError> {
        let rows = sqlx::query(
            r#"
            select
                id, local_actor_id, target_inbox_url, kind, payload, state, attempts,
                max_attempts, next_attempt_at, last_error, created_at, delivered_at
            from delivery_jobs
            where state = 'pending'
              and next_attempt_at <= $1
            order by next_attempt_at asc, created_at asc
            limit $2
            "#,
        )
        .bind(now)
        .bind(limit)
        .fetch_all(self.pool)
        .await?;

        rows.into_iter().map(delivery_job_from_row).collect()
    }

    pub async fn find(&self, id: DeliveryJobId) -> Result<Option<DeliveryJob>, DbError> {
        let row = sqlx::query(
            r#"
            select
                id, local_actor_id, target_inbox_url, kind, payload, state, attempts,
                max_attempts, next_attempt_at, last_error, created_at, delivered_at
            from delivery_jobs
            where id = $1
            "#,
        )
        .bind(id.0)
        .fetch_optional(self.pool)
        .await?;

        row.map(delivery_job_from_row).transpose()
    }

    pub async fn mark_processing(&self, id: DeliveryJobId) -> Result<(), DbError> {
        sqlx::query(
            r#"
            update delivery_jobs
            set state = 'processing'
            where id = $1
            "#,
        )
        .bind(id.0)
        .execute(self.pool)
        .await?;

        Ok(())
    }

    pub async fn mark_delivered(&self, id: DeliveryJobId) -> Result<(), DbError> {
        sqlx::query(
            r#"
            update delivery_jobs
            set state = 'delivered', delivered_at = $2, last_error = null
            where id = $1
            "#,
        )
        .bind(id.0)
        .bind(Utc::now())
        .execute(self.pool)
        .await?;

        Ok(())
    }

    pub async fn reschedule(
        &self,
        id: DeliveryJobId,
        attempts: u32,
        next_attempt_at: DateTime<Utc>,
        last_error: String,
    ) -> Result<(), DbError> {
        sqlx::query(
            r#"
            update delivery_jobs
            set state = 'pending', attempts = $2, next_attempt_at = $3, last_error = $4
            where id = $1
            "#,
        )
        .bind(id.0)
        .bind(attempts as i64)
        .bind(next_attempt_at)
        .bind(last_error)
        .execute(self.pool)
        .await?;

        Ok(())
    }

    pub async fn mark_failed(
        &self,
        id: DeliveryJobId,
        attempts: u32,
        last_error: String,
    ) -> Result<(), DbError> {
        sqlx::query(
            r#"
            update delivery_jobs
            set state = 'failed', attempts = $2, last_error = $3
            where id = $1
            "#,
        )
        .bind(id.0)
        .bind(attempts as i64)
        .bind(last_error)
        .execute(self.pool)
        .await?;

        Ok(())
    }

    pub async fn reset_failed(&self) -> Result<u64, DbError> {
        let result = sqlx::query(
            r#"
            update delivery_jobs
            set state = 'pending', next_attempt_at = $1
            where state = 'failed'
            "#,
        )
        .bind(Utc::now())
        .execute(self.pool)
        .await?;

        Ok(result.rows_affected())
    }
}

impl<'a> InboxDedupRepository<'a> {
    pub async fn record(
        &self,
        activity_id: &str,
        actor_id: ActorId,
        activity_type: &str,
    ) -> Result<bool, DbError> {
        let result = sqlx::query(
            r#"
            insert into inbox_dedup (activity_id, actor_id, activity_type, received_at)
            values ($1, $2, $3, $4)
            on conflict (activity_id) do nothing
            "#,
        )
        .bind(activity_id)
        .bind(actor_id.0)
        .bind(activity_type)
        .bind(Utc::now())
        .execute(self.pool)
        .await?;

        Ok(result.rows_affected() > 0)
    }
}

fn local_actor_from_row(row: sqlx::sqlite::SqliteRow) -> Result<LocalActor, DbError> {
    Ok(LocalActor {
        profile: actor_profile_from_columns(&row)?,
        public_key_pem: row.try_get("public_key_pem")?,
        private_key_pem: row.try_get("private_key_pem")?,
    })
}

fn remote_actor_from_row(row: sqlx::sqlite::SqliteRow) -> Result<RemoteActor, DbError> {
    Ok(RemoteActor {
        profile: actor_profile_from_columns(&row)?,
        public_key_pem: row.try_get("public_key_pem")?,
        fetched_at: row.try_get("fetched_at")?,
    })
}

fn actor_profile_from_columns(row: &sqlx::sqlite::SqliteRow) -> Result<ActorProfile, DbError> {
    Ok(ActorProfile {
        id: ActorId(row.try_get::<Uuid, _>("id")?),
        username: row
            .try_get::<String, _>("username")?
            .parse()
            .map_err(DbError::InvalidUsername)?,
        display_name: row
            .try_get::<String, _>("display_name")?
            .parse()
            .map_err(DbError::InvalidTextValue)?,
        summary: row
            .try_get::<Option<String>, _>("summary")?
            .map(|value| value.parse())
            .transpose()
            .map_err(DbError::InvalidTextValue)?,
        actor_url: parse_url(row.try_get("actor_url")?)?,
        inbox_url: parse_optional_url(row.try_get("inbox_url")?)?,
        outbox_url: parse_optional_url(row.try_get("outbox_url")?)?,
    })
}

fn post_from_row(row: sqlx::sqlite::SqliteRow) -> Result<Post, DbError> {
    let content_format = content_format_from_db(&row.try_get::<String, _>("content_format")?)?;
    let visibility = visibility_from_db(&row.try_get::<String, _>("visibility")?)?;

    Ok(Post {
        id: PostId(row.try_get::<Uuid, _>("id")?),
        actor_id: ActorId(row.try_get::<Uuid, _>("actor_id")?),
        url: parse_url(row.try_get("url")?)?,
        content_source: row
            .try_get::<String, _>("content_source")?
            .parse()
            .map_err(DbError::InvalidTextValue)?,
        content_format,
        content_html: row.try_get("content_html")?,
        visibility,
        in_reply_to: row.try_get::<Option<Uuid>, _>("in_reply_to")?.map(PostId),
        created_at: row.try_get::<DateTime<Utc>, _>("created_at")?,
    })
}

fn follow_from_row(row: sqlx::sqlite::SqliteRow) -> Result<FollowRelation, DbError> {
    Ok(FollowRelation {
        local_actor_id: ActorId(row.try_get::<Uuid, _>("local_actor_id")?),
        remote_actor_id: ActorId(row.try_get::<Uuid, _>("remote_actor_id")?),
        remote_actor_url: parse_url(row.try_get("remote_actor_url")?)?,
        state: follow_state_from_db(&row.try_get::<String, _>("state")?)?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn delivery_job_from_row(row: sqlx::sqlite::SqliteRow) -> Result<DeliveryJob, DbError> {
    Ok(DeliveryJob {
        id: DeliveryJobId(row.try_get::<Uuid, _>("id")?),
        local_actor_id: ActorId(row.try_get::<Uuid, _>("local_actor_id")?),
        target_inbox_url: parse_url(row.try_get("target_inbox_url")?)?,
        kind: delivery_kind_from_db(&row.try_get::<String, _>("kind")?)?,
        payload: row.try_get("payload")?,
        state: delivery_state_from_db(&row.try_get::<String, _>("state")?)?,
        attempts: row.try_get::<i64, _>("attempts")? as u32,
        max_attempts: row.try_get::<i64, _>("max_attempts")? as u32,
        next_attempt_at: row.try_get("next_attempt_at")?,
        last_error: row.try_get("last_error")?,
        created_at: row.try_get("created_at")?,
        delivered_at: row.try_get("delivered_at")?,
    })
}

fn parse_url(value: String) -> Result<Url, DbError> {
    Url::parse(&value).map_err(DbError::InvalidUrl)
}

fn parse_optional_url(value: Option<String>) -> Result<Option<Url>, DbError> {
    value.map(parse_url).transpose()
}

fn visibility_to_db(visibility: &Visibility) -> &'static str {
    match visibility {
        Visibility::Public => "public",
        Visibility::Unlisted => "unlisted",
        Visibility::Followers => "followers",
        Visibility::Direct => "direct",
    }
}

fn visibility_from_db(value: &str) -> Result<Visibility, DbError> {
    match value {
        "public" => Ok(Visibility::Public),
        "unlisted" => Ok(Visibility::Unlisted),
        "followers" => Ok(Visibility::Followers),
        "direct" => Ok(Visibility::Direct),
        _ => Err(DbError::UnknownVisibility(value.to_string())),
    }
}

fn content_format_to_db(format: &ContentFormat) -> &'static str {
    match format {
        ContentFormat::Plaintext => "plaintext",
        ContentFormat::Markdown => "markdown",
    }
}

fn content_format_from_db(value: &str) -> Result<ContentFormat, DbError> {
    match value {
        "plaintext" => Ok(ContentFormat::Plaintext),
        "markdown" => Ok(ContentFormat::Markdown),
        _ => Err(DbError::UnknownContentFormat(value.to_string())),
    }
}

fn follow_state_to_db(state: &FollowState) -> &'static str {
    match state {
        FollowState::Pending => "pending",
        FollowState::Active => "active",
        FollowState::Rejected => "rejected",
    }
}

fn follow_state_from_db(value: &str) -> Result<FollowState, DbError> {
    match value {
        "pending" => Ok(FollowState::Pending),
        "active" => Ok(FollowState::Active),
        "rejected" => Ok(FollowState::Rejected),
        _ => Err(DbError::UnknownFollowState(value.to_string())),
    }
}

fn delivery_kind_to_db(kind: &DeliveryKind) -> &'static str {
    match kind {
        DeliveryKind::Follow => "follow",
        DeliveryKind::Create => "create",
        DeliveryKind::Accept => "accept",
    }
}

fn delivery_kind_from_db(value: &str) -> Result<DeliveryKind, DbError> {
    match value {
        "follow" => Ok(DeliveryKind::Follow),
        "create" => Ok(DeliveryKind::Create),
        "accept" => Ok(DeliveryKind::Accept),
        _ => Err(DbError::UnknownDeliveryKind(value.to_string())),
    }
}

fn delivery_state_to_db(state: &DeliveryState) -> &'static str {
    match state {
        DeliveryState::Pending => "pending",
        DeliveryState::Processing => "processing",
        DeliveryState::Delivered => "delivered",
        DeliveryState::Failed => "failed",
    }
}

fn delivery_state_from_db(value: &str) -> Result<DeliveryState, DbError> {
    match value {
        "pending" => Ok(DeliveryState::Pending),
        "processing" => Ok(DeliveryState::Processing),
        "delivered" => Ok(DeliveryState::Delivered),
        "failed" => Ok(DeliveryState::Failed),
        _ => Err(DbError::UnknownDeliveryState(value.to_string())),
    }
}

fn is_public_visibility(visibility: &Visibility) -> bool {
    matches!(visibility, Visibility::Public | Visibility::Unlisted)
}

fn opt_url_str(url: &Option<Url>) -> Option<&str> {
    url.as_ref().map(Url::as_str)
}

pub fn hash_password(password: &str) -> Result<String, AuthError> {
    let salt = SaltString::generate(&mut OsRng);
    let hash = Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map_err(|error| AuthError::Hash(error.to_string()))?;
    Ok(hash.to_string())
}

pub fn verify_password(password: &str, password_hash: &str) -> Result<bool, AuthError> {
    let parsed = PasswordHash::new(password_hash).map_err(|error| AuthError::Hash(error.to_string()))?;
    Ok(Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok())
}

#[derive(Debug, Error)]
pub enum DbError {
    #[error(transparent)]
    Migrate(#[from] sqlx::migrate::MigrateError),
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
    #[error("invalid url in database: {0}")]
    InvalidUrl(url::ParseError),
    #[error("unknown visibility in database: {0}")]
    UnknownVisibility(String),
    #[error("unknown content format in database: {0}")]
    UnknownContentFormat(String),
    #[error("unknown follow state in database: {0}")]
    UnknownFollowState(String),
    #[error("unknown delivery kind in database: {0}")]
    UnknownDeliveryKind(String),
    #[error("unknown delivery state in database: {0}")]
    UnknownDeliveryState(String),
    #[error("invalid username in database: {0}")]
    InvalidUsername(UsernameError),
    #[error("invalid text value in database: {0}")]
    InvalidTextValue(TextValueError),
}

#[derive(Debug, Error)]
pub enum AuthError {
    #[error("unable to hash password: {0}")]
    Hash(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use kodamapub_domain::{DeliveryJob, DeliveryKind, FollowRelation, NewPost, Visibility};
    use url::Url;

    #[test]
    fn maps_visibility_to_db_values() {
        assert_eq!(visibility_to_db(&Visibility::Public), "public");
        assert_eq!(visibility_to_db(&Visibility::Direct), "direct");
    }

    #[test]
    fn maps_content_format_to_db_values() {
        assert_eq!(content_format_to_db(&ContentFormat::Plaintext), "plaintext");
        assert_eq!(content_format_to_db(&ContentFormat::Markdown), "markdown");
    }

    async fn memory_db() -> Database {
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
                Some("local actor".parse().expect("summary")),
                Url::parse("https://example.invalid/users/alice").expect("actor url"),
                Some(Url::parse("https://example.invalid/users/alice/inbox").expect("inbox url")),
                Some(Url::parse("https://example.invalid/users/alice/outbox").expect("outbox url")),
            ),
            public_key_pem: "PUBLIC KEY".to_string(),
            private_key_pem: "PRIVATE KEY".to_string(),
        }
    }

    #[tokio::test]
    async fn migrate_creates_expected_tables() {
        let db = memory_db().await;

        let count: i64 = sqlx::query_scalar(
            r#"
            select count(*)
            from sqlite_master
            where type = 'table'
              and name in (
                'actors',
                'local_actor_secrets',
                'remote_actor_state',
                'posts',
                'follows',
                'delivery_jobs'
              )
            "#,
        )
        .fetch_one(db.pool())
        .await
        .expect("count migrated tables");

        assert_eq!(count, 6);
    }

    #[tokio::test]
    async fn local_actor_repository_round_trips_actor() {
        let db = memory_db().await;
        let actor = sample_local_actor();

        db.local_actors()
            .create(&actor)
            .await
            .expect("create local actor");

        let found = db
            .local_actors()
            .find_by_username(&"alice".parse().expect("username"))
            .await
            .expect("find local actor")
            .expect("local actor exists");

        assert_eq!(found, actor);
    }

    #[tokio::test]
    async fn post_repository_round_trips_post() {
        let db = memory_db().await;
        let actor = sample_local_actor();

        db.local_actors()
            .create(&actor)
            .await
            .expect("create local actor");

        let post = Post::new(
            NewPost {
                actor_id: actor.id(),
                content_source: "hello from sqlite".parse().expect("content source"),
                content_format: ContentFormat::Plaintext,
                visibility: Visibility::Public,
                in_reply_to: None,
            },
            &"https://example.invalid".parse().expect("public base url"),
        )
        .expect("create post");

        db.posts().create(&post).await.expect("insert post");

        let found = db
            .posts()
            .find(post.id)
            .await
            .expect("find post")
            .expect("post exists");

        assert_eq!(found, post);
    }

    #[tokio::test]
    async fn list_public_by_actor_filters_private_posts_and_paginates() {
        let db = memory_db().await;
        let actor = sample_local_actor();

        db.local_actors()
            .create(&actor)
            .await
            .expect("create local actor");

        let public_post = Post::new(
            NewPost {
                actor_id: actor.id(),
                content_source: "public".parse().expect("content source"),
                content_format: ContentFormat::Plaintext,
                visibility: Visibility::Public,
                in_reply_to: None,
            },
            &"https://example.invalid".parse().expect("public base url"),
        )
        .expect("public post");
        let unlisted_post = Post::new(
            NewPost {
                actor_id: actor.id(),
                content_source: "unlisted".parse().expect("content source"),
                content_format: ContentFormat::Plaintext,
                visibility: Visibility::Unlisted,
                in_reply_to: None,
            },
            &"https://example.invalid".parse().expect("public base url"),
        )
        .expect("unlisted post");
        let direct_post = Post::new(
            NewPost {
                actor_id: actor.id(),
                content_source: "direct".parse().expect("content source"),
                content_format: ContentFormat::Plaintext,
                visibility: Visibility::Direct,
                in_reply_to: None,
            },
            &"https://example.invalid".parse().expect("public base url"),
        )
        .expect("direct post");

        db.posts()
            .create(&public_post)
            .await
            .expect("insert public post");
        db.posts()
            .create(&unlisted_post)
            .await
            .expect("insert unlisted post");
        db.posts()
            .create(&direct_post)
            .await
            .expect("insert direct post");

        let page = db
            .posts()
            .list_public_by_actor(actor.id(), None, 1)
            .await
            .expect("public page");

        assert_eq!(page.posts.len(), 1);
        assert!(page.next_cursor.is_some());
        assert!(is_public_visibility(&page.posts[0].visibility));

        let next_page = db
            .posts()
            .list_public_by_actor(actor.id(), page.next_cursor, 1)
            .await
            .expect("next public page");

        assert_eq!(next_page.posts.len(), 1);
        assert!(next_page.next_cursor.is_none());
        assert!(is_public_visibility(&next_page.posts[0].visibility));

        let total = db
            .posts()
            .count_public_by_actor(actor.id())
            .await
            .expect("public post count");
        assert_eq!(total, 2);
    }

    #[tokio::test]
    async fn follow_repository_round_trips_and_lists_active_remote_actors() {
        let db = memory_db().await;
        let local_actor = sample_local_actor();

        db.local_actors()
            .create(&local_actor)
            .await
            .expect("create local actor");

        let remote_actor = RemoteActor {
            profile: ActorProfile::new(
                "bob".parse().expect("username"),
                "Bob".parse().expect("display name"),
                Some("remote actor".parse().expect("summary")),
                Url::parse("https://remote.example/users/bob").expect("actor url"),
                Some(Url::parse("https://remote.example/users/bob/inbox").expect("inbox url")),
                Some(Url::parse("https://remote.example/users/bob/outbox").expect("outbox url")),
            ),
            public_key_pem: Some("REMOTE PUBLIC KEY".to_string()),
            fetched_at: Utc::now(),
        };

        db.remote_actors()
            .upsert(&remote_actor)
            .await
            .expect("upsert remote actor");

        let mut follow = FollowRelation::new(local_actor.id(), &remote_actor);
        follow.state = FollowState::Active;

        db.follows().upsert(&follow).await.expect("upsert follow");

        let found = db
            .follows()
            .find(local_actor.id(), remote_actor.id())
            .await
            .expect("find follow")
            .expect("follow exists");

        assert_eq!(found.state, FollowState::Active);

        let active = db
            .follows()
            .list_active_remote_actors(local_actor.id())
            .await
            .expect("list active follows");
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].profile.actor_url, remote_actor.profile.actor_url);
    }

    #[tokio::test]
    async fn remote_actor_upsert_reuses_existing_actor_url() {
        let db = memory_db().await;

        let remote_actor = RemoteActor {
            profile: ActorProfile::new(
                "bob".parse().expect("username"),
                "Bob".parse().expect("display name"),
                Some("remote actor".parse().expect("summary")),
                Url::parse("https://remote.example/users/bob").expect("actor url"),
                Some(Url::parse("https://remote.example/users/bob/inbox").expect("inbox url")),
                Some(Url::parse("https://remote.example/users/bob/outbox").expect("outbox url")),
            ),
            public_key_pem: Some("REMOTE PUBLIC KEY".to_string()),
            fetched_at: Utc::now(),
        };
        let updated_remote_actor = RemoteActor {
            profile: ActorProfile::new(
                "bob".parse().expect("username"),
                "Bob Updated".parse().expect("display name"),
                Some("remote actor updated".parse().expect("summary")),
                Url::parse("https://remote.example/users/bob").expect("actor url"),
                Some(Url::parse("https://remote.example/users/bob/inbox").expect("inbox url")),
                Some(Url::parse("https://remote.example/users/bob/outbox").expect("outbox url")),
            ),
            public_key_pem: Some("REMOTE PUBLIC KEY 2".to_string()),
            fetched_at: Utc::now(),
        };

        db.remote_actors()
            .upsert(&remote_actor)
            .await
            .expect("upsert remote actor");
        db.remote_actors()
            .upsert(&updated_remote_actor)
            .await
            .expect("upsert remote actor again");

        let found = db
            .remote_actors()
            .find_by_actor_url(&Url::parse("https://remote.example/users/bob").expect("actor url"))
            .await
            .expect("find remote actor")
            .expect("remote actor exists");

        assert_eq!(found.profile.actor_url, remote_actor.profile.actor_url);
        assert_eq!(found.profile.display_name, updated_remote_actor.profile.display_name);
        assert_eq!(found.public_key_pem, updated_remote_actor.public_key_pem);
    }

    #[tokio::test]
    async fn delivery_job_repository_round_trips_and_resets_failed_jobs() {
        let db = memory_db().await;
        let local_actor = sample_local_actor();

        db.local_actors()
            .create(&local_actor)
            .await
            .expect("create local actor");

        let job = DeliveryJob::new(
            local_actor.id(),
            Url::parse("https://remote.example/users/bob/inbox").expect("inbox url"),
            DeliveryKind::Follow,
            "{\"type\":\"Follow\"}".to_string(),
            5,
        );

        db.delivery_jobs()
            .create(&job)
            .await
            .expect("create delivery job");
        db.delivery_jobs()
            .reschedule(job.id, 1, Utc::now(), "network error".to_string())
            .await
            .expect("reschedule delivery job");

        let due = db
            .delivery_jobs()
            .list_due(Utc::now(), 10)
            .await
            .expect("list due jobs");
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].kind, DeliveryKind::Follow);

        db.delivery_jobs()
            .mark_failed(job.id, 2, "final error".to_string())
            .await
            .expect("mark final failed");
        let reset = db
            .delivery_jobs()
            .reset_failed()
            .await
            .expect("reset failed jobs");
        assert_eq!(reset, 1);

        let found = db
            .delivery_jobs()
            .find(job.id)
            .await
            .expect("find delivery job")
            .expect("job exists");
        assert_eq!(found.state, DeliveryState::Pending);
    }
}
