use crate::provider::Provider;
use aws_sdk_ec2::operation::start_instances::StartInstancesOutput;
use aws_sdk_ec2::Client;
use aws_types::SdkConfig;
use color_eyre::eyre::Result;
use std::net::IpAddr;
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
        let _res: StartInstancesOutput = self
            .client
            .start_instances()
            .instance_ids(&id)
            .send()
            .await?;

        // Wait until the instance public ip is available
        let ip_addr = loop {
            let desc = self
                .client
                .describe_instances()
                .instance_ids(&id)
                .send()
                .await?
                .reservations
                .expect("No reservations found");
            let res = desc
                .first()
                .expect("Reservations empty. This should not be the case")
                .instances
                .as_ref()
                .expect("No instances found. This should not be the case")
                .first()
                .expect("Instances empty. This should not be the case")
                .public_ip_address
                .as_ref();
            if let Some(ip) = res {
                break ip.parse().expect("Could not parse IP address");
            }
        };
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ssh::get_ssh_key_from_ip;
    use dotenvy::dotenv;
    use std::time::Instant;

    #[tokio::test]
    async fn test_instance_startup() {
        let start = Instant::now();
        dotenv().unwrap();
        let handler = AWSInstanceHandler::new().await;
        let ip_address = handler
            .start_instance_by_id("i-0ed154fb115a44566".to_string())
            .await
            .expect("Could not start other instance");
        println!("IP Address: {}", ip_address);
        get_ssh_key_from_ip(ip_address).await;
        println!(
            "Time taken to start instance: {}s",
            start.elapsed().as_secs()
        );
    }
}
