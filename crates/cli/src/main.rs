use clap::{Parser, Subcommand};
use kodamapub_db::Database;

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
    }

    Ok(())
}
