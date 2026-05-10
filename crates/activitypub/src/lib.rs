use base64::{Engine as _, engine::general_purpose::STANDARD};
use chrono::{DateTime, Utc};
use http::HeaderMap;
use kodamapub_domain::{
    ActorId, ActorProfile, DeliveryKind, LocalActor, Post, PostId, RemoteActor, Visibility,
};
use reqwest::{
    Client,
    header::{ACCEPT, CONTENT_TYPE, DATE, HOST, HeaderValue},
};
use rsa::{
    RsaPrivateKey, RsaPublicKey,
    pkcs1v15::SigningKey,
    pkcs1v15::VerifyingKey,
    pkcs8::{DecodePrivateKey, DecodePublicKey, EncodePrivateKey, EncodePublicKey, LineEnding},
    rand_core::OsRng,
    signature::{RandomizedSigner, SignatureEncoding, Verifier},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use std::collections::HashMap;
use url::Url;
use uuid::Uuid;

const ACTIVITY_STREAMS_CONTEXT: &str = "https://www.w3.org/ns/activitystreams";
const SECURITY_CONTEXT: &str = "https://w3id.org/security/v1";
const PUBLIC_COLLECTION: &str = "https://www.w3.org/ns/activitystreams#Public";

#[derive(Debug, thiserror::Error)]
pub enum ActivityPubError {
    #[error("request failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("invalid remote resource: {0}")]
    InvalidResource(String),
    #[error("remote actor missing inbox url")]
    MissingInboxUrl,
    #[error("remote actor missing id")]
    MissingActorId,
    #[error("missing signature header")]
    MissingSignatureHeader,
    #[error("missing digest header")]
    MissingDigestHeader,
    #[error("invalid signature header: {0}")]
    InvalidSignatureHeader(String),
    #[error("signature verification failed")]
    SignatureVerificationFailed,
    #[error("unable to generate local actor keypair: {0}")]
    KeyGeneration(String),
    #[error("unable to sign request: {0}")]
    Signing(String),
    #[error("unable to serialize activity payload: {0}")]
    Serialize(#[from] serde_json::Error),
}

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
pub struct FollowActivity {
    #[serde(rename = "@context")]
    pub context: String,
    pub id: Url,
    #[serde(rename = "type")]
    pub object_type: String,
    pub actor: Url,
    pub object: Url,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AcceptActivity {
    #[serde(rename = "@context")]
    pub context: String,
    pub id: Url,
    #[serde(rename = "type")]
    pub object_type: String,
    pub actor: Url,
    pub object: Url,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UndoActivity {
    #[serde(rename = "@context")]
    pub context: String,
    pub id: Url,
    #[serde(rename = "type")]
    pub object_type: String,
    pub actor: Url,
    pub object: FollowActivity,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeleteActivity {
    #[serde(rename = "@context")]
    pub context: String,
    pub id: Url,
    #[serde(rename = "type")]
    pub object_type: String,
    pub actor: Url,
    pub object: Url,
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
    pub media_type: Option<String>,
    pub href: Option<Url>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WebFingerResponse {
    pub subject: String,
    pub links: Vec<WebFingerLink>,
}

#[derive(Debug, Clone)]
pub struct RemoteActorDiscovery {
    pub resource: String,
    pub actor: RemoteActor,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IncomingActivity {
    Follow(IncomingFollowActivity),
    Create(IncomingCreateActivity),
    Accept(IncomingAcceptActivity),
    Undo(IncomingUndoActivity),
    Delete(IncomingDeleteActivity),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IncomingActivityObject {
    Url(Url),
    ObjectId(Url),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncomingFollowActivity {
    pub id: Url,
    pub actor: Url,
    pub object: Url,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncomingCreateActivity {
    pub id: Url,
    pub actor: Url,
    pub object: IncomingNoteObject,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncomingAcceptActivity {
    pub id: Url,
    pub actor: Url,
    pub object: Url,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncomingUndoActivity {
    pub id: Url,
    pub actor: Url,
    pub object: IncomingActivityObject,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncomingDeleteActivity {
    pub id: Url,
    pub actor: Url,
    pub object: Url,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncomingNoteObject {
    pub id: Url,
    pub attributed_to: Url,
    pub content: String,
    pub published: DateTime<Utc>,
    pub to: Vec<Url>,
    pub cc: Vec<Url>,
    pub in_reply_to: Option<Url>,
}

#[derive(Debug, Deserialize)]
struct RemoteActorResponse {
    id: Option<Url>,
    #[serde(rename = "preferredUsername")]
    preferred_username: Option<String>,
    name: Option<String>,
    summary: Option<String>,
    inbox: Option<Url>,
    outbox: Option<Url>,
    #[serde(rename = "publicKey")]
    public_key: Option<PublicKeyObject>,
}

pub fn generate_local_actor_keypair_pem() -> Result<(String, String), ActivityPubError> {
    let mut rng = OsRng;
    let private_key = RsaPrivateKey::new(&mut rng, 2048)
        .map_err(|error| ActivityPubError::KeyGeneration(error.to_string()))?;
    let public_key = private_key.to_public_key();

    let private_key_pem = private_key
        .to_pkcs8_pem(LineEnding::LF)
        .map_err(|error| ActivityPubError::KeyGeneration(error.to_string()))?
        .to_string();
    let public_key_pem = public_key
        .to_public_key_pem(LineEnding::LF)
        .map_err(|error| ActivityPubError::KeyGeneration(error.to_string()))?;

    Ok((public_key_pem, private_key_pem))
}

pub async fn discover_remote_actor(
    client: &Client,
    resource: &str,
) -> Result<RemoteActorDiscovery, ActivityPubError> {
    if let Ok(actor_url) = Url::parse(resource) {
        if actor_url.scheme() != "http" && actor_url.scheme() != "https" {
            return discover_remote_actor_by_webfinger(client, resource).await;
        }
        let actor = fetch_remote_actor(client, &actor_url).await?;
        return Ok(RemoteActorDiscovery {
            resource: resource.to_string(),
            actor,
        });
    }

    discover_remote_actor_by_webfinger(client, resource).await
}

async fn discover_remote_actor_by_webfinger(
    client: &Client,
    resource: &str,
) -> Result<RemoteActorDiscovery, ActivityPubError> {
    let (_username, host) = parse_acct_resource(resource)?;
    let webfinger_url = webfinger_url_for_host(&host, resource)?;

    let webfinger = client
        .get(webfinger_url)
        .header(ACCEPT, "application/jrd+json, application/json")
        .send()
        .await?
        .error_for_status()?
        .json::<WebFingerResponse>()
        .await?;

    let actor_url = webfinger
        .links
        .iter()
        .find(|link| link.rel == "self")
        .and_then(|link| link.href.clone())
        .ok_or_else(|| {
            ActivityPubError::InvalidResource("webfinger self link missing".to_string())
        })?;

    let actor = fetch_remote_actor(client, &actor_url).await?;

    Ok(RemoteActorDiscovery {
        resource: resource.to_string(),
        actor,
    })
}

pub async fn fetch_remote_actor(
    client: &Client,
    actor_url: &Url,
) -> Result<RemoteActor, ActivityPubError> {
    let actor_json = client
        .get(actor_url.clone())
        .header(ACCEPT, "application/activity+json, application/ld+json")
        .send()
        .await?
        .error_for_status()?
        .json::<RemoteActorResponse>()
        .await?;

    let actor_id = actor_json.id.clone().unwrap_or_else(|| actor_url.clone());
    let preferred_username = actor_json.preferred_username.unwrap_or_else(|| {
        actor_id
            .path_segments()
            .and_then(|mut segments| segments.next_back())
            .unwrap_or("remote")
            .to_string()
    });
    let username = preferred_username
        .parse::<kodamapub_domain::Username>()
        .or_else(|_| {
            actor_id
                .path_segments()
                .and_then(|mut segments| segments.next_back())
                .unwrap_or("remote")
                .parse::<kodamapub_domain::Username>()
        })
        .map_err(|error| ActivityPubError::InvalidResource(error.to_string()))?;
    let display_name = actor_json
        .name
        .unwrap_or_else(|| preferred_username.clone())
        .parse::<kodamapub_domain::DisplayName>()
        .map_err(|error| ActivityPubError::InvalidResource(error.to_string()))?;
    let summary = actor_json
        .summary
        .as_deref()
        .and_then(|value| {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.parse::<kodamapub_domain::Summary>())
            }
        })
        .transpose()
        .map_err(|error| ActivityPubError::InvalidResource(error.to_string()))?;

    Ok(RemoteActor {
        profile: ActorProfile {
            id: ActorId::from_stable_name(actor_id.as_str()),
            username,
            display_name,
            summary,
            actor_url: actor_id,
            inbox_url: actor_json.inbox,
            outbox_url: actor_json.outbox,
        },
        public_key_pem: actor_json.public_key.map(|key| key.public_key_pem),
        fetched_at: Utc::now(),
    })
}

pub async fn deliver_signed_activity(
    client: &Client,
    actor: &LocalActor,
    target_inbox_url: &Url,
    body: &str,
) -> Result<(), ActivityPubError> {
    let headers = signed_headers(actor, "post", target_inbox_url, body)?;
    let host = headers
        .get(HOST)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("<invalid-host>");
    let digest = headers
        .get("Digest")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("<invalid-digest>");
    let signature = headers
        .get("Signature")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("<invalid-signature>");

    eprintln!(
        "delivering signed activity target={} host={} digest={} signature={}",
        target_inbox_url, host, digest, signature
    );

    client
        .post(target_inbox_url.clone())
        .headers(headers)
        .body(body.to_string())
        .send()
        .await?
        .error_for_status()?;

    Ok(())
}

pub fn serialize_activity<T: Serialize>(value: &T) -> Result<String, ActivityPubError> {
    serde_json::to_string(value).map_err(ActivityPubError::from)
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

pub fn follow_activity(local_actor: &LocalActor, remote_actor: &RemoteActor) -> FollowActivity {
    FollowActivity {
        context: ACTIVITY_STREAMS_CONTEXT.to_string(),
        id: follow_activity_id(
            &local_actor.profile.actor_url,
            &remote_actor.profile.actor_url,
        ),
        object_type: "Follow".to_string(),
        actor: local_actor.profile.actor_url.clone(),
        object: remote_actor.profile.actor_url.clone(),
    }
}

pub fn accept_activity(
    local_actor: &LocalActor,
    remote_actor: &RemoteActor,
    follow_activity_id: &Url,
) -> AcceptActivity {
    AcceptActivity {
        context: ACTIVITY_STREAMS_CONTEXT.to_string(),
        id: accept_activity_id(
            &local_actor.profile.actor_url,
            &remote_actor.profile.actor_url,
            follow_activity_id,
        ),
        object_type: "Accept".to_string(),
        actor: local_actor.profile.actor_url.clone(),
        object: follow_activity_id.clone(),
    }
}

pub fn undo_follow_activity(
    local_actor: &LocalActor,
    remote_actor: &RemoteActor,
) -> UndoActivity {
    UndoActivity {
        context: ACTIVITY_STREAMS_CONTEXT.to_string(),
        id: undo_activity_id(&local_actor.profile.actor_url, &remote_actor.profile.actor_url),
        object_type: "Undo".to_string(),
        actor: local_actor.profile.actor_url.clone(),
        object: follow_activity(local_actor, remote_actor),
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
            media_type: Some("application/activity+json".to_string()),
            href: Some(actor_url),
        }],
    }
}

pub fn is_publicly_visible(visibility: &Visibility) -> bool {
    matches!(visibility, Visibility::Public | Visibility::Unlisted)
}

pub fn activity_kind_for_payload(payload: &str) -> Result<DeliveryKind, ActivityPubError> {
    let json: serde_json::Value = serde_json::from_str(payload)?;
    match json.get("type").and_then(|value| value.as_str()) {
        Some("Follow") => Ok(DeliveryKind::Follow),
        Some("Create") => Ok(DeliveryKind::Create),
        Some("Accept") => Ok(DeliveryKind::Accept),
        Some(other) => Err(ActivityPubError::InvalidResource(format!(
            "unsupported activity type {other}"
        ))),
        None => Err(ActivityPubError::InvalidResource(
            "activity payload missing type".to_string(),
        )),
    }
}

pub fn parse_incoming_activity(body: &str) -> Result<IncomingActivity, ActivityPubError> {
    let value: serde_json::Value = serde_json::from_str(body)?;
    let activity_type = value
        .get("type")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| ActivityPubError::InvalidResource("activity missing type".to_string()))?;
    let id = parse_url_field(&value, "id")?;
    let actor = parse_url_field(&value, "actor")?;

    match activity_type {
        "Follow" => Ok(IncomingActivity::Follow(IncomingFollowActivity {
            id,
            actor,
            object: parse_activity_object_url_field(&value, "object")?,
        })),
        "Create" => Ok(IncomingActivity::Create(IncomingCreateActivity {
            id,
            actor,
            object: parse_incoming_note_object(value.get("object").ok_or_else(|| {
                ActivityPubError::InvalidResource("create activity missing object".to_string())
            })?)?,
        })),
        "Accept" => Ok(IncomingActivity::Accept(IncomingAcceptActivity {
            id,
            actor,
            object: parse_activity_object_url_field(&value, "object")?,
        })),
        "Undo" => Ok(IncomingActivity::Undo(IncomingUndoActivity {
            id,
            actor,
            object: parse_incoming_object_reference(value.get("object").ok_or_else(|| {
                ActivityPubError::InvalidResource("activity missing object".to_string())
            })?)?,
        })),
        "Delete" => Ok(IncomingActivity::Delete(IncomingDeleteActivity {
            id,
            actor,
            object: parse_activity_object_url_field(&value, "object")?,
        })),
        other => Err(ActivityPubError::InvalidResource(format!(
            "unsupported activity type {other}"
        ))),
    }
}

pub fn verify_incoming_activity_signature(
    headers: &HeaderMap,
    method: &str,
    request_target: &str,
    body: &[u8],
    public_key_pem: &str,
) -> Result<(), ActivityPubError> {
    let signature_header = headers
        .get("Signature")
        .ok_or(ActivityPubError::MissingSignatureHeader)?
        .to_str()
        .map_err(|error: http::header::ToStrError| {
            ActivityPubError::InvalidSignatureHeader(error.to_string())
        })?;
    let digest_header = headers
        .get("Digest")
        .ok_or(ActivityPubError::MissingDigestHeader)?
        .to_str()
        .map_err(|error: http::header::ToStrError| {
            ActivityPubError::InvalidSignatureHeader(error.to_string())
        })?;
    let header_map = parse_signature_header(signature_header)?;
    let headers_list = header_map
        .get("headers")
        .cloned()
        .unwrap_or_else(|| "(request-target) host date digest".to_string());
    let key_id = header_map
        .get("keyId")
        .ok_or_else(|| ActivityPubError::InvalidSignatureHeader("missing keyId".to_string()))?;
    let signature = header_map
        .get("signature")
        .ok_or_else(|| ActivityPubError::InvalidSignatureHeader("missing signature".to_string()))?;
    let key_id_url = key_id
        .parse::<Url>()
        .map_err(|error: url::ParseError| ActivityPubError::InvalidSignatureHeader(error.to_string()))?;
    let key_id_host = key_id_url.host_str().map(|host| match key_id_url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_string(),
    });
    let signature_host = signature_host_header(headers)?;
    let mut candidate_hosts = host_candidates(&signature_host);
    if let Some(key_id_host) = key_id_host {
        candidate_hosts.extend(host_candidates(&key_id_host));
    }
    candidate_hosts.sort();
    candidate_hosts.dedup();

    eprintln!(
        "incoming signature raw key_id={} headers={} signature_header={} digest_header={} method={} request_target={} host={} date={}",
        key_id,
        headers_list,
        signature,
        digest_header,
        method,
        request_target,
        headers
            .get("x-original-host")
            .and_then(|value| value.to_str().ok())
            .or_else(|| headers.get("x-forwarded-host").and_then(|value| value.to_str().ok()))
            .or_else(|| headers.get("Host").and_then(|value| value.to_str().ok()))
            .unwrap_or("-"),
        headers
            .get("Date")
            .and_then(|value| value.to_str().ok())
            .unwrap_or("-")
    );

    let expected_digest = digest_header_value(
        std::str::from_utf8(body)
            .map_err(|error| ActivityPubError::InvalidSignatureHeader(error.to_string()))?,
    );
    if expected_digest != digest_header {
        return Err(ActivityPubError::SignatureVerificationFailed);
    }

    let public_key = RsaPublicKey::from_public_key_pem(public_key_pem)
        .map_err(|error| ActivityPubError::Signing(error.to_string()))?;
    let signature = rsa::pkcs1v15::Signature::try_from(
        STANDARD
            .decode(signature)
            .map_err(|error: base64::DecodeError| {
                ActivityPubError::InvalidSignatureHeader(error.to_string())
            })?
            .as_slice(),
    )
    .map_err(|error: rsa::signature::Error| {
        ActivityPubError::InvalidSignatureHeader(error.to_string())
    })?;
    for candidate_host in candidate_hosts {
        let signing_string = canonical_signature_string(
            headers,
            method,
            request_target,
            &headers_list,
            digest_header,
            Some(&candidate_host),
        )?;
        eprintln!(
            "incoming signature key_id={} headers={} candidate_host={} signing_string={}",
            key_id,
            headers_list,
            candidate_host,
            signing_string
        );

        if verifying_key_matches(&public_key, &signing_string, &signature) {
            return Ok(());
        }
    }

    let _ = key_id;
    Err(ActivityPubError::SignatureVerificationFailed)
}

pub fn signature_key_id_actor_url(headers: &HeaderMap) -> Result<Url, ActivityPubError> {
    let signature_header = headers
        .get("Signature")
        .ok_or(ActivityPubError::MissingSignatureHeader)?
        .to_str()
        .map_err(|error: http::header::ToStrError| {
            ActivityPubError::InvalidSignatureHeader(error.to_string())
        })?;
    let header_map = parse_signature_header(signature_header)?;
    let key_id = header_map
        .get("keyId")
        .ok_or_else(|| ActivityPubError::InvalidSignatureHeader("missing keyId".to_string()))?;
    let mut url: Url = key_id.parse().map_err(|error: url::ParseError| {
        ActivityPubError::InvalidSignatureHeader(error.to_string())
    })?;
    url.set_fragment(None);
    Ok(url)
}

fn parse_signature_header(header: &str) -> Result<HashMap<String, String>, ActivityPubError> {
    let mut values = HashMap::new();
    for part in header.split(',') {
        let (name, value) = part
            .split_once('=')
            .ok_or_else(|| ActivityPubError::InvalidSignatureHeader(header.to_string()))?;
        values.insert(
            name.trim().to_string(),
            value.trim().trim_matches('"').to_string(),
        );
    }

    Ok(values)
}

fn canonical_signature_string(
    headers: &HeaderMap,
    method: &str,
    request_target: &str,
    headers_list: &str,
    digest_header: &str,
    host_override: Option<&str>,
) -> Result<String, ActivityPubError> {
    let mut lines = Vec::new();
    for header_name in headers_list.split_whitespace() {
        match header_name {
            "(request-target)" => {
                lines.push(format!(
                    "(request-target): {} {}",
                    method.to_ascii_lowercase(),
                    request_target
                ));
            }
            "host" => {
                let value = host_override
                    .map(|value| value.to_string())
                    .unwrap_or(signature_host_header(headers)?);
                lines.push(format!("host: {value}"));
            }
            "date" => {
                let value = headers
                    .get("Date")
                    .ok_or_else(|| {
                        ActivityPubError::InvalidSignatureHeader("missing date".to_string())
                    })?
                    .to_str()
                    .map_err(|error| ActivityPubError::InvalidSignatureHeader(error.to_string()))?;
                lines.push(format!("date: {value}"));
            }
            "digest" => lines.push(format!("digest: {digest_header}")),
            other => {
                let value = headers
                    .get(other)
                    .ok_or_else(|| {
                        ActivityPubError::InvalidSignatureHeader(format!("missing {other}"))
                    })?
                    .to_str()
                    .map_err(|error| ActivityPubError::InvalidSignatureHeader(error.to_string()))?;
                lines.push(format!("{other}: {value}"));
            }
        }
    }

    Ok(lines.join("\n"))
}

fn verifying_key_matches(
    public_key: &RsaPublicKey,
    signing_string: &str,
    signature: &rsa::pkcs1v15::Signature,
) -> bool {
    let prefixed = VerifyingKey::<Sha256>::new(public_key.clone());
    if prefixed.verify(signing_string.as_bytes(), signature).is_ok() {
        return true;
    }

    let unprefixed = VerifyingKey::<Sha256>::new_unprefixed(public_key.clone());
    unprefixed.verify(signing_string.as_bytes(), signature).is_ok()
}

fn signature_host_header(headers: &HeaderMap) -> Result<String, ActivityPubError> {
    for header_name in ["x-original-host", "x-forwarded-host", "Host"] {
        if let Some(value) = headers.get(header_name) {
            return value.to_str().map(|value| value.to_string()).map_err(|error| {
                ActivityPubError::InvalidSignatureHeader(error.to_string())
            });
        }
    }

    Err(ActivityPubError::InvalidSignatureHeader(
        "missing host".to_string(),
    ))
}

fn host_candidates(host: &str) -> Vec<String> {
    let mut candidates = vec![host.to_string()];

    if let Ok(parsed) = Url::parse(&format!("https://{host}")) {
        if let Some(host_str) = parsed.host_str() {
            candidates.push(host_str.to_string());
            if parsed.port().is_none() {
                candidates.push(format!("{host_str}:443"));
                candidates.push(format!("{host_str}:80"));
            }
        }
    }

    candidates
}

fn parse_url_field(value: &serde_json::Value, field: &str) -> Result<Url, ActivityPubError> {
    let raw = value
        .get(field)
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| ActivityPubError::InvalidResource(format!("activity missing {field}")))?;
    raw.parse::<Url>()
        .map_err(|error: url::ParseError| ActivityPubError::InvalidResource(error.to_string()))
}

fn parse_activity_object_url_field(
    value: &serde_json::Value,
    field: &str,
) -> Result<Url, ActivityPubError> {
    let Some(raw) = value.get(field) else {
        return Err(ActivityPubError::InvalidResource(format!(
            "activity missing {field}"
        )));
    };

    if let Some(url) = raw.as_str() {
        return url
            .parse::<Url>()
            .map_err(|error: url::ParseError| ActivityPubError::InvalidResource(error.to_string()));
    }

    parse_url_field(raw, "id")
}

fn parse_incoming_object_reference(
    value: &serde_json::Value,
) -> Result<IncomingActivityObject, ActivityPubError> {
    if let Some(url) = value.as_str() {
        return url
            .parse::<Url>()
            .map(IncomingActivityObject::Url)
            .map_err(|error: url::ParseError| ActivityPubError::InvalidResource(error.to_string()));
    }

    parse_url_field(value, "id").map(IncomingActivityObject::ObjectId)
}

fn parse_url_list(value: Option<&serde_json::Value>) -> Result<Vec<Url>, ActivityPubError> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };

    if let Some(raw) = value.as_str() {
        return raw
            .parse::<Url>()
            .map(|url| vec![url])
            .map_err(|error: url::ParseError| {
                ActivityPubError::InvalidResource(error.to_string())
            });
    }

    let Some(array) = value.as_array() else {
        return Ok(Vec::new());
    };

    array
        .iter()
        .filter_map(serde_json::Value::as_str)
        .map(|item| {
            item.parse::<Url>().map_err(|error: url::ParseError| {
                ActivityPubError::InvalidResource(error.to_string())
            })
        })
        .collect()
}

fn parse_optional_url(value: Option<&serde_json::Value>) -> Result<Option<Url>, ActivityPubError> {
    match value.and_then(serde_json::Value::as_str) {
        Some(raw) => raw
            .parse::<Url>()
            .map(Some)
            .map_err(|error: url::ParseError| ActivityPubError::InvalidResource(error.to_string())),
        None => Ok(None),
    }
}

fn parse_incoming_note_object(
    value: &serde_json::Value,
) -> Result<IncomingNoteObject, ActivityPubError> {
    Ok(IncomingNoteObject {
        id: parse_url_field(value, "id")?,
        attributed_to: parse_url_field(value, "attributedTo")?,
        content: value
            .get("content")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string(),
        published: value
            .get("published")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| ActivityPubError::InvalidResource("note missing published".to_string()))?
            .parse::<DateTime<Utc>>()
            .map_err(|error: chrono::ParseError| {
                ActivityPubError::InvalidResource(error.to_string())
            })?,
        to: parse_url_list(value.get("to"))?,
        cc: parse_url_list(value.get("cc"))?,
        in_reply_to: parse_optional_url(value.get("inReplyTo"))?,
    })
}

fn signed_headers(
    actor: &LocalActor,
    method: &str,
    url: &Url,
    body: &str,
) -> Result<HeaderMap, ActivityPubError> {
    let digest = digest_header_value(body);
    let host =
        host_header_value(url).map_err(|error| ActivityPubError::Signing(error.to_string()))?;
    let date = http_date(Utc::now());
    let path = match url.query() {
        Some(query) => format!("{}?{query}", url.path()),
        None => url.path().to_string(),
    };
    let signing_string = format!(
        "(request-target): {} {}\nhost: {}\ndate: {}\ndigest: {}\ncontent-type: application/activity+json",
        method.to_ascii_lowercase(),
        path,
        host,
        date,
        digest
    );

    let private_key = RsaPrivateKey::from_pkcs8_pem(&actor.private_key_pem)
        .map_err(|error| ActivityPubError::Signing(error.to_string()))?;
    let signing_key = SigningKey::<Sha256>::new(private_key);
    let signature = signing_key.sign_with_rng(&mut OsRng, signing_string.as_bytes());
    let signature_header = format!(
        "keyId=\"{}\",algorithm=\"rsa-sha256\",headers=\"(request-target) host date digest content-type\",signature=\"{}\"",
        key_id_url(&actor.profile.actor_url),
        STANDARD.encode(signature.to_vec())
    );

    let mut headers = HeaderMap::new();
    headers.insert(
        ACCEPT,
        HeaderValue::from_static("application/activity+json, application/ld+json"),
    );
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("application/activity+json"),
    );
    headers.insert(
        HOST,
        HeaderValue::from_str(&host)
            .map_err(|error| ActivityPubError::Signing(error.to_string()))?,
    );
    headers.insert(
        DATE,
        HeaderValue::from_str(&date)
            .map_err(|error| ActivityPubError::Signing(error.to_string()))?,
    );
    headers.insert(
        "Digest",
        HeaderValue::from_str(&digest)
            .map_err(|error| ActivityPubError::Signing(error.to_string()))?,
    );
    headers.insert(
        "Signature",
        HeaderValue::from_str(&signature_header)
            .map_err(|error| ActivityPubError::Signing(error.to_string()))?,
    );

    Ok(headers)
}

fn parse_acct_resource(resource: &str) -> Result<(String, String), ActivityPubError> {
    let value = resource
        .strip_prefix("acct:")
        .ok_or_else(|| ActivityPubError::InvalidResource(resource.to_string()))?;
    let (username, host) = value
        .split_once('@')
        .ok_or_else(|| ActivityPubError::InvalidResource(resource.to_string()))?;
    Ok((username.to_string(), host.to_string()))
}

fn webfinger_url_for_host(host: &str, resource: &str) -> Result<Url, ActivityPubError> {
    let scheme = if host.starts_with("localhost")
        || host.starts_with("127.0.0.1")
        || host.starts_with("[::1]")
        || host.starts_with("host.docker.internal")
    {
        "http"
    } else {
        "https"
    };
    Url::parse(&format!(
        "{scheme}://{host}/.well-known/webfinger?resource={resource}"
    ))
    .map_err(|error| ActivityPubError::InvalidResource(error.to_string()))
}

fn digest_header_value(body: &str) -> String {
    let hash = Sha256::digest(body.as_bytes());
    format!("SHA-256={}", STANDARD.encode(hash))
}

fn host_header_value(url: &Url) -> Result<String, ActivityPubError> {
    let host = url
        .host_str()
        .ok_or_else(|| ActivityPubError::InvalidResource(url.to_string()))?;
    Ok(match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_string(),
    })
}

fn http_date(now: DateTime<Utc>) -> String {
    now.format("%a, %d %b %Y %H:%M:%S GMT").to_string()
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

fn follow_activity_id(local_actor_url: &Url, remote_actor_url: &Url) -> Url {
    Url::parse(&format!(
        "{}#follow-{}-{}",
        local_actor_url.as_str(),
        sanitize_for_fragment(remote_actor_url.host_str().unwrap_or("remote")),
        Uuid::now_v7()
    ))
    .expect("follow activity URL")
}

fn accept_activity_id(
    local_actor_url: &Url,
    remote_actor_url: &Url,
    follow_activity_id: &Url,
) -> Url {
    Url::parse(&format!(
        "{}#accept-{}-{}-{}",
        local_actor_url.as_str(),
        sanitize_for_fragment(remote_actor_url.host_str().unwrap_or("remote")),
        sanitize_for_fragment(follow_activity_id.path()),
        Uuid::now_v7()
    ))
    .expect("accept activity URL")
}

fn undo_activity_id(local_actor_url: &Url, remote_actor_url: &Url) -> Url {
    Url::parse(&format!(
        "{}#undo-follow-{}-{}",
        local_actor_url.as_str(),
        sanitize_for_fragment(remote_actor_url.host_str().unwrap_or("remote")),
        Uuid::now_v7()
    ))
    .expect("undo activity URL")
}

fn sanitize_for_fragment(value: &str) -> String {
    value
        .chars()
        .map(|char| {
            if char.is_ascii_alphanumeric() || matches!(char, '-' | '_') {
                char
            } else {
                '-'
            }
        })
        .collect()
}

fn reply_to_post_url(post: &Post, reply_to: PostId) -> Option<Url> {
    let (prefix, _) = post.url.as_str().rsplit_once("/posts/")?;
    Url::parse(&format!("{prefix}/posts/{reply_to}")).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Json, Router,
        extract::Query,
        response::IntoResponse,
        routing::{get, post},
    };
    use kodamapub_domain::{ActorProfile, ContentFormat, NewPost, Username};
    use std::{collections::HashMap, sync::Arc};
    use tokio::sync::Mutex;

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
        let (public_key_pem, private_key_pem) =
            generate_local_actor_keypair_pem().expect("generate keypair");
        LocalActor {
            profile: sample_actor_profile(),
            public_key_pem,
            private_key_pem,
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

    #[test]
    fn follow_activity_targets_remote_actor() {
        let local_actor = sample_local_actor();
        let remote_actor = RemoteActor {
            profile: ActorProfile::new(
                "bob".parse().expect("username"),
                "Bob".parse().expect("display name"),
                None,
                Url::parse("https://remote.example/users/bob").expect("actor url"),
                Some(Url::parse("https://remote.example/users/bob/inbox").expect("inbox")),
                Some(Url::parse("https://remote.example/users/bob/outbox").expect("outbox")),
            ),
            public_key_pem: None,
            fetched_at: Utc::now(),
        };

        let activity = follow_activity(&local_actor, &remote_actor);
        assert_eq!(activity.object_type, "Follow");
        assert_eq!(activity.actor, local_actor.profile.actor_url);
        assert_eq!(activity.object, remote_actor.profile.actor_url);
    }

    #[test]
    fn parse_undo_activity_accepts_embedded_follow_object() {
        let body = serde_json::json!({
            "@context": "https://www.w3.org/ns/activitystreams",
            "id": "https://remote.example/activities/undo-1",
            "type": "Undo",
            "actor": "https://remote.example/users/bob",
            "object": {
                "id": "https://remote.example/activities/follow-1",
                "type": "Follow",
                "actor": "https://remote.example/users/bob",
                "object": "https://example.invalid/users/alice"
            }
        })
        .to_string();

        let activity = parse_incoming_activity(&body).expect("parse undo activity");

        match activity {
            IncomingActivity::Undo(value) => match value.object {
                IncomingActivityObject::ObjectId(url) => assert_eq!(
                    url.as_str(),
                    "https://remote.example/activities/follow-1"
                ),
                IncomingActivityObject::Url(_) => panic!("expected embedded activity id"),
            },
            other => panic!("unexpected activity: {other:?}"),
        }
    }

    #[tokio::test]
    async fn discovers_remote_actor_from_webfinger_and_actor_json() {
        async fn webfinger(Query(query): Query<HashMap<String, String>>) -> impl IntoResponse {
            Json(webfinger_response(
                query["resource"].clone(),
                Url::parse("http://127.0.0.1:38901/users/bob").expect("actor url"),
            ))
        }

        async fn actor() -> impl IntoResponse {
            Json(serde_json::json!({
                "@context": "https://www.w3.org/ns/activitystreams",
                "id": "http://127.0.0.1:38901/users/bob",
                "preferredUsername": "bob",
                "name": "Bob",
                "summary": "remote actor",
                "inbox": "http://127.0.0.1:38901/users/bob/inbox",
                "outbox": "http://127.0.0.1:38901/users/bob/outbox"
            }))
        }

        let app = Router::new()
            .route("/.well-known/webfinger", get(webfinger))
            .route("/users/bob", get(actor));

        let listener = tokio::net::TcpListener::bind("127.0.0.1:38901")
            .await
            .expect("bind test server");
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve");
        });

        let client = Client::new();
        let discovery = discover_remote_actor(&client, "acct:bob@127.0.0.1:38901")
            .await
            .expect("discover remote actor");

        assert_eq!(discovery.actor.profile.username.as_str(), "bob");
        assert_eq!(
            discovery.actor.profile.inbox_url.expect("inbox").as_str(),
            "http://127.0.0.1:38901/users/bob/inbox"
        );

        server.abort();
    }

    #[tokio::test]
    async fn signed_delivery_posts_activity_payload() {
        #[derive(Clone, Default)]
        struct InboxState {
            requests: Arc<Mutex<Vec<(String, String)>>>,
        }

        async fn inbox(
            headers: HeaderMap,
            axum::extract::State(state): axum::extract::State<InboxState>,
            body: String,
        ) -> impl IntoResponse {
            let signature = headers
                .get("Signature")
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_string();
            state.requests.lock().await.push((signature, body));
            axum::http::StatusCode::ACCEPTED
        }

        let state = InboxState::default();
        let app = Router::new()
            .route("/inbox", post(inbox))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:38902")
            .await
            .expect("bind test server");
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve");
        });

        let actor = sample_local_actor();
        let remote_inbox = Url::parse("http://127.0.0.1:38902/inbox").expect("inbox url");
        let payload = serialize_activity(&follow_activity(
            &actor,
            &RemoteActor {
                profile: ActorProfile::new(
                    "bob".parse().expect("username"),
                    "Bob".parse().expect("display name"),
                    None,
                    Url::parse("http://127.0.0.1:38902/users/bob").expect("actor url"),
                    Some(remote_inbox.clone()),
                    None,
                ),
                public_key_pem: None,
                fetched_at: Utc::now(),
            },
        ))
        .expect("serialize activity");

        deliver_signed_activity(&Client::new(), &actor, &remote_inbox, &payload)
            .await
            .expect("deliver activity");

        let requests = state.requests.lock().await;
        assert_eq!(requests.len(), 1);
        assert!(requests[0].0.contains("rsa-sha256"));
        assert!(requests[0].1.contains("\"type\":\"Follow\""));

        server.abort();
    }

    #[test]
    fn verify_incoming_signature_prefers_forwarded_host() {
        let actor = sample_local_actor();
        let body = serde_json::json!({
            "@context": "https://www.w3.org/ns/activitystreams",
            "id": "https://remote.example/activities/follow-1",
            "type": "Follow",
            "actor": "https://remote.example/users/bob",
            "object": "https://example.invalid/users/alice"
        })
        .to_string();
        let target = Url::parse("https://mastodon.e2e:3001/users/alice/inbox")
            .expect("target inbox url");

        let mut headers = signed_headers(&actor, "post", &target, &body).expect("sign headers");
        headers.insert(HOST, HeaderValue::from_static("server"));
        headers.insert(
            "x-forwarded-host",
            HeaderValue::from_static("mastodon.e2e:3001"),
        );

        verify_incoming_activity_signature(
            &headers,
            "POST",
            "/users/alice/inbox",
            body.as_bytes(),
            &actor.public_key_pem,
        )
        .expect("verify signature");
    }
}
