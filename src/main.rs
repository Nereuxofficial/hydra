mod aws;
mod docker;
#[cfg(feature = "gcp")]
mod gcp;
mod infracost;
#[cfg(feature = "libvirt")]
mod libvirt;
mod migration;
mod mock_termination;
use crate::mock_termination::MockTermination;
mod files;
mod provider;
mod ssh;
mod zip;

use crate::aws::AWSInstanceHandler;
use clap::{Parser, Subcommand};
use dotenvy::dotenv;
use std::time::Instant;

///  The CLI interface of hydra to allow for either only migrating or creating a new instance
#[derive(Debug, Parser)]
#[command(name = "hydra")]
#[command(about = "A tool for VM live-migration", long_about = None)]
#[command(subcommand_required = false)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Debug, Subcommand)]
enum Commands {
    #[command(arg_required_else_help = false)]
    CreateInstance,
    #[command(arg_required_else_help = true)]
    Migrate { remote: String },
}

#[tokio::main]
async fn main() {
    dotenv().unwrap();
    tracing_subscriber::fmt::init();
    let args = Cli::parse();
    match args.command {
        Some(Commands::CreateInstance) => todo!(),
        Some(Commands::Migrate { remote }) => {
            todo!()
        }
        None => {
            // TODO: Add support for GCP
            let instancehandler: Box<dyn provider::Provider> = if cfg!(feature = "mock_termination")
            {
                Box::new(MockTermination::new(AWSInstanceHandler::new().await))
            } else {
                Box::new(AWSInstanceHandler::new().await)
            };
            let start = Instant::now();
            let dur = instancehandler
                .wait_until_termination_signal()
                .await
                .unwrap();
            let ip_addr = instancehandler
                .start_instance(std::env::var("INSTANCE_ID").expect("INSTANCE_ID not set"))
                .await
                .unwrap();
            let mut cr_backend: Box<dyn migration::Migration> =
                Box::new(docker::DockerBackend::new().unwrap());
            cr_backend.checkpoint().await.unwrap();
            cr_backend.migrate(ip_addr).await.unwrap();
            println!(
                "Migration took: {}s/{}s",
                start.elapsed().as_secs(),
                dur.as_secs()
            );
        }
    }
}
