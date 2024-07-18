use aws_sdk_ec2::operation::start_instances::StartInstancesOutput;
use aws_sdk_ec2::Client;
use aws_types::SdkConfig;
use color_eyre::eyre::Result;
use std::net::IpAddr;
use std::time::Duration;

use crate::instances::InstanceHandler;

struct AWSInstanceHandler {
    client: Client,
}

pub async fn start_instance(client: &Client, id: &str) -> Result<IpAddr> {
    // start_instance has no unique errors to handle.
    let res: StartInstancesOutput = client.start_instances().instance_ids(id).send().await?;

    let ip_addr: IpAddr = client
        .describe_instances()
        .instance_ids(id)
        .send()
        .await?
        .reservations()[0]
        .instances()[0]
        .network_interfaces()
        .iter()
        .next()
        .unwrap()
        .private_ip_address()
        .expect("No private IP address found")
        .parse()
        .unwrap();
    println!("Starting instance with IP: {}", ip_addr);

    Ok(ip_addr)
}

impl AWSInstanceHandler {
    pub async fn new() -> Self {
        let sdk_config: SdkConfig = aws_config::load_from_env().await;
        let client = Client::new(&sdk_config);
        Self { client }
    }
}

impl InstanceHandler for AWSInstanceHandler {
    async fn start_instance(&self, id: &str) -> Result<IpAddr> {
        start_instance(&self.client, id).await
    }
}
