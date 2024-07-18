mod aws;
mod docker;
mod gcp;
mod infracost;
mod instances;
mod libvirt;
mod ssh;

use crate::libvirt::QemuConnection;
use crate::ssh::get_ssh_key_from_ip;
use clap::{Parser, Subcommand};
use dotenvy::dotenv;
use gcp::create_instance_with_image;
use std::time::Instant;
use tracing::info;

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
    let mut last = Instant::now();
    let start = Instant::now();
    dotenv().unwrap();
    tracing_subscriber::fmt::init();
    let args = Cli::parse();
    match args.command {
        Some(Commands::CreateInstance) => println!(
            "Ip Address of new server: {}",
            create_instance_with_image().await
        ),
        Some(Commands::Migrate { remote }) => {
            let connection = QemuConnection::new();
            let domains = connection.get_running_vms();
            connection.migrate(Some(remote), domains);
        }
        None => {
            let connection = QemuConnection::new();
            let domains = connection.get_running_vms();
            assert!(!domains.is_empty(), "No running domains to migrate");
            info!("Migration starting... Requesting new machine to be started...");
            println!("Startup: {}ms", last.elapsed().as_millis());
            last = Instant::now();
            let ip_address = create_instance_with_image().await;
            println!("Instance Creation: {}ms", last.elapsed().as_millis());
            last = Instant::now();
            get_ssh_key_from_ip(ip_address).await;
            println!("known_hosts: {}ms", last.elapsed().as_millis());
            last = Instant::now();
            connection.migrate(Some(format!("qemu+ssh://{}/session", ip_address)), domains);
            println!("Migration: {}ms", last.elapsed().as_millis());
            let duration = start.elapsed();
            info!("Migration completed in {}ms", duration.as_millis());
        }
    }
}
