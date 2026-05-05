use chrono::{DateTime, Utc};
use kodamapub_domain::{Actor, Post};
use serde::{Deserialize, Serialize};
use url::Url;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ActorObject {
    pub id: Url,
    pub preferred_username: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NoteObject {
    pub id: Url,
    pub content: String,
    pub published: DateTime<Utc>,
}

pub fn actor_to_object(actor: &Actor) -> ActorObject {
    ActorObject {
        id: actor.url.clone(),
        preferred_username: actor.username.clone(),
    }
}

pub fn post_to_note(post: &Post) -> NoteObject {
    NoteObject {
        id: post.url.clone(),
        content: post.content.clone(),
        published: post.created_at,
    }
}
