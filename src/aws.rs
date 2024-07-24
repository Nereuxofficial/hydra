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

impl Provider for AWSInstanceHandler {
    async fn start_instance(&self, id: &str) -> Result<IpAddr> {
        start_instance(&self.client, id).await
    }

    async fn wait_until_termination_signal(&self) {
        execute_upon_termination_notice(&|dur| {
            println!("Termination note received. About {}s left", dur.as_secs());
            Ok(())
        })
        .await;
    }
}

/// Executes the callback upon receiving a termination notice in an AWS environment. See https://aws.amazon.com/blogs/aws/new-ec2-spot-instance-termination-notices/
/// Basically repeatedly requests http://169.254.169.254/latest/meta-data/spot/termination-time until it is available and then calls the callback.
pub async fn execute_upon_termination_notice(callback: &dyn Fn(Duration) -> Result<()>) {
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
            let time_left = Duration::from_secs(197);
            callback(time_left).unwrap();
            break;
        }
        // The requests should be every two seconds so we in the worst case we probably have 197 seconds to react to the termination notice.
        interval.tick().await;
    }
}
