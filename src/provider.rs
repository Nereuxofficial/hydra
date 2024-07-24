use color_eyre::Result;
use std::net::IpAddr;

pub trait Provider {
    async fn start_instance(&self, id: &str) -> Result<IpAddr>;
    async fn wait_until_termination_signal(&self);
}
