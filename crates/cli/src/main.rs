use clap::{Parser, Subcommand};
use kodamapub_activitypub::{discover_remote_actor, generate_local_actor_keypair_pem};
use kodamapub_db::Database;
use kodamapub_domain::{
    ActorProfile, ContentFormat, ContentSource, DisplayName, LocalActor, NewPost, Post,
    PublicBaseUrl, Summary, Username, Visibility,
};
use kodamapub_job::{
    RetryPolicy, enqueue_follow_delivery, process_due_jobs, retry_failed_deliveries,
};
use reqwest::Client;
use url::Url;

#[derive(Debug, Parser)]
#[command(name = "kodamapub")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    Version,
    Migrate {
        #[arg(long, env = "DATABASE_URL", default_value = "sqlite://kodamapub.db")]
        database_url: String,
    },
    CreateLocalActor {
        #[arg(long, env = "DATABASE_URL", default_value = "sqlite://kodamapub.db")]
        database_url: String,
        #[arg(long, env = "PUBLIC_BASE_URL", default_value = "http://127.0.0.1:3000")]
        public_base_url: PublicBaseUrl,
        #[arg(long)]
        username: Username,
        #[arg(long)]
        display_name: DisplayName,
        #[arg(long)]
        summary: Option<Summary>,
    },
    SeedDemo {
        #[arg(long, env = "DATABASE_URL", default_value = "sqlite://kodamapub.db")]
        database_url: String,
        #[arg(long, env = "PUBLIC_BASE_URL", default_value = "http://127.0.0.1:8080")]
        public_base_url: PublicBaseUrl,
        #[arg(long, default_value = "alice")]
        username: Username,
        #[arg(long, default_value = "Alice")]
        display_name: DisplayName,
        #[arg(long, default_value = "Seeded demo account")]
        summary: Summary,
    },
    Follow {
        #[arg(long, env = "DATABASE_URL", default_value = "sqlite://kodamapub.db")]
        database_url: String,
        #[arg(long)]
        local_username: Username,
        #[arg(long)]
        resource: String,
    },
    RetryDeliveries {
        #[arg(long, env = "DATABASE_URL", default_value = "sqlite://kodamapub.db")]
        database_url: String,
    },
    RunDeliveries {
        #[arg(long, env = "DATABASE_URL", default_value = "sqlite://kodamapub.db")]
        database_url: String,
        #[arg(long, default_value_t = 100)]
        limit: i64,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _ = dotenvy::dotenv();
    tracing_subscriber::fmt::init();
    let cli = Cli::parse();

    match cli.command.unwrap_or(Command::Version) {
        Command::Version => {
            println!("{}", env!("CARGO_PKG_VERSION"));
        }
        Command::Migrate { database_url } => {
            let db = Database::connect(&database_url).await?;
            db.migrate().await?;
            println!("migrated {database_url}");
        }
        Command::CreateLocalActor {
            database_url,
            public_base_url,
            username,
            display_name,
            summary,
        } => {
            let db = Database::connect(&database_url).await?;
            db.migrate().await?;
            let actor = build_local_actor(public_base_url, username, display_name, summary)?;
            db.local_actors().create(&actor).await?;
            println!("{}", actor.profile.actor_url);
        }
        Command::SeedDemo {
            database_url,
            public_base_url,
            username,
            display_name,
            summary,
        } => {
            let db = Database::connect(&database_url).await?;
            db.migrate().await?;

            let actor = match db.local_actors().find_by_username(&username).await? {
                Some(actor) => actor,
                None => {
                    let actor = build_local_actor(
                        public_base_url.clone(),
                        username.clone(),
                        display_name,
                        Some(summary),
                    )?;
                    db.local_actors().create(&actor).await?;
                    actor
                }
            };

            let mut created = 0usize;
            for content in demo_posts() {
                let post = Post::new(
                    NewPost {
                        actor_id: actor.id(),
                        content_source: content.parse::<ContentSource>()?,
                        content_format: ContentFormat::Plaintext,
                        visibility: Visibility::Public,
                        in_reply_to: None,
                    },
                    &public_base_url,
                )?;
                db.posts().create(&post).await?;
                created += 1;
            }

            println!(
                "seeded actor {} with {} posts",
                actor.profile.actor_url, created
            );
        }
        Command::Follow {
            database_url,
            local_username,
            resource,
        } => {
            let db = Database::connect(&database_url).await?;
            db.migrate().await?;

            let local_actor = db
                .local_actors()
                .find_by_username(&local_username)
                .await?
                .ok_or_else(|| anyhow::anyhow!("local actor not found: {local_username}"))?;

            let discovery = discover_remote_actor(&Client::new(), &resource).await?;
            db.remote_actors().upsert(&discovery.actor).await?;

            let job = enqueue_follow_delivery(
                &db,
                &local_actor,
                &discovery.actor,
                &RetryPolicy::default(),
            )
            .await?;
            let summary = process_due_jobs(&db, &RetryPolicy::default(), 100).await?;

            println!(
                "queued follow job {} and delivered={} rescheduled={} failed={}",
                job.id.0, summary.delivered, summary.rescheduled, summary.failed
            );
        }
        Command::RetryDeliveries { database_url } => {
            let db = Database::connect(&database_url).await?;
            db.migrate().await?;
            let reset = retry_failed_deliveries(&db).await?;
            println!("reset {reset} failed delivery jobs");
        }
        Command::RunDeliveries {
            database_url,
            limit,
        } => {
            let db = Database::connect(&database_url).await?;
            db.migrate().await?;
            let summary = process_due_jobs(&db, &RetryPolicy::default(), limit).await?;
            println!(
                "delivered={} rescheduled={} failed={}",
                summary.delivered, summary.rescheduled, summary.failed
            );
        }
    }

    Ok(())
}

fn build_local_actor(
    public_base_url: PublicBaseUrl,
    username: Username,
    display_name: DisplayName,
    summary: Option<Summary>,
) -> anyhow::Result<LocalActor> {
    let base = public_base_url.as_str().trim_end_matches('/');
    let (public_key_pem, private_key_pem) = generate_local_actor_keypair_pem()?;
    let actor = LocalActor {
        profile: ActorProfile::new(
            username.clone(),
            display_name,
            summary,
            Url::parse(&format!("{base}/users/{username}"))?,
            Some(Url::parse(&format!("{base}/users/{username}/inbox"))?),
            Some(Url::parse(&format!("{base}/users/{username}/outbox"))?),
        ),
        public_key_pem,
        private_key_pem,
    };

    Ok(actor)
}

fn demo_posts() -> [&'static str; 3] {
    [
        "seed post 1: hello from kodamapub",
        "seed post 2: this is a local timeline sample",
        "seed post 3: Docker compose seeding works",
    ]
}
