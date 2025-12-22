//! Rnostr cli
use clap::Parser;
#[macro_use]
extern crate clap;

use rnostr::*;

/// Cli
#[derive(Debug, Parser)]
#[command(name = "rnostr", about = "Rnostr cli.", version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

/// Commands
#[derive(Debug, Subcommand)]
enum Commands {
    /// Import data from jsonl file
    #[command(arg_required_else_help = true)]
    Import(ImportOpts),
    /// Export data to jsonl file
    #[command(arg_required_else_help = true)]
    Export(ExportOpts),
    /// Benchmark filter
    #[command(arg_required_else_help = true)]
    Bench(BenchOpts),
    /// Start nostr relay server
    Relay(RelayOpts),
    /// Delete data by filter
    Delete(DeleteOpts),
    /// Clean up expired keypackages
    Cleanup,
}

fn main() -> anyhow::Result<()> {
    let args = Cli::parse();
    match args.command {
        Commands::Import(opts) => {
            let total = import_opts(opts)?;
            println!("imported {} events", total);
        }
        Commands::Export(opts) => {
            export_opts(opts)?;
        }
        Commands::Bench(opts) => {
            bench_opts(opts)?;
        }
        Commands::Relay(opts) => {
            relay(&opts.config, opts.watch)?;
        }
        Commands::Delete(opts) => {
            let count = delete(&opts.path, &opts.filter, opts.dry_run)?;
            if opts.dry_run {
                println!("Would delete {} events", count);
            } else {
                println!("Deleted {} events", count);
            }
        }
        Commands::Cleanup => {
            #[cfg(feature = "mls_gateway_firestore")]
            {
                // Initialize tracing for logging
                tracing_subscriber::fmt::init();
                
                // Run cleanup in async context using actix-rt
                let system = actix_rt::System::new();
                system.block_on(async {
                    rnostr::cleanup::run_cleanup().await
                })?;
            }
            
            #[cfg(not(feature = "mls_gateway_firestore"))]
            {
                eprintln!("Error: Cleanup command requires mls_gateway_firestore feature to be enabled");
                eprintln!("Build with: cargo build --features mls_gateway_firestore");
                std::process::exit(1);
            }
        }
    }
    Ok(())
}
