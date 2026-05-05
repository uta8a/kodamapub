use std::{fmt, str::FromStr};

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

impl fmt::Display for PostId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl FromStr for PostId {
    type Err = uuid::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let uuid = Uuid::parse_str(value)?;
        Ok(Self(uuid))
    }
}

impl TryFrom<String> for PostId {
    type Error = uuid::Error;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        value.parse()
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

fn validate_nonempty_string(
    value: &str,
    max_len: usize,
    label: &'static str,
) -> Result<String, TextValueError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(TextValueError::Empty(label));
    }
    if trimmed.len() > max_len {
        return Err(TextValueError::TooLong(label, max_len));
    }
    Ok(trimmed.to_string())
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Username(String);

impl Username {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Username {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl FromStr for Username {
    type Err = UsernameError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.is_empty() {
            return Err(UsernameError::Empty);
        }
        if value.len() > 32 {
            return Err(UsernameError::TooLong);
        }
        if !value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'_' | b'-'))
        {
            return Err(UsernameError::InvalidCharacters);
        }
        if !value
            .as_bytes()
            .first()
            .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        {
            return Err(UsernameError::InvalidStart);
        }
        Ok(Self(value.to_string()))
    }
}

impl TryFrom<String> for Username {
    type Error = UsernameError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        value.parse()
    }
}

