use gcloud_sdk::google_rest_apis::compute_v1;
use tracing::info;
use virt::connect::Connect;
use virt::domain::Domain;
use virt::sys;

#[tokio::main(worker_threads = 2)]
async fn main() {
    tracing_subscriber::fmt::init();
    info!("Migration starting... Requesting new machine to be started...");
    migrate(
        Some("qemu:///system".into()),
        Some("ssh+qemu://192.168.0.1/system".into()),
        "example-vm",
    )
}

async fn create_instance_using_image() {
    let project_id = gcloud_sdk::GoogleEnvironment::detect_google_project_id()
        .await
        .unwrap();
    let client = gcloud_sdk::GoogleRestApi::new().await.unwrap();
    let compute_v1_config = client.create_google_compute_v1_config().await.unwrap();
    // TODO: FInd out method to create instance using image
    // let new_instance = compute_v1::instances_api::
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
