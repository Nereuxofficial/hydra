//! Docker integration into Hydra. This module provides migration for Docker containers.
//! This is achieved by using CRIU to [checkpoint](https://github.com/docker/cli/blob/master/docs/reference/commandline/checkpoint.md) the container and then restore it on the target machine.
//! While this is not live migration per se, even live migration of VMs needs to pause the VM for a short period of time to copy the rest of the memory state
use color_eyre::eyre::Result;
use rs_docker::Docker;

/// Checkpoint a container. This needs to use the docker cli because checkpointing is an experimental feature and not available in the docker sdk
pub fn checkpoint_container() -> Result<()> {
    // Use DOCKER_HOST, since we want to support user-mode docker too
    let mut docker = Docker::connect(&std::env::var("DOCKER_HOST")?)?;
    let containers = docker.get_containers(false)?;
    // TODO: Mechanism to make individual checkpoints
    containers
        .iter()
        .try_for_each(|c| {
            docker.create_checkpoint(&c.Id, "c1", std::env::var("CHECKPOINT_DIR").ok(), false)
        })
        .unwrap();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_checkpoint_container() {
        assert!(checkpoint_container().is_ok());
    }
}
