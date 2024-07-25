mod aws;
mod docker;
mod gcp;
mod infracost;
#[cfg(feature = "libvirt")]
mod libvirt;
mod migration;
mod provider;
mod ssh;

use crate::aws::AWSInstanceHandler;
use crate::provider::Provider;
use crate::ssh::get_ssh_key_from_ip;
use aws::wait_until_termination_notice;
use clap::{Parser, Subcommand};
use dotenvy::dotenv;
use std::pin::pin;
use std::time::{Duration, Instant};

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
    let last = Instant::now();
    let start = Instant::now();
    dotenv().unwrap();
    tracing_subscriber::fmt::init();
    let args = Cli::parse();
    match args.command {
        Some(Commands::CreateInstance) => todo!(),
        Some(Commands::Migrate { remote }) => {
            todo!()
        }
        None => {
            let instancehandler: Box<dyn provider::Provider> =
                Box::new(AWSInstanceHandler::new().await);
            let dur = instancehandler
                .wait_until_termination_signal()
                .await
                .unwrap();
            let ip_addr = instancehandler
                .start_instance(std::env::var("INSTANCE_ID").unwrap())
                .await
                .unwrap();
            let mut cr_backend: Box<dyn migration::Migration> =
                Box::new(docker::DockerBackend::new().unwrap());
            cr_backend.checkpoint().await.unwrap();
            cr_backend.migrate(ip_addr).await.unwrap();
        }
    }
}
