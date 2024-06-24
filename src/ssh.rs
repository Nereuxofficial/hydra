use async_trait::async_trait;
use russh::client;
use russh_keys::key::PublicKey;
use russh_keys::{load_secret_key, PublicKeyBase64};
use std::env;
use std::fs::read_to_string;
use std::io::Read;
use std::io::Write;
use std::net::IpAddr;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

/// Adds the ssh fingerprint to known_hosts to pass the fingerprint verification securely. The new
/// instance will have the same fingerprint as ours because it is built from the machine image of
/// the current instance.
#[allow(unused)]
pub fn add_ssh_fingerprint_to_known_hosts(
    ip_address: IpAddr,
    public_key: PublicKey,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut known_hosts = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open(format!("{}/.ssh/known_hosts", get_home()))?;
    let mut contents = String::new();
    known_hosts.read_to_string(&mut contents)?;

    // Since the format is different we need to convert it to the format of known_hosts
    // which is: "ip_address type public_key"
    let fingerprint = format!(
        "{} {} {}",
        ip_address,
        public_key.name(),
        public_key.public_key_base64()
    );
    if contents.contains(&fingerprint) {
        return Ok(());
    }
    writeln!(known_hosts, "{}", fingerprint)?;
    Ok(())
}

pub fn get_ssh_key() -> String {
    let paths = get_key_paths()
        .iter()
        .map(|path| format!("{}.pub", path))
        .collect::<Vec<String>>();
    paths
        .iter()
        .find_map(|path| read_to_string(path).ok())
        .unwrap_or_else(|| panic!("Failed to read ssh key"))
}
/// Returns possible private ssh key paths
fn get_key_paths() -> Vec<String> {
    let home_path = get_home();
    ["/.ssh/id_rsa", "/.ssh/id_ed25519"]
        .map(|path| format!("{home_path}{path}"))
        .into_iter()
        .collect()
}

fn get_home() -> String {
    env::var("HOME").expect("HOME not found in environment. Please provide a home path")
}

struct SharedClient(Arc<Mutex<Client>>);

struct Client {
    public_key: OnceLock<PublicKey>,
}

#[async_trait]
impl client::Handler for SharedClient {
    type Error = russh::Error;

    async fn check_server_key(&mut self, public_key: &PublicKey) -> Result<bool, Self::Error> {
        self.0
            .lock()
            .unwrap()
            .public_key
            .set(public_key.clone())
            .unwrap();
        Ok(true)
    }
}

pub async fn get_ssh_key_from_ip(ip_addr: IpAddr) {
    let key_pair = get_key_paths()
        .into_iter()
        // It is expected that the key has no password. TODO: Allow passing a password
        .find_map(|p| load_secret_key(p, None).ok())
        .unwrap();
    let config = client::Config {
        // The RTT in a datacenter should be relatively short
        inactivity_timeout: None,
        ..Default::default()
    };
    let client = Arc::new(Mutex::new(Client {
        public_key: OnceLock::new(),
    }));
    let shared_client = SharedClient(client.clone());
    println!("Trying to connect to the new instance...");
    let mut session = client::connect(Arc::new(config), (ip_addr, 22), shared_client)
        .await
        .unwrap();
    session
        .authenticate_publickey(env::var("USER").unwrap(), Arc::new(key_pair))
        .await
        .unwrap();
    add_ssh_fingerprint_to_known_hosts(
        ip_addr,
        client.lock().unwrap().public_key.get().unwrap().clone(),
    )
    .unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;

    #[test]
    fn get_secret_key() {
        let key_paths = get_key_paths();
        for path in key_paths {
            match load_secret_key(path, None) {
                Ok(p) => return,
                Err(e) => println!("{:?}", e),
            }
        }
        panic!("No key found");
    }

    #[test]
    fn test_get_key_paths() {
        let paths = get_key_paths();
        println!("{:?}", paths);
        assert!(paths.iter().any(|p| File::open(p).is_ok()));
    }
}
