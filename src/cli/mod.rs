use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "magnum")]
#[command(about = "MagnumDB Command Line Interface", long_about = None)]
#[command(version)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Initialize a new database cluster
    Init {
        #[arg(short, long, default_value = "./data")]
        path: PathBuf,
    },
    /// Start the database server
    Start {
        #[arg(short, long, default_value = "magnum.toml")]
        config: PathBuf,
    },
    /// Stop the running database server
    Stop,
    /// Open the interactive database shell
    Shell,
    /// Create a consistent backup
    Backup {
        #[arg(short, long)]
        destination: PathBuf,
    },
    /// Restore from a backup
    Restore {
        #[arg(short, long)]
        source: PathBuf,
    },
    /// View or validate configuration
    Config,
    /// Display version information
    Version,
    /// Run performance tests
    Benchmark,
}
