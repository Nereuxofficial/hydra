use std::env;
use std::fs::read_to_string;
use std::io::Read;
use std::io::Write;
use std::net::IpAddr;

/// Adds the ssh fingerprint to known_hosts to pass the fingerprint verification securely. The new
/// instance will have the same fingerprint as ours because it is built from the machine image of
/// the current instance.
pub fn add_ssh_fingerprint_to_known_hosts(
    ip_address: IpAddr,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut known_hosts = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open(format!("{}/.ssh/known_hosts", get_home()))?;
    let mut contents = String::new();
    known_hosts.read_to_string(&mut contents)?;
    let public_key = get_ssh_key();
    let parts = public_key.split_whitespace().collect::<Vec<&str>>();
    // Since the format is different we need to convert it to the format of known_hosts
    // which is: "ip_address type public_key"
    let fingerprint = format!("{} {} {}", ip_address, parts[0], parts[1]);
    if contents.contains(&fingerprint) {
        return Ok(());
    }
    writeln!(known_hosts, "{}", fingerprint)?;
    Ok(())
}

pub fn get_ssh_key() -> String {
    let home_path = get_home();
    let paths = ["/.ssh/id_rsa.pub", "/.ssh/id_ed25519.pub"];
    paths
        .iter()
        .find_map(|path| read_to_string(format!("{home_path}/{path}")).ok())
        .unwrap_or_else(|| panic!("Failed to read ssh key"))
}

fn get_home() -> String {
    env::var("HOME").expect("HOME not found in environment. Please provide a home path")
}
