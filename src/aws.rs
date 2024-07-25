use crate::provider::Provider;
use aws_sdk_ec2::operation::start_instances::StartInstancesOutput;
use aws_sdk_ec2::Client;
use aws_types::SdkConfig;
use color_eyre::eyre::Result;
use futures::future::BoxFuture;
use std::net::IpAddr;
use std::pin::Pin;
use std::time::Duration;

pub struct AWSInstanceHandler {
    client: Client,
}

impl AWSInstanceHandler {
    pub async fn new() -> Self {
        let sdk_config: SdkConfig = aws_config::load_from_env().await;
        let client = Client::new(&sdk_config);
        Self { client }
    }
    pub async fn start_instance_by_id(&self, id: String) -> Result<IpAddr> {
        // start_instance has no unique errors to handle.
        let res: StartInstancesOutput = self
            .client
            .start_instances()
            .instance_ids(&id)
            .send()
            .await?;

        let ip_addr: IpAddr = self
            .client
            .describe_instances()
            .instance_ids(&id)
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
}

#[async_trait::async_trait]
impl Provider for AWSInstanceHandler {
    async fn start_instance(&self, id: String) -> Result<IpAddr> {
        self.start_instance_by_id(id).await
    }

    async fn wait_until_termination_signal(&self) -> Result<Duration> {
        wait_until_termination_notice().await
    }
}

/// Executes the callback upon receiving a termination notice in an AWS environment. See https://aws.amazon.com/blogs/aws/new-ec2-spot-instance-termination-notices/
/// Basically repeatedly requests http://169.254.169.254/latest/meta-data/spot/termination-time until it is available and then returns the Duration left till termination
pub async fn wait_until_termination_notice() -> Result<Duration> {
    let client = reqwest::Client::new();
    let mut interval = tokio::time::interval(Duration::from_secs(2));
    loop {
        let res = client
            .get("http://169.254.169.254/latest/meta-data/spot/termination-time")
            .send()
            .await
            .unwrap();
        let status = res.status();
        println!("Termination Response: {}", status);
        println!("Termination Response Body: {}", res.text().await.unwrap());
        if status.is_success() {
            // TODO: Deserialize response to get termination countdown time. See https://docs.aws.amazon.com/AWSEC2/latest/UserGuide/spot-instance-termination-notices.html
            return Ok(Duration::from_secs(197));
        }
        // The requests should be every two seconds so we in the worst case we probably have 197 seconds to react to the termination notice.
        interval.tick().await;
    }
}
