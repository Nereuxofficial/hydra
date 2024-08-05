use async_trait::async_trait;
use russh::client::Msg;
use russh::{client, Channel, ChannelMsg};
use russh_keys::key::PublicKey;
use russh_keys::{load_secret_key, PublicKeyBase64};
use std::env;
use std::fs::read_to_string;
use std::io::Read;
use std::io::Write;
use std::net::IpAddr;
use std::sync::{Arc, Mutex, OnceLock};
use tokio::io::AsyncWriteExt;

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

#[derive(Clone)]
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

/// Creates an ssh channel.
/// *WARNING*: Will loop infinitely until a connection is established.
pub async fn get_ssh_session(ip_addr: &IpAddr) -> color_eyre::Result<Channel<Msg>> {
    loop {
        let res: color_eyre::Result<Channel<Msg>> = {
            let key_pair = Arc::new(
                get_key_paths()
                    .into_iter()
                    // It is expected that the key has no password. TODO: Allow passing a password
                    .find_map(|p| load_secret_key(p, None).ok())
                    .unwrap(),
            );
            let config = client::Config {
                inactivity_timeout: None,
                ..Default::default()
            };
            let client = Arc::new(Mutex::new(Client {
                public_key: OnceLock::new(),
            }));
            let shared_client = SharedClient(client.clone());
            println!("Trying to connect to the new instance...");
            let shared_config = Arc::new(config);

            let try_connect =
                || client::connect(shared_config.clone(), (*ip_addr, 22), shared_client.clone());
            let mut session = loop {
                match try_connect().await {
                    Ok(s) => break s,
                    Err(e) => {
                        println!("Error connecting to the new instance: {:?}", e);
                    }
                }
            };
            let res = session
                .authenticate_publickey(env::var("USER").unwrap(), key_pair.clone())
                .await
                .unwrap();
            if !res {
                continue;
            }
            let channel = session.channel_open_session().await?;
            println!("Connected to the new instance");
            Ok(channel)
        };
        if let Ok(channel) = res {
            return Ok(channel);
        }
    }
}

/// Gets the public ssh key from a new instance and adds it to known_hosts
pub async fn add_ssh_pubkey_from_ip(ip_addr: IpAddr) {
    let key_pair = get_key_paths()
        .into_iter()
        // It is expected that the key has no password. TODO: Allow passing a password
        .find_map(|p| load_secret_key(p, None).ok())
        .unwrap();
    let config = client::Config {
        inactivity_timeout: None,
        ..Default::default()
    };
    let client = Arc::new(Mutex::new(Client {
        public_key: OnceLock::new(),
    }));
    let shared_client = SharedClient(client.clone());
    println!("Trying to connect to the new instance...");
    let shared_config = Arc::new(config);
    let try_connect =
        || client::connect(shared_config.clone(), (ip_addr, 22), shared_client.clone());
    let mut session = loop {
        match try_connect().await {
            Ok(s) => break s,
            Err(e) => {
                println!("Error connecting to the new instance: {:?}", e);
            }
        }
    };
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
pub async fn call(session: &mut Channel<Msg>, command: &str) -> color_eyre::Result<u32> {
    session.exec(true, command).await?;

    let mut code = None;
    let mut stdout = tokio::io::stdout();

    loop {
        // There's an event available on the session channel
        let Some(msg) = session.wait().await else {
            break;
        };
        match msg {
            // Write data to the terminal
            ChannelMsg::Data { ref data } => {
                stdout.write_all(data).await?;
                stdout.flush().await?;
            }
            // The command has returned an exit code
            ChannelMsg::ExitStatus { exit_status } => {
                code = Some(exit_status);
                // cannot leave the loop immediately, there might still be more data to receive
            }
            _ => {}
        }
    }
    Ok(code.expect("program did not exit cleanly"))
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
