use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "kodamapub")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    Version,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();
    let cli = Cli::parse();

    match cli.command.unwrap_or(Command::Version) {
        Command::Version => {
            println!("{}", env!("CARGO_PKG_VERSION"));
        }
    }
}
