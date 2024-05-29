use dotenvy::dotenv;
use gcloud_sdk::google_rest_apis::compute_v1::instances_api::{
    compute_instances_insert, ComputePeriodInstancesPeriodInsertParams,
};
use gcloud_sdk::google_rest_apis::compute_v1::machine_images_api::{
    compute_machine_images_list, ComputePeriodMachineImagesPeriodListParams,
};
use gcloud_sdk::google_rest_apis::compute_v1::scheduling::{
    InstanceTerminationAction, OnHostMaintenance, ProvisioningModel,
};
use gcloud_sdk::google_rest_apis::compute_v1::{Instance, Scheduling};
use rand::{thread_rng, Rng};
use tracing::info;
use virt::connect::Connect;
use virt::domain::Domain;
use virt::sys;

#[tokio::main(worker_threads = 2)]
async fn main() {
    dotenv().unwrap();
    tracing_subscriber::fmt::init();
    info!("Migration starting... Requesting new machine to be started...");
    create_instance_using_image().await;
    migrate(
        Some("qemu:///session".into()),
        Some("ssh+qemu://192.168.0.1/system".into()),
        "example-vm",
    )
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

async fn create_instance_with_image() {
    let mut rng = thread_rng();
    let zone = get_zone()
        .await
        .unwrap_or_else(|_| std::env::var("ZONE").unwrap());
    let project = gcloud_sdk::GoogleEnvironment::detect_google_project_id()
        .await
        .unwrap_or_else(|| std::env::var("GCP_PROJECT").unwrap());
    info!(
        "Creating instance in project '{}' and zone '{}'",
        project, zone
    );
    let client = gcloud_sdk::GoogleRestApi::new().await.unwrap();
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
    let machine_image = machine_images.first().unwrap();
    info!("Machine image: {:?}", machine_image);
    let instance = compute_instances_insert(
        &compute_v1_config,
        ComputePeriodInstancesPeriodInsertParams {
            project: project.clone(),
            zone,
            source_machine_image: Some(format!(
                "projects/{}/global/machineImages/{}",
                project,
                machine_image.name.as_ref().unwrap()
            )),
            instance: Some(Instance {
                name: Some(format!("nested-qemu-{}", rng.gen::<u32>())),
                scheduling: Some(Box::new(Scheduling {
                    preemptible: Some(false),
                    provisioning_model: Some(ProvisioningModel::Standard),
                    on_host_maintenance: Some(OnHostMaintenance::Terminate),
                    automatic_restart: Some(false),
                    instance_termination_action: None,
                    ..Default::default()
                })),
                ..Default::default()
            }),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    info!("Instance created: {:?}", instance);
}

fn migrate(src_uri: Option<String>, dst_uri: Option<String>, dname: &str) {
    println!(
        "Attempting to migrate domain '{}' from '{:?}' to '{:?}'...",
        dname, src_uri, dst_uri
    );

    let mut conn = match Connect::open(src_uri.as_deref()) {
        Ok(c) => c,
        Err(e) => panic!("No connection to source hypervisor: {}", e),
    };

    if let Ok(dom) = Domain::lookup_by_name(&conn, &dname) {
        let flags = sys::VIR_MIGRATE_LIVE | sys::VIR_MIGRATE_PEER2PEER | sys::VIR_MIGRATE_TUNNELLED;
        if dom
            .migrate(&conn, flags, None, dst_uri.as_deref(), 0)
            .is_ok()
        {
            println!("Domain migrated");

            if let Ok(job_stats) = dom.get_job_stats(sys::VIR_DOMAIN_JOB_STATS_COMPLETED) {
                println!(
                    "Migration completed in {}ms",
                    job_stats
                        .time_elapsed
                        .map(|time| time.to_string())
                        .unwrap_or("?".into())
                );
            }
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
        create_instance_using_image().await;
    }
}
