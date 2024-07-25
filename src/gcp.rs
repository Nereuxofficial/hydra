use crate::ssh::get_ssh_key;
use gcloud_sdk::google_rest_apis::compute_v1::configuration::Configuration;
use gcloud_sdk::google_rest_apis::compute_v1::instances_api::{
    compute_instances_get, compute_instances_insert, ComputePeriodInstancesPeriodGetParams,
    ComputePeriodInstancesPeriodInsertParams,
};
use gcloud_sdk::google_rest_apis::compute_v1::machine_images_api::{
    compute_machine_images_list, ComputePeriodMachineImagesPeriodListParams,
};
use gcloud_sdk::google_rest_apis::compute_v1::scheduling::ProvisioningModel;
use gcloud_sdk::google_rest_apis::compute_v1::{
    AttachedDisk, AttachedDiskInitializeParams, Instance, Metadata, Scheduling,
};
use gcloud_sdk::{TokenSourceType, GCP_DEFAULT_SCOPES};
use rand::{thread_rng, Rng};
use std::env;
use std::fs::read_to_string;
use std::net::{IpAddr, Ipv4Addr};
use std::str::FromStr;
use tracing::info;

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

async fn get_machine_type() -> Result<String, reqwest::Error> {
    let client = reqwest::Client::new();
    let response = client
        .get("http://metadata.google.internal/computeMetadata/v1/instance/machine-type")
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
pub async fn create_instance_with_image() -> IpAddr {
    let mut rng = thread_rng();
    let machine_type_job = tokio::spawn(async { get_machine_type().await });
    let zone_job = tokio::spawn(async {
        get_zone()
            .await
            .unwrap_or_else(|_| env::var("ZONE").unwrap())
    });
    let project = gcloud_sdk::GoogleEnvironment::detect_google_project_id()
        .await
        .unwrap_or_else(|| env::var("GCP_PROJECT").unwrap());
    let client = gcloud_sdk::GoogleRestApi::with_token_source(
        TokenSourceType::Json(read_to_string(".key.json").unwrap()),
        GCP_DEFAULT_SCOPES.clone(),
    )
    .await
    .unwrap();
    let compute_v1_config = client.create_google_compute_v1_config().await.unwrap();

    let machine_images = compute_machine_images_list(
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
    let machine_image = machine_images
        .into_iter()
        .find(|i| i.name == Some(env::var("MACHINE_IMAGE_NAME").unwrap()))
        .expect("Image not found");

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
    metadata_items.iter_mut().for_each(|item| {
        if item.key.as_ref().unwrap() == "ssh-keys" {
            let mut value = item.value.as_ref().unwrap().clone();
            value.push('\n');
            value.push_str(get_ssh_key().as_str());
            item.value = Some(value);
        }
    });

    let name = format!("nested-qemu-{}", rng.gen::<u32>());
    let zone = zone_job.await.unwrap();
    let operation = compute_instances_insert(
        &compute_v1_config,
        ComputePeriodInstancesPeriodInsertParams {
            project: project.clone(),
            zone: zone.clone(),
            instance: Some(Instance {
                name: Some(name.clone()),
                disks: Some(vec![AttachedDisk {
                    boot: Some(true),
                    auto_delete: Some(true),
                    initialize_params: Some(Box::new(AttachedDiskInitializeParams {
                        source_image: Some(format!(
                            "projects/{}/global/images/{}",
                            project.clone(),
                            env::var("SOURCE_IMAGE").unwrap()
                        )),
                        ..Default::default()
                    })),
                    ..Default::default()
                }]),
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
                machine_type: Some(
                    format!(
                        "projects/{}/zones/{}/machineTypes/{}",
                        project.clone(),
                        zone.clone(),
                        machine_type_job.await.unwrap().unwrap()
                    )
                    .to_string(),
                ),
                network_interfaces: Some(vec![Default::default()]),
                ..Default::default()
            }),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    info!("Instance created: {:?}", operation);

    get_ip_addr_of_instance(&compute_v1_config, name.clone(), zone, project).await
}

async fn get_ip_addr_of_instance(
    compute_v1_config: &Configuration,
    instance_name: String,
    zone: String,
    project: String,
) -> IpAddr {
    // Maybe we could also give back the instance
    let details = loop {
        let res = compute_instances_get(
            compute_v1_config,
            ComputePeriodInstancesPeriodGetParams {
                project: project.clone(),
                zone: zone.clone(),
                instance: instance_name.clone(),
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

    IpAddr::V4(
        Ipv4Addr::from_str(&internal_ip_address)
            .unwrap_or_else(|_| panic!("Failed to parse ip address: {}", internal_ip_address)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ssh::get_ssh_key_from_ip;
    use crate::Instant;
    use dotenvy::dotenv;
    use gcloud_sdk::google_rest_apis::compute_v1::instances_api::ComputePeriodInstancesPeriodStartParams;
    use reqwest::header::HeaderMap;

    #[tokio::test]
    async fn test_create_instance_using_image() {
        tracing_subscriber::fmt::init();
        create_instance_with_image().await;
    }

    #[tokio::test]
    async fn test_get_prices_gcp() {
        tracing_subscriber::fmt::init();
        dotenv().unwrap();
        let ic_api_key = std::env::var("INFRACOST_API_KEY").unwrap();
        let zone = env::var("ZONE").unwrap();
        let client = reqwest::Client::new();
        let mut headers = HeaderMap::new();
        headers.insert("X-Api-Key", ic_api_key.parse().unwrap());
        headers.insert("content-type", "application/json".parse().unwrap());
        let res = client.post("https://pricing.api.infracost.io/graphql")
                .headers(headers)
                .body("{\"operationName\":null,\"variables\":{},\"query\":\"{\\n  products(\\n    filter: {vendorName: \\\"gcp\\\", service: \\\"Compute Engine\\\", productFamily: \\\"Compute Instance\\\", region: \\\"europe-west1\\\"}\\n  ) {\\n    region\\n    attributes {\\n      key\\n      value\\n    }\\n    sku\\n    prices(filter: {purchaseOption: \\\"on_demand\\\"}) {\\n      EUR\\n      purchaseOption\\n    }\\n  }\\n}\\n\"}")
                .send().await.unwrap()
                .text().await.unwrap();
        println!("{}", res);
    }

    #[tokio::test]
    async fn start_gcp_instance() {
        dotenvy::dotenv().ok();
        let instance_name = "qemu-testing-instance-ssd";
        let zone = env::var("ZONE").unwrap();
        let start = Instant::now();
        let project = gcloud_sdk::GoogleEnvironment::detect_google_project_id()
            .await
            .unwrap_or_else(|| env::var("GCP_PROJECT").unwrap());
        let client = gcloud_sdk::GoogleRestApi::with_token_source(
            TokenSourceType::Json(read_to_string(".key.json").unwrap()),
            GCP_DEFAULT_SCOPES.clone(),
        )
        .await
        .unwrap();
        let compute_v1_config = client.create_google_compute_v1_config().await.unwrap();
        let instance =
            gcloud_sdk::google_rest_apis::compute_v1::instances_api::compute_instances_start(
                &compute_v1_config,
                ComputePeriodInstancesPeriodStartParams {
                    project: project.clone(),
                    zone: zone.clone(),
                    instance: instance_name.to_string(),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        println!("{:?}", instance);
        let ip_addr =
            get_ip_addr_of_instance(&compute_v1_config, instance_name.to_string(), zone, project)
                .await;
        get_ssh_key_from_ip(ip_addr).await;
        println!("Time taken: {}s", start.elapsed().as_secs());
    }
}
