use crate::provider::Provider;
use color_eyre::Result;
use std::net::IpAddr;
use std::time::Duration;

pub struct MockTermination<C: Provider> {
    provider: C,
}

impl<C: Provider> MockTermination<C> {
    pub fn new(provider: C) -> Self {
        Self { provider }
    }
}

#[async_trait::async_trait]
impl<C> Provider for MockTermination<C>
where
    C: Provider + Sync + Send,
{
    async fn start_instance(&self, id: String) -> Result<IpAddr> {
        self.provider.start_instance(id).await
    }

    async fn wait_until_termination_signal(&self) -> Result<Duration> {
        println!("Mocking termination signal... 117 seconds till termination");
        Ok(Duration::from_secs(117))
    }
}
