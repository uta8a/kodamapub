use chrono::{DateTime, Utc};
use kodamapub_domain::{ActorProfile, LocalActor, Post, RemoteActor};
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

pub fn actor_profile_to_object(actor: &ActorProfile) -> ActorObject {
    ActorObject {
        id: actor.actor_url.clone(),
        preferred_username: actor.username.clone(),
    }
}

pub fn local_actor_to_object(actor: &LocalActor) -> ActorObject {
    actor_profile_to_object(&actor.profile)
}

pub fn remote_actor_to_object(actor: &RemoteActor) -> ActorObject {
    actor_profile_to_object(&actor.profile)
}

pub fn post_to_note(post: &Post) -> NoteObject {
    NoteObject {
        id: post.url.clone(),
        content: post.content_html.clone(),
        published: post.created_at,
    }
}
