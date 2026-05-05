use chrono::{DateTime, Utc};
use kodamapub_domain::{ActorProfile, LocalActor, Post, PostId, RemoteActor, Visibility};
use serde::{Deserialize, Serialize};
use url::Url;

const ACTIVITY_STREAMS_CONTEXT: &str = "https://www.w3.org/ns/activitystreams";
const SECURITY_CONTEXT: &str = "https://w3id.org/security/v1";
const PUBLIC_COLLECTION: &str = "https://www.w3.org/ns/activitystreams#Public";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PublicKeyObject {
    pub id: Url,
    pub owner: Url,
    #[serde(rename = "publicKeyPem")]
    pub public_key_pem: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ActorObject {
    #[serde(rename = "@context")]
    pub context: Vec<String>,
    pub id: Url,
    #[serde(rename = "type")]
    pub object_type: String,
    #[serde(rename = "preferredUsername")]
    pub preferred_username: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    pub inbox: Url,
    pub outbox: Url,
    pub followers: Url,
    pub following: Url,
    #[serde(rename = "publicKey", skip_serializing_if = "Option::is_none")]
    pub public_key: Option<PublicKeyObject>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NoteObject {
    #[serde(rename = "@context")]
    pub context: String,
    pub id: Url,
    #[serde(rename = "type")]
    pub object_type: String,
    #[serde(rename = "attributedTo")]
    pub attributed_to: Url,
    pub content: String,
    pub published: DateTime<Utc>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub to: Vec<Url>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub cc: Vec<Url>,
    #[serde(rename = "inReplyTo", skip_serializing_if = "Option::is_none")]
    pub in_reply_to: Option<Url>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreateActivity {
    #[serde(rename = "@context")]
    pub context: String,
    pub id: Url,
    #[serde(rename = "type")]
    pub object_type: String,
    pub actor: Url,
    pub published: DateTime<Utc>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub to: Vec<Url>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub cc: Vec<Url>,
    pub object: NoteObject,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OrderedCollection {
    #[serde(rename = "@context")]
    pub context: String,
    pub id: Url,
    #[serde(rename = "type")]
    pub object_type: String,
    #[serde(rename = "totalItems")]
    pub total_items: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first: Option<Url>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OrderedCollectionPage {
    #[serde(rename = "@context")]
    pub context: String,
    pub id: Url,
    #[serde(rename = "type")]
    pub object_type: String,
    #[serde(rename = "partOf")]
    pub part_of: Url,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next: Option<Url>,
    #[serde(rename = "orderedItems")]
    pub ordered_items: Vec<CreateActivity>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WebFingerLink {
    pub rel: String,
    #[serde(rename = "type")]
    pub media_type: String,
    pub href: Url,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WebFingerResponse {
    pub subject: String,
    pub links: Vec<WebFingerLink>,
}

pub fn actor_profile_to_object(actor: &ActorProfile) -> ActorObject {
    ActorObject {
        context: vec![
            ACTIVITY_STREAMS_CONTEXT.to_string(),
            SECURITY_CONTEXT.to_string(),
        ],
        id: actor.actor_url.clone(),
        object_type: "Person".to_string(),
        preferred_username: actor.username.to_string(),
        name: actor.display_name.to_string(),
        summary: actor.summary.as_ref().map(ToString::to_string),
        inbox: actor
            .inbox_url
            .clone()
            .unwrap_or_else(|| derived_collection_url(&actor.actor_url, "inbox")),
        outbox: actor
            .outbox_url
            .clone()
            .unwrap_or_else(|| derived_collection_url(&actor.actor_url, "outbox")),
        followers: derived_collection_url(&actor.actor_url, "followers"),
        following: derived_collection_url(&actor.actor_url, "following"),
        public_key: None,
    }
}

pub fn local_actor_to_object(actor: &LocalActor) -> ActorObject {
    let mut object = actor_profile_to_object(&actor.profile);
    object.public_key = Some(PublicKeyObject {
        id: key_id_url(&actor.profile.actor_url),
        owner: actor.profile.actor_url.clone(),
        public_key_pem: actor.public_key_pem.clone(),
    });
    object
}

pub fn remote_actor_to_object(actor: &RemoteActor) -> ActorObject {
    let mut object = actor_profile_to_object(&actor.profile);
    object.public_key = actor
        .public_key_pem
        .clone()
        .map(|public_key_pem| PublicKeyObject {
            id: key_id_url(&actor.profile.actor_url),
            owner: actor.profile.actor_url.clone(),
            public_key_pem,
        });
    object
}

pub fn post_to_note(post: &Post, actor: &ActorProfile) -> NoteObject {
    let (to, cc) = visibility_audience(&post.visibility);

    NoteObject {
        context: ACTIVITY_STREAMS_CONTEXT.to_string(),
        id: post.url.clone(),
        object_type: "Note".to_string(),
        attributed_to: actor.actor_url.clone(),
        content: post.content_html.clone(),
        published: post.created_at,
        to,
        cc,
        in_reply_to: post
            .in_reply_to
            .and_then(|reply_to| reply_to_post_url(post, reply_to)),
    }
}

pub fn post_to_create_activity(post: &Post, actor: &ActorProfile) -> CreateActivity {
    let note = post_to_note(post, actor);
    let (to, cc) = (note.to.clone(), note.cc.clone());

    CreateActivity {
        context: ACTIVITY_STREAMS_CONTEXT.to_string(),
        id: create_activity_id(&note.id),
        object_type: "Create".to_string(),
        actor: actor.actor_url.clone(),
        published: post.created_at,
        to,
        cc,
        object: note,
    }
}

pub fn ordered_collection(id: Url, first: Option<Url>, total_items: u64) -> OrderedCollection {
    OrderedCollection {
        context: ACTIVITY_STREAMS_CONTEXT.to_string(),
        id,
        object_type: "OrderedCollection".to_string(),
        total_items,
        first,
    }
}

pub fn ordered_collection_page(
    id: Url,
    part_of: Url,
    next: Option<Url>,
    ordered_items: Vec<CreateActivity>,
) -> OrderedCollectionPage {
    OrderedCollectionPage {
        context: ACTIVITY_STREAMS_CONTEXT.to_string(),
        id,
        object_type: "OrderedCollectionPage".to_string(),
        part_of,
        next,
        ordered_items,
    }
}

pub fn webfinger_response(subject: String, actor_url: Url) -> WebFingerResponse {
    WebFingerResponse {
        subject,
        links: vec![WebFingerLink {
            rel: "self".to_string(),
            media_type: "application/activity+json".to_string(),
            href: actor_url,
        }],
    }
}

pub fn is_publicly_visible(visibility: &Visibility) -> bool {
    matches!(visibility, Visibility::Public | Visibility::Unlisted)
}

fn visibility_audience(visibility: &Visibility) -> (Vec<Url>, Vec<Url>) {
    let public = Url::parse(PUBLIC_COLLECTION).expect("public collection URL");

    match visibility {
        Visibility::Public | Visibility::Unlisted => (vec![public], Vec::new()),
        Visibility::Followers | Visibility::Direct => (Vec::new(), Vec::new()),
    }
}

fn derived_collection_url(actor_url: &Url, suffix: &str) -> Url {
    Url::parse(&format!(
        "{}/{}",
        actor_url.as_str().trim_end_matches('/'),
        suffix
    ))
    .expect("derived collection URL")
}

fn key_id_url(actor_url: &Url) -> Url {
    Url::parse(&format!("{}#main-key", actor_url.as_str())).expect("actor key URL")
}

fn create_activity_id(note_id: &Url) -> Url {
    Url::parse(&format!("{}#create", note_id.as_str())).expect("create activity URL")
}

fn reply_to_post_url(post: &Post, reply_to: PostId) -> Option<Url> {
    let (prefix, _) = post.url.as_str().rsplit_once("/posts/")?;
    Url::parse(&format!("{prefix}/posts/{reply_to}")).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use kodamapub_domain::{ActorProfile, ContentFormat, LocalActor, NewPost, Username};

    fn sample_actor_profile() -> ActorProfile {
        ActorProfile::new(
            Username::try_from("alice".to_string()).expect("username"),
            "Alice".parse().expect("display name"),
            Some("summary".parse().expect("summary")),
            Url::parse("https://example.invalid/users/alice").expect("actor url"),
            Some(Url::parse("https://example.invalid/users/alice/inbox").expect("inbox url")),
            Some(Url::parse("https://example.invalid/users/alice/outbox").expect("outbox url")),
        )
    }

    fn sample_local_actor() -> LocalActor {
        LocalActor {
            profile: sample_actor_profile(),
            public_key_pem: "PUBLIC KEY".to_string(),
            private_key_pem: "PRIVATE KEY".to_string(),
        }
    }

    fn sample_post(visibility: Visibility) -> Post {
        Post::new(
            NewPost {
                actor_id: sample_local_actor().id(),
                content_source: "hello".parse().expect("content source"),
                content_format: ContentFormat::Plaintext,
                visibility,
                in_reply_to: None,
            },
            &"https://example.invalid".parse().expect("public base url"),
        )
        .expect("post")
    }

    #[test]
    fn local_actor_object_contains_public_key_and_context() {
        let actor = sample_local_actor();

        let object = local_actor_to_object(&actor);

        assert_eq!(object.object_type, "Person");
        assert_eq!(object.preferred_username, "alice");
        assert_eq!(
            object.context,
            vec![
                ACTIVITY_STREAMS_CONTEXT.to_string(),
                SECURITY_CONTEXT.to_string()
            ]
        );
        assert_eq!(
            object.public_key.expect("public key").id,
            Url::parse("https://example.invalid/users/alice#main-key").expect("key id")
        );
    }

    #[test]
    fn note_for_public_post_targets_public_collection() {
        let actor = sample_actor_profile();
        let note = post_to_note(&sample_post(Visibility::Public), &actor);

        assert_eq!(note.object_type, "Note");
        assert_eq!(
            note.to,
            vec![Url::parse(PUBLIC_COLLECTION).expect("public collection")]
        );
    }

    #[test]
    fn webfinger_response_points_to_actor() {
        let response = webfinger_response(
            "acct:alice@example.invalid".to_string(),
            Url::parse("https://example.invalid/users/alice").expect("actor url"),
        );

        assert_eq!(response.subject, "acct:alice@example.invalid");
        assert_eq!(response.links.len(), 1);
        assert_eq!(response.links[0].rel, "self");
        assert_eq!(response.links[0].media_type, "application/activity+json");
    }
}
