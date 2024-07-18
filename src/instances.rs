use color_eyre::eyre::Result;
use std::net::IpAddr;

pub trait InstanceHandler {
    async fn start_instance(&self, id: &str) -> Result<IpAddr>;
}
