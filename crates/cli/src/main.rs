use clap::{Parser, Subcommand};
use kodamapub_domain::{
    ActorProfile, DisplayName, LocalActor, PublicBaseUrl, Summary, Username,
};
use kodamapub_db::Database;
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
            let base = public_base_url.as_str().trim_end_matches('/');
            let actor = LocalActor {
                profile: ActorProfile::new(
                    username.clone(),
                    display_name,
                    summary,
                    Url::parse(&format!("{base}/users/{username}"))?,
                    Some(Url::parse(&format!("{base}/users/{username}/inbox"))?),
                    Some(Url::parse(&format!("{base}/users/{username}/outbox"))?),
                ),
                // Placeholder keys until real key generation is implemented.
                public_key_pem: format!("LOCAL PUBLIC KEY {}", username),
                private_key_pem: format!("LOCAL PRIVATE KEY {}", username),
            };

            let db = Database::connect(&database_url).await?;
            db.migrate().await?;
            db.local_actors().create(&actor).await?;
            println!("{}", actor.profile.actor_url);
        }
    }

    Ok(())
}
