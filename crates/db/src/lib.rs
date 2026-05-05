use chrono::{DateTime, Utc};
use kodamapub_domain::{
    ActorId, ActorProfile, ContentFormat, LocalActor, Post, PostId, RemoteActor, Summary,
    TextValueError, Username, UsernameError, Visibility,
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
}

pub struct RemoteActorRepository<'a> {
    pool: &'a SqlitePool,
}

impl<'a> RemoteActorRepository<'a> {
    pub async fn upsert(&self, actor: &RemoteActor) -> Result<(), DbError> {
        let mut tx = self.pool.begin().await?;

        sqlx::query(
            r#"
            insert into actors (
                id, username, display_name, summary, actor_url, inbox_url, outbox_url, created_at
            ) values ($1, $2, $3, $4, $5, $6, $7, current_timestamp)
            on conflict (id) do update set
                username = excluded.username,
                display_name = excluded.display_name,
                summary = excluded.summary,
                actor_url = excluded.actor_url,
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
}

pub struct PostRepository<'a> {
    pool: &'a SqlitePool,
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

    pub async fn list_by_actor(&self, actor_id: ActorId, limit: i64) -> Result<Vec<Post>, DbError> {
        let rows = sqlx::query(
            r#"
            select
                id, actor_id, url, content_source, content_format,
                content_html, visibility, in_reply_to, created_at
            from posts
            where actor_id = $1
            order by created_at desc
            limit $2
            "#,
        )
        .bind(actor_id.0)
        .bind(limit)
        .fetch_all(self.pool)
        .await?;

        rows.into_iter().map(post_from_row).collect()
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

fn opt_url_str(url: &Option<Url>) -> Option<&str> {
    url.as_ref().map(Url::as_str)
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
    #[error("invalid username in database: {0}")]
    InvalidUsername(UsernameError),
    #[error("invalid text value in database: {0}")]
    InvalidTextValue(TextValueError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use kodamapub_domain::{NewPost, Visibility};
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
              and name in ('actors', 'local_actor_secrets', 'remote_actor_state', 'posts')
            "#,
        )
        .fetch_one(db.pool())
        .await
        .expect("count migrated tables");

        assert_eq!(count, 4);
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
}
