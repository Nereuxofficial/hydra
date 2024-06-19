use virt::connect::Connect;
use virt::domain::Domain;
use virt::error::Error;
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

    /// Migrate domains to a new URI. This assumes the connection will be successful, so for example
    /// for ssh our public key should be in the authorized_keys file of the remote machine as well
    /// as the remote machine's public key in our known_hosts file.
    ///
    /// We do not have access to the job stats in 0.3.1 so stdout may show more information about errors.
    pub fn migrate(&self, dst_uri: Option<String>, domains: Vec<Domain>) {
        // TODO: show information about errors
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
                println!("Domain {:?} probably migrated", dom.get_name());
                println!("Last Error: {}", Error::last_error());
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
