use dotenvy::dotenv;
use gcloud_sdk::google_rest_apis::compute_v1::configuration::Configuration;
use gcloud_sdk::google_rest_apis::compute_v1::instances_api::{
    compute_instances_get, compute_instances_insert, compute_instances_set_metadata,
    ComputePeriodInstancesPeriodGetParams, ComputePeriodInstancesPeriodInsertParams,
};
use gcloud_sdk::google_rest_apis::compute_v1::machine_images_api::{
    compute_machine_images_list, ComputePeriodMachineImagesPeriodListParams,
};
use gcloud_sdk::google_rest_apis::compute_v1::scheduling::{
    InstanceTerminationAction, OnHostMaintenance, ProvisioningModel,
};
use gcloud_sdk::google_rest_apis::compute_v1::{Instance, MetadataItemsInner, Scheduling};
use gcloud_sdk::GoogleRestApi;
use rand::{thread_rng, Rng};
use std::fmt::Debug;
use std::fs::read_to_string;
use std::net::{IpAddr, Ipv4Addr};
use std::str::FromStr;
use std::time::Instant;
use tracing::info;
use virt::connect::Connect;
use virt::domain::Domain;
use virt::sys;

#[tokio::main(worker_threads = 2)]
async fn main() {
    dotenv().unwrap();
    tracing_subscriber::fmt::init();
    info!("Migration starting... Requesting new machine to be started...");
    let start = Instant::now();
    let ip_address = create_instance_with_image().await;
    migrate(
        Some("qemu:///session".into()),
        Some(format!("ssh+qemu://{}/session", ip_address)),
        "example-vm",
    );
    let duration = start.elapsed();
    info!(
        "Migration completed in {:?}. Time left: {} seconds",
        duration,
        30 - duration.as_secs()
    );
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
    let client = gcloud_sdk::GoogleRestApi::new().await.unwrap();
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
    let mut machine_image = machine_images.first_mut().unwrap();
    info!("Machine image: {:?}", machine_image.name.clone());
    // Edit the metadata to add our machine's ssh public key while preserving the previous ssh keys
    let mut metadata_items = machine_image
        .instance_properties
        .as_mut()
        .unwrap()
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

fn get_ssh_key() -> String {
    let home_path =
        std::env::var("HOME").expect("HOME not found in environment. Please provide a home path");
    let paths = ["/.ssh/id_rsa.pub", "/.ssh/id_ed25519.pub"];
    paths
        .iter()
        .find_map(|path| read_to_string(format!("{home_path}/{path}")).ok())
        .unwrap_or_else(|| panic!("Failed to read ssh key"))
}

fn migrate(src_uri: Option<String>, dst_uri: Option<String>, dname: &str) {
    println!(
        "Attempting to migrate domain '{}' from '{:?}' to '{:?}'...",
        dname, src_uri, dst_uri
    );

    let mut conn = match Connect::open(src_uri.as_deref().unwrap()) {
        Ok(c) => c,
        Err(e) => panic!("No connection to source hypervisor: {}", e),
    };

    if let Ok(dom) = Domain::lookup_by_name(&conn, &dname) {
        let flags = sys::VIR_MIGRATE_LIVE | sys::VIR_MIGRATE_PEER2PEER | sys::VIR_MIGRATE_TUNNELLED;
        if dom
            .migrate(&conn, flags, dst_uri.as_deref().unwrap(), 0)
            .is_ok()
        {
            println!("Domain migrated");
            /*
            if let Ok(job_stats) = dom.get_job_stats(sys::VIR_DOMAIN_JOB_STATS_COMPLETED) {
                println!(
                    "Migration completed in {}ms",
                    job_stats
                        .time_elapsed
                        .map(|time| time.to_string())
                        .unwrap_or("?".into())
                );
            }
            */
        }
    }

    if let Err(e) = conn.close() {
        panic!("Failed to disconnect from hypervisor: {}", e);
    }
    println!("Disconnected from source hypervisor");
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
