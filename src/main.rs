mod libvirt;
mod ssh;

use crate::libvirt::QemuConnection;
use crate::ssh::{get_ssh_key, get_ssh_key_from_ip};
use clap::{Parser, Subcommand};
use dotenvy::dotenv;
use gcloud_sdk::google_rest_apis::compute_v1::instances_api::{
    compute_instances_get, compute_instances_insert, ComputePeriodInstancesPeriodGetParams,
    ComputePeriodInstancesPeriodInsertParams,
};
use gcloud_sdk::google_rest_apis::compute_v1::machine_images_api::{
    compute_machine_images_list, ComputePeriodMachineImagesPeriodListParams,
};
use gcloud_sdk::google_rest_apis::compute_v1::scheduling::ProvisioningModel;
use gcloud_sdk::google_rest_apis::compute_v1::{Instance, Metadata, Scheduling};
use gcloud_sdk::{TokenSourceType, GCP_DEFAULT_SCOPES};
use rand::{thread_rng, Rng};
use std::fs::read_to_string;
use std::net::{IpAddr, Ipv4Addr};
use std::str::FromStr;
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
            let start = Instant::now();
            let ip_address = create_instance_with_image().await;
            println!(
                "Created Instance. Time used {}s/30s",
                start.elapsed().as_secs()
            );
            get_ssh_key_from_ip(ip_address).await;
            println!(
                "known_hosts entry added. Time used: {}s/30s",
                start.elapsed().as_secs()
            );
            connection.migrate(Some(format!("qemu+ssh://{}/session", ip_address)), domains);
            let duration = start.elapsed();
            info!("Migration completed in {}s/30s", duration.as_secs(),);
        }
    }
}

async fn get_zone() -> Result<String, reqwest::Error> {
    let client = reqwest::Client::new();
    let response = client
        .get("http://metadata.google.internal/computeMetadata/v1/instance/zone")
        .header("Metadata-Flavor", "Google")
        .send()
        .await?;
    Ok(response
        .text()
        .await?
        .split('/')
        .last()
        .unwrap()
        .to_string())
}

/// Creates a new instance using the first machine image it finds. It also adds the ssh key to the
/// metadata of the new instance to allow for migration.
async fn create_instance_with_image() -> IpAddr {
    let mut rng = thread_rng();
    let zone = get_zone()
        .await
        .unwrap_or_else(|_| std::env::var("ZONE").unwrap());
    let project = gcloud_sdk::GoogleEnvironment::detect_google_project_id()
        .await
        .unwrap_or_else(|| std::env::var("GCP_PROJECT").unwrap());
    info!(
        "Creating operation in project '{}' and zone '{}'",
        project, zone
    );
    let client = gcloud_sdk::GoogleRestApi::with_token_source(
        TokenSourceType::Json(read_to_string(".key.json").unwrap()),
        GCP_DEFAULT_SCOPES.clone(),
    )
    .await
    .unwrap();
    let compute_v1_config = client.create_google_compute_v1_config().await.unwrap();
    let mut machine_images = compute_machine_images_list(
        &compute_v1_config,
        ComputePeriodMachineImagesPeriodListParams {
            project: project.clone(),
            ..Default::default()
        },
    )
    .await
    .unwrap()
    .items
    .unwrap();
    // TODO: We need a way to provide one via env optimally
    let machine_image = machine_images.first_mut().unwrap();
    info!("Machine image: {:?}", machine_image.name.clone());
    let mut properties = machine_image.instance_properties.as_ref().unwrap().clone();
    // Edit the metadata to add our machine's ssh public key while preserving the previous ssh keys
    let metadata_items = properties
        .metadata
        .as_mut()
        .unwrap()
        .items
        .as_mut()
        .unwrap();
    metadata_items.into_iter().for_each(|item| {
        if item.key.as_ref().unwrap() == "ssh-keys" {
            let mut value = item.value.as_ref().unwrap().clone();
            value.push_str("\n");
            value.push_str(get_ssh_key().as_str());
            item.value = Some(value);
        }
    });

    let name = format!("nested-qemu-{}", rng.gen::<u32>());
    let operation = compute_instances_insert(
        &compute_v1_config,
        ComputePeriodInstancesPeriodInsertParams {
            project: project.clone(),
            zone: zone.clone(),
            source_machine_image: Some(format!(
                "projects/{}/global/machineImages/{}",
                project,
                machine_image.name.as_ref().unwrap()
            )),
            instance: Some(Instance {
                name: Some(name.clone()),
                scheduling: Some(Box::new(Scheduling {
                    preemptible: Some(false),
                    provisioning_model: Some(ProvisioningModel::Standard),
                    instance_termination_action: Some(None),
                    ..Default::default()
                })),
                metadata: Some(Box::new(Metadata {
                    fingerprint: machine_image
                        .clone()
                        .instance_properties
                        .unwrap()
                        .metadata
                        .unwrap()
                        .fingerprint
                        .clone(),
                    items: Some(metadata_items.clone()),
                    kind: machine_image
                        .clone()
                        .instance_properties
                        .unwrap()
                        .metadata
                        .unwrap()
                        .kind
                        .clone(),
                })),
                ..Default::default()
            }),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    info!("Instance created: {:?}", operation);
    // Maybe we could also give back the instance
    let details = loop {
        let res = compute_instances_get(
            &compute_v1_config,
            ComputePeriodInstancesPeriodGetParams {
                project: project.clone(),
                zone: zone.clone(),
                instance: name.clone(),
                ..Default::default()
            },
        )
        .await;
        if let Ok(details) = res {
            match details.network_interfaces.clone() {
                Some(network_interfaces) => {
                    // We can safely unwrap here since network_interfaces would be None if the does not yet have any
                    if network_interfaces.first().unwrap().network_ip.is_none() {
                        continue;
                    }
                    break details;
                }
                _ => continue,
            }
        } else {
            continue;
        }
    };
    let internal_ip_address = details
        .network_interfaces
        .unwrap()
        .first()
        .unwrap()
        .network_ip
        .clone()
        .unwrap();
    info!("Instance internal ip: {}", internal_ip_address);
    // Simultaneously we should also probably add our ssh key to the target machine to allow for migration

    return IpAddr::V4(
        Ipv4Addr::from_str(&internal_ip_address)
            .unwrap_or_else(|_| panic!("Failed to parse ip address: {}", internal_ip_address)),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_create_instance_using_image() {
        tracing_subscriber::fmt::init();
        create_instance_with_image().await;
    }
}