impl Serialize for Username {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for Username {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        value.parse().map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DisplayName(String);

impl DisplayName {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for DisplayName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl FromStr for DisplayName {
    type Err = TextValueError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Ok(Self(validate_nonempty_string(value, 64, "display_name")?))
    }
}

impl Serialize for DisplayName {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for DisplayName {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        value.parse().map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Summary(String);

impl Summary {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Summary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl FromStr for Summary {
    type Err = TextValueError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Ok(Self(validate_nonempty_string(value, 500, "summary")?))
    }
}

impl Serialize for Summary {
    fn serialize<S>(self: &Self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for Summary {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        value.parse().map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ContentSource(String);

impl ContentSource {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ContentSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl FromStr for ContentSource {
    type Err = TextValueError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Ok(Self(validate_nonempty_string(
            value,
            10000,
            "content_source",
        )?))
    }
}

impl Serialize for ContentSource {
    fn serialize<S>(self: &Self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for ContentSource {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        value.parse().map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicBaseUrl(Url);

impl PublicBaseUrl {
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl fmt::Display for PublicBaseUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.as_str().fmt(f)
    }
}

impl FromStr for PublicBaseUrl {
    type Err = PublicBaseUrlError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let url = Url::parse(value).map_err(PublicBaseUrlError::Parse)?;
        if url.scheme() != "http" && url.scheme() != "https" {
            return Err(PublicBaseUrlError::InvalidScheme);
        }
        if url.host_str().is_none() {
            return Err(PublicBaseUrlError::MissingHost);
        }
        if url.query().is_some() || url.fragment().is_some() {
            return Err(PublicBaseUrlError::MustNotContainQueryOrFragment);
        }
        if url.path() != "/" && !url.path().is_empty() {
            return Err(PublicBaseUrlError::MustBeOriginOnly);
        }
        Ok(Self(url))
    }
}

impl Serialize for PublicBaseUrl {
    fn serialize<S>(self: &Self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.0.as_str())
    }
}

impl<'de> Deserialize<'de> for PublicBaseUrl {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        value.parse().map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ActorProfile {
    pub id: ActorId,
    pub username: Username,
    pub display_name: DisplayName,
    pub summary: Option<Summary>,
    pub actor_url: Url,
    pub inbox_url: Option<Url>,
    pub outbox_url: Option<Url>,
}

impl ActorProfile {
    pub fn new(
        username: Username,
        display_name: DisplayName,
        summary: Option<Summary>,
        actor_url: Url,
        inbox_url: Option<Url>,
        outbox_url: Option<Url>,
    ) -> Self {
        Self {
            id: ActorId::new(),
            username,
            display_name,
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
    pub content_source: ContentSource,
    pub content_format: ContentFormat,
    pub visibility: Visibility,
    pub in_reply_to: Option<PostId>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Post {
    pub id: PostId,
    pub actor_id: ActorId,
    pub url: Url,
    pub content_source: ContentSource,
    pub content_format: ContentFormat,
    pub content_html: String,
    pub visibility: Visibility,
    pub in_reply_to: Option<PostId>,
    pub created_at: DateTime<Utc>,
}

impl Post {
    pub fn new(new_post: NewPost, public_base_url: &PublicBaseUrl) -> Result<Self, DomainError> {
        let base_url = public_base_url.as_str().trim_end_matches('/');
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

pub fn render_content(source: &ContentSource, format: &ContentFormat) -> String {
    match format {
        // For the first implementation, keep rendering conservative and safe.
        // Markdown support can become richer later without changing the model.
        ContentFormat::Plaintext | ContentFormat::Markdown => {
            render_plaintext_like(source.as_str())
        }
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
    #[error("invalid public base url: {0}")]
    InvalidPublicBaseUrl(url::ParseError),
    #[error("not found")]
    NotFound,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum UsernameError {
    #[error("username must not be empty")]
    Empty,
    #[error("username must be at most 32 characters")]
    TooLong,
    #[error("username must start with an ASCII lowercase letter or digit")]
    InvalidStart,
    #[error("username may only contain ASCII lowercase letters, digits, '_' or '-'")]
    InvalidCharacters,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum TextValueError {
    #[error("{0} must not be empty")]
    Empty(&'static str),
    #[error("{0} must be at most {1} characters")]
    TooLong(&'static str, usize),
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum PublicBaseUrlError {
    #[error("public base url parse error: {0}")]
    Parse(url::ParseError),
    #[error("public base url must use http or https")]
    InvalidScheme,
    #[error("public base url must contain a host")]
    MissingHost,
    #[error("public base url must not contain query or fragment")]
    MustNotContainQueryOrFragment,
    #[error("public base url must be origin-only")]
    MustBeOriginOnly,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_plaintext_escapes_html_and_preserves_lines() {
        let html = render_content(
            &"hello <world>\nnext line".parse().expect("content source"),
            &ContentFormat::Plaintext,
        );
        assert_eq!(html, "<p>hello &lt;world&gt;<br>\nnext line</p>");
    }

    #[test]
    fn post_new_generates_url_and_html() {
        let post = Post::new(
            NewPost {
                actor_id: ActorId::new(),
                content_source: "hello".parse().expect("content source"),
                content_format: ContentFormat::Plaintext,
                visibility: Visibility::Public,
                in_reply_to: None,
            },
            &"https://example.invalid/".parse().expect("public base url"),
        )
        .expect("create post");

        assert_eq!(
            post.url.as_str(),
            format!("https://example.invalid/posts/{}", post.id.0)
        );
        assert_eq!(post.content_html, "<p>hello</p>");
    }

    #[test]
    fn username_parses_valid_value() {
        let username: Username = "alice-1".parse().expect("parse username");
        assert_eq!(username.as_str(), "alice-1");
    }

    #[test]
    fn username_rejects_invalid_value() {
        let error = "Alice".parse::<Username>().expect_err("reject username");
        assert_eq!(error, UsernameError::InvalidCharacters);
    }

    #[test]
    fn display_name_rejects_empty_value() {
        let error = "   "
            .parse::<DisplayName>()
            .expect_err("reject display name");
        assert_eq!(error, TextValueError::Empty("display_name"));
    }

    #[test]
    fn public_base_url_rejects_path() {
        let error = "https://example.invalid/api"
            .parse::<PublicBaseUrl>()
            .expect_err("reject base url");
        assert_eq!(error, PublicBaseUrlError::MustBeOriginOnly);
    }
}
