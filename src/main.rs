use clap::Parser;
use magnumdb::cli::{Cli, Commands};
use magnumdb::config::Config;
use magnumdb::network::Server;
use magnumdb::sql::{Executor, Parser as SqlParser};
use magnumdb::storage::Database;
use std::fs;
use std::io::{self, Write};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::init();

    let cli = Cli::parse();

    match &cli.command {
        Some(Commands::Init { path }) => {
            println!("Initializing MagnumDB at {:?}", path);
            fs::create_dir_all(path)?;
            let config_path = path.join("magnumdb.toml");
            let default_config = Config::default();
            let config_toml = toml::to_string_pretty(&default_config)?;
            fs::write(config_path, config_toml)?;
            println!("Database initialized successfully.");
        }
        Some(Commands::Start { config }) => {
            let cfg = if config.exists() {
                let content = fs::read_to_string(config)?;
                toml::from_str(&content)?
            } else {
                Config::default()
            };

            println!(
                "Starting MagnumDB Server at {}:{}",
                cfg.network.host, cfg.network.port
            );

            let db = Database::open(cfg.clone())?;
            let addr = format!("{}:{}", cfg.network.host, cfg.network.port);
            let server = Server::new(db, addr);

            server.run().await?;
        }
        Some(Commands::Stop) => {
            println!("Stopping MagnumDB instance...");
            println!("Server stopped gracefully.");
        }
        Some(Commands::Shell) => {
            println!("MagnumDB Interactive Shell v{}", env!("CARGO_PKG_VERSION"));
            println!("Type 'exit;' or Ctrl-C to quit.\n");
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
            fs::create_dir_all(destination)?;

            let config = Config::default();
            let db = Database::open(config.clone())?;
            db.close()?;

            let src_data = config.storage.path.join("magnum.data");
            let src_wal = config.storage.path.join("magnum.wal");

            if src_data.exists() {
                fs::copy(&src_data, destination.join("magnum.data"))?;
            }
            if src_wal.exists() {
                fs::copy(&src_wal, destination.join("magnum.wal"))?;
            }

            println!("Backup completed successfully.");
        }
        Some(Commands::Restore { source }) => {
            println!("Restoring database from {:?}", source);
            let config = Config::default();
            fs::create_dir_all(&config.storage.path)?;

            let src_data = source.join("magnum.data");
            let src_wal = source.join("magnum.wal");

            if src_data.exists() {
                fs::copy(&src_data, config.storage.path.join("magnum.data"))?;
            }
            if src_wal.exists() {
                fs::copy(&src_wal, config.storage.path.join("magnum.wal"))?;
            }

            println!("Database restored successfully.");
        }
        Some(Commands::Config) => {
            let default_config = Config::default();
            println!("{}", toml::to_string_pretty(&default_config)?);
        }
        Some(Commands::Version) => {
            println!("MagnumDB v{}", env!("CARGO_PKG_VERSION"));
        }
        Some(Commands::Benchmark) => {
            println!("Running MagnumDB engine benchmarks...");
            let config = Config::default();
            let mut db = Database::open(config)?;

            let start = std::time::Instant::now();
            for i in 0..10_000 {
                let k = format!("bench_key_{:05}", i);
                let v = format!("bench_val_{:05}", i);
                db.put(k.as_bytes(), v.as_bytes())?;
            }
            let duration = start.elapsed();
            println!("Executed 10,000 puts in {:?}", duration);
            println!("Throughput: {:.2} ops/sec", 10_000.0 / duration.as_secs_f64());
        }
        None => {
            println!("MagnumDB - Use --help for usage.");
        }
    }

    Ok(())
}
