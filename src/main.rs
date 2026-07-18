use clap::Parser;
use magnumdb::cli::{Cli, Commands};
use magnumdb::config::Config;
use magnumdb::storage::Database;
use magnumdb::sql::{Parser as SqlParser, Executor};
use std::io::{self, Write};

fn main() -> anyhow::Result<()> {
    env_logger::init();

    let cli = Cli::parse();

    match &cli.command {
        Some(Commands::Init { path }) => {
            println!("Initializing database at {:?}", path);
        }
        Some(Commands::Start { config }) => {
            println!("Starting database with config {:?}", config);
        }
        Some(Commands::Stop) => {
            println!("Stopping database");
        }
        Some(Commands::Shell) => {
            println!("MagnumDB Shell. Type 'exit;' or Ctrl-C to quit.");
            let config = Config::default();
            let mut db = Database::open(config)?;
            let mut executor = Executor::new(&mut db);

            loop {
                print!("magnum> ");
                io::stdout().flush()?;

                let mut input = String::new();
                if io::stdin().read_line(&mut input).is_err() {
                    break;
                }

                let input = input.trim();
                if input.is_empty() {
                    continue;
                }
                
                if input.eq_ignore_ascii_case("exit;") || input.eq_ignore_ascii_case("quit;") {
                    break;
                }

                match SqlParser::parse(input) {
                    Ok(stmt) => match executor.execute(stmt) {
                        Ok(result) => println!("{}", result),
                        Err(e) => println!("Error: {}", e),
                    },
                    Err(e) => println!("{}", e),
                }
            }
        }
        Some(Commands::Backup { destination }) => {
            println!("Backing up database to {:?}", destination);
        }
        Some(Commands::Restore { source }) => {
            println!("Restoring database from {:?}", source);
        }
        Some(Commands::Config) => {
            println!("Current configuration:");
        }
        Some(Commands::Version) => {
            println!("MagnumDB v{}", env!("CARGO_PKG_VERSION"));
        }
        Some(Commands::Benchmark) => {
            println!("Running benchmarks...");
        }
        None => {
            println!("MagnumDB - Use --help for usage.");
        }
    }

    Ok(())
}
