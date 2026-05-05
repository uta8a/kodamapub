use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use url::Url;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(transparent)]
pub struct ActorId(pub Uuid);

impl ActorId {
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(transparent)]
pub struct PostId(pub Uuid);

impl PostId {
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum Visibility {
    Public,
    Unlisted,
    Followers,
    Direct,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ContentFormat {
    Plaintext,
    Markdown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ActorProfile {
    pub id: ActorId,
    pub username: String,
    pub display_name: String,
    pub summary: Option<String>,
    pub actor_url: Url,
    pub inbox_url: Option<Url>,
    pub outbox_url: Option<Url>,
}

impl ActorProfile {
    pub fn new(
        username: impl Into<String>,
        display_name: impl Into<String>,
        summary: Option<String>,
        actor_url: Url,
        inbox_url: Option<Url>,
        outbox_url: Option<Url>,
    ) -> Self {
        Self {
            id: ActorId::new(),
            username: username.into(),
            display_name: display_name.into(),
            summary,
            actor_url,
            inbox_url,
            outbox_url,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LocalActor {
    pub profile: ActorProfile,
    pub public_key_pem: String,
    pub private_key_pem: String,
}

impl LocalActor {
    pub fn id(&self) -> ActorId {
        self.profile.id
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoteActor {
    pub profile: ActorProfile,
    pub public_key_pem: Option<String>,
    pub fetched_at: DateTime<Utc>,
}

impl RemoteActor {
    pub fn id(&self) -> ActorId {
        self.profile.id
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NewPost {
    pub actor_id: ActorId,
    pub content_source: String,
    pub content_format: ContentFormat,
    pub visibility: Visibility,
    pub in_reply_to: Option<PostId>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Post {
    pub id: PostId,
    pub actor_id: ActorId,
    pub url: Url,
    pub content_source: String,
    pub content_format: ContentFormat,
    pub content_html: String,
    pub visibility: Visibility,
    pub in_reply_to: Option<PostId>,
    pub created_at: DateTime<Utc>,
}

impl Post {
    pub fn new(new_post: NewPost, public_base_url: &str) -> Result<Self, DomainError> {
        if new_post.content_source.trim().is_empty() {
            return Err(DomainError::EmptyContent);
        }

        let base_url = public_base_url.trim_end_matches('/');
        let id = PostId::new();
        let url = Url::parse(&format!("{base_url}/posts/{}", id.0))
            .map_err(DomainError::InvalidPublicBaseUrl)?;
        let content_html = render_content(&new_post.content_source, &new_post.content_format);

        Ok(Self {
            id,
            actor_id: new_post.actor_id,
            url,
            content_source: new_post.content_source,
            content_format: new_post.content_format,
            content_html,
            visibility: new_post.visibility,
            in_reply_to: new_post.in_reply_to,
            created_at: Utc::now(),
        })
    }
}

pub fn render_content(source: &str, format: &ContentFormat) -> String {
    match format {
        // For the first implementation, keep rendering conservative and safe.
        // Markdown support can become richer later without changing the model.
        ContentFormat::Plaintext | ContentFormat::Markdown => render_plaintext_like(source),
    }
}

fn render_plaintext_like(source: &str) -> String {
    let paragraphs = source
        .split("\n\n")
        .map(escape_html)
        .map(|paragraph| paragraph.replace('\n', "<br>\n"))
        .collect::<Vec<_>>();

    paragraphs
        .into_iter()
        .map(|paragraph| format!("<p>{paragraph}</p>"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn escape_html(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

#[derive(Debug, Error)]
pub enum DomainError {
    #[error("content must not be empty")]
    EmptyContent,
    #[error("invalid public base url: {0}")]
    InvalidPublicBaseUrl(url::ParseError),
    #[error("not found")]
    NotFound,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_plaintext_escapes_html_and_preserves_lines() {
        let html = render_content("hello <world>\nnext line", &ContentFormat::Plaintext);
        assert_eq!(html, "<p>hello &lt;world&gt;<br>\nnext line</p>");
    }

    #[test]
    fn post_new_generates_url_and_html() {
        let post = Post::new(
            NewPost {
                actor_id: ActorId::new(),
                content_source: "hello".to_string(),
                content_format: ContentFormat::Plaintext,
                visibility: Visibility::Public,
                in_reply_to: None,
            },
            "https://example.invalid/",
        )
        .expect("create post");

        assert_eq!(
            post.url.as_str(),
            format!("https://example.invalid/posts/{}", post.id.0)
        );
        assert_eq!(post.content_html, "<p>hello</p>");
    }
}
