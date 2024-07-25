//! Docker integration into Hydra. This module provides migration for Docker containers.
//! This is achieved by using CRIU to [checkpoint](https://github.com/docker/cli/blob/master/docs/reference/commandline/checkpoint.md) the container and then restore it on the target machine.
//! While this is not live migration per se, even live migration of VMs needs to pause the VM for a short period of time to copy the rest of the memory state

use crate::migration::Migration;
use color_eyre::eyre::Result;
use rand::Rng;
use rs_docker::Docker;
use std::net::IpAddr;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Checkpoint {
    pub checkpoint_name: String,
    pub container_id: String,
}

//TODO: COPY /var/lib/docker/containers/<CONTAINER ID>/checkpoints/ since custom dirs are not supported yet(Maybe this could also be on a networked FS?) See https://github.com/moby/moby/issues/37344
fn restore_checkpoints(checkpoints: Vec<Checkpoint>) {
    todo!()
}

pub struct DockerBackend {
    client: Docker,
    checkpoints: Vec<Checkpoint>,
}

impl DockerBackend {
    pub fn new() -> Result<Self> {
        let docker = Docker::connect(&std::env::var("DOCKER_HOST").expect(
            "DOCKER_HOST not found in environment. Please add it with a correct target to .env(Typically: DOCKER_HOST=unix:///var/run/docker.sock"),
        )
            .unwrap();
        Ok(Self {
            client: docker,
            checkpoints: vec![],
        })
    }
    pub fn checkpoint_all_containers(&mut self) -> Result<Vec<Checkpoint>> {
        let docker = &mut self.client;
        let containers = docker.get_containers(false).unwrap();
        let mut rng = rand::thread_rng();
        let results = containers
            .iter()
            .map(|container| {
                let checkpoint_name: String = rng.gen::<u64>().to_string();
                docker.create_checkpoint(
                    &container.Id,
                    &checkpoint_name,
                    None::<PathBuf>,
                    false,
                )?;
                Ok(Checkpoint {
                    checkpoint_name,
                    container_id: container.Id.clone(),
                })
            })
            .collect::<Result<Vec<Checkpoint>>>()?;
        Ok(results)
    }
}

#[async_trait::async_trait]
impl Migration for DockerBackend {
    async fn checkpoint(&mut self) -> Result<()> {
        self.checkpoints = self.checkpoint_all_containers()?;
        Ok(())
    }

    async fn migrate(&mut self, ip_addr: IpAddr) -> Result<()> {
        todo!()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use std::thread::sleep;
    use std::time::Duration;

    #[test]
    fn test_checkpoint_container() {
        let mut docker = Docker::connect("unix:///var/run/docker.sock").unwrap();
        // Execute a checkpoint-enabled container via this command: docker run -d --name looper busybox /bin/sh -c 'i=0; while true; do echo $i; i=$(expr $i + 1); sleep 1; done'
        let res = Command::new("docker")
            .arg("run")
            .arg("-d")
            .arg("--name")
            .arg("looper1812")
            .arg("busybox")
            .arg("/bin/sh")
            .arg("-c")
            .arg("i=0; while true; do echo $i; i=$(expr $i + 1); sleep 1; done")
            .output()
            .unwrap();
        println!("{:?}", res);
        sleep(Duration::from_secs(4));
        assert!(docker
            .get_containers(false)
            .is_ok_and(|containers| !containers.is_empty()));
        let mut docker_backend = DockerBackend::new().unwrap();
        let checkpoint_all_containers = docker_backend.checkpoint_all_containers().unwrap();
        println!("{:?}", checkpoint_all_containers);
        let checkpoint = checkpoint_all_containers.first().unwrap();
        docker
            .start_container(
                &checkpoint.container_id,
                Some(checkpoint.checkpoint_name.clone()),
                None,
            )
            .unwrap();

        // Cleanup container
        docker.delete_container("looper1812").unwrap();
    }
}
