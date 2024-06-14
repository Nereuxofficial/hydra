use virt::connect::Connect;
use virt::domain::Domain;
use virt::sys;

pub struct QemuConnection {
    connection: Connect,
}

impl QemuConnection {
    pub fn new() -> Self {
        let connection = Connect::open("qemu:///session").unwrap();
        QemuConnection { connection }
    }

    pub fn get_running_vms(&self) -> Vec<Domain> {
        let doman_ids = self.connection.list_domains().unwrap();
        doman_ids
            .iter()
            .filter_map(|id| Domain::lookup_by_id(&self.connection, *id).ok())
            .collect()
    }

    pub fn migrate(&self, dst_uri: Option<String>, domains: Vec<Domain>) {
        if domains.is_empty() {
            println!("No domains specified for migration");
            return;
        }
        println!(
            "Attempting to migrate domains '{:?}' to '{:?}'...",
            domains, dst_uri
        );

        for dom in domains {
            // TODO: Either use VIR_MIGRATE_TUNNELED or VIR_MIGRATE_TLS for encryption
            let flags = sys::VIR_MIGRATE_LIVE | sys::VIR_MIGRATE_PEER2PEER;
            if dom
                .migrate(&self.connection, flags, dst_uri.as_deref().unwrap(), 0)
                .is_ok()
            {
                println!("Domain {:?} migrated", dom.get_name());
                /* DOES NOT WORK IN THIS VERSION. TODO: Hide behind feature flag
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
    }
}
