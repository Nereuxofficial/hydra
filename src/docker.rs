//! Docker integration into Hydra. This module provides migration for Docker containers.
//! This is achieved by using CRIU to [checkpoint](https://github.com/docker/cli/blob/master/docs/reference/commandline/checkpoint.md) the container and then restore it on the target machine.
//! While this is not live migration per se, even live migration of VMs needs to pause the VM for a short period of time to copy the rest of the memory state

use crate::migration::Migration;
use crate::ssh::{call, get_ssh_session};
use crate::zip::zip_dir;
use bollard::container::{Config, CreateContainerOptions, ListContainersOptions};
use bollard::image::{CommitContainerOptions, ImportImageOptions};
use bollard::secret::Commit;
use color_eyre::eyre::Result as EyreResult;
use color_eyre::owo_colors::OwoColorize;
use futures::future::join_all;
use gen_passphrase::dictionary::EFF_SHORT_2;
use rand::Rng;
use rs_docker::Docker;
use russh_sftp::client::SftpSession;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::{read_dir, ReadDir};
use std::net::IpAddr;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::{fs, thread};
use tokio::io::AsyncWriteExt;
use tokio::task::JoinError;
use tokio_stream::StreamExt;
use tokio_util::codec;
use tracing::debug;
use tracing::info;
use tracing_subscriber::fmt::format;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Checkpoint {
    pub checkpoint_name: String,
    pub container_id: String,
}

//TODO: COPY /var/lib/docker/containers/<CONTAINER ID>/checkpoints/ since custom dirs are not supported yet(Maybe this could also be on a networked FS?) See https://github.com/moby/moby/issues/37344

pub struct DockerBackend {
    client: Docker,
    /// This is sadly needed because the forked version of rs-docker doees not support modern docker APIs, however bollard does not support checkpoints
    async_client: bollard::Docker,
    checkpoints: Vec<Checkpoint>,
    container_names: BTreeMap<OldContainerName, NewContainerName>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Ord, Eq, PartialEq, PartialOrd)]
struct OldContainerName(String);
#[derive(Debug, Clone, Serialize, Deserialize, Ord, Eq, PartialEq, PartialOrd)]
struct NewContainerName(String);

impl DockerBackend {
    pub fn new() -> EyreResult<Self> {
        let docker = Docker::connect(&std::env::var("DOCKER_HOST").expect(
            "DOCKER_HOST not found in environment. Please add it with a correct target to .env(Typically: DOCKER_HOST=unix:///var/run/docker.sock"),
        )
            .unwrap();
        Ok(Self {
            client: docker,
            checkpoints: vec![],
            async_client: bollard::Docker::connect_with_local_defaults().unwrap(),
            container_names: BTreeMap::new(),
        })
    }
    pub async fn checkpoint_all_containers(&mut self) -> EyreResult<Vec<Checkpoint>> {
        let docker = &mut self.client;
        // TODO: Do this via an Atomicptr
        // Workaround for spawning a seconds tokio runtime since rs-docker spawns a tokio runtime internally
        let results = Arc::new(Mutex::new(vec![]));
        std::thread::scope(|s| {
            let a = results.clone();
            s.spawn(move || {
                let containers = docker.get_containers(false).unwrap();
                let mut rng = rand::thread_rng();
                a.lock().unwrap().append(
                    &mut containers
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
                        .collect::<EyreResult<Vec<Checkpoint>>>()
                        .unwrap(),
                );
            })
            .join()
            .unwrap();
        });
        let cloned_res = results.lock().unwrap().clone();
        Ok(cloned_res)
    }

    // Exports all containers by committing them and saving them to a tarball
    async fn export_all_containers(&mut self) -> EyreResult<()> {
        let running_containers = self
            .async_client
            .list_containers(None::<ListContainersOptions<&str>>)
            .await?;
        // TODO: Should we even commit running containers if we can just export them?
        let futures = running_containers.into_iter().map(|c| {
            let client = self.async_client.clone();
            async move {
                Ok((
                    c.id.clone().unwrap(),
                    client
                        .commit_container(
                            CommitContainerOptions {
                                container: c.id.clone().unwrap(),
                                repo: gen_passphrase::generate(&[EFF_SHORT_2], 2, None),
                                tag: "checkpointedlatest".into(),
                                comment: "Autocheckpointed by hydra".into(),
                                author: "hydra".into(),
                                pause: false,
                                changes: None,
                            },
                            // TODO: Copy the config of the running container
                            Config::<String>::default(),
                        )
                        .await?,
                ))
            }
        });
        let commits = join_all(futures)
            .await
            .into_iter()
            .collect::<Result<Vec<(String, Commit)>, bollard::errors::Error>>()?;
        // Create the containers directory if it does not exist
        tokio::fs::create_dir_all("containers").await?;
        let mut tasks = vec![];
        // Paralellise for each container
        for (container_id, commit) in commits {
            println!("Exporting container: {:?}", commit);
            let acc = self.async_client.clone();
            tasks.push(tokio::spawn(async move {
                let mut stream = acc.export_container(&container_id);
                let mut data = vec![];
                while let Some(chunk) = stream.next().await {
                    let mut bytes_vec: Vec<u8> = chunk.unwrap().to_vec();
                    data.append(&mut bytes_vec);
                }
                info!("Container size: {}", data.len());
                // TODO: If we do not write the containers to disk we could write the data to the remote server, however it would hamper the ability to checkpoint the containers
                tokio::fs::write(format!("containers/{}.tar.gz", container_id), data)
                    .await
                    .unwrap();
            }));
        }
        join_all(tasks).await;

        Ok(())
    }

    async fn transfer_containers(&mut self, ip_addr: IpAddr) -> EyreResult<()> {
        let ssh_session = get_ssh_session(&ip_addr).await?;
        ssh_session.request_subsystem(true, "sftp").await?;
        let sftp = SftpSession::new(ssh_session.into_stream()).await?;
        let container_files = fs::read_dir("containers")?;
        let _res = sftp.create_dir("containers").await;
        debug!("Result of creating the containers dir remotely: {_res:?}");
        for container_file in container_files {
            let container_file = container_file?;
            let path = container_file.path();
            let container_id = path.file_name().unwrap().to_str().unwrap();
            let mut remote_file = sftp.create(format!("containers/{}", container_id)).await?;
            remote_file.write_all(&tokio::fs::read(path).await?).await?;
        }
        Ok(())
    }
    /// Restores all containers from their respective tarballs.
    /// This assumes that the and *only* tarballs are in the containers directory relatively to the current directory
    /// This also assumes that the tarballs are named <container_id>.tar
    ///
    /// This also creates a Map of the old container id to the new container id to allow for migration
    pub async fn import_containers(&mut self) -> EyreResult<()> {
        let container_files = fs::read_dir("containers")?;
        let image_ids = self.import_via_command(container_files).await?;
        info!("Imported containers: {:?}", image_ids);
        for (container_id, image_id) in image_ids {
            let options = Some(CreateContainerOptions {
                name: container_id.clone(),
                platform: None,
            });
            let config = Config {
                image: Some(image_id.clone()),
                cmd: Some(vec![
                    "/bin/sh".to_string(),
                    "-c".to_string(),
                    "sleep".to_string(),
                    "10".to_string(),
                ]),
                ..Default::default()
            };
            let container = self.async_client.create_container(options, config).await?;
            // Should not panic since container IDs are unique
            self.container_names.insert(
                OldContainerName(container_id.split('.').next().unwrap().to_string()),
                NewContainerName(container.id),
            );
        }

        Ok(())
    }

    /// This imports the containers via the command line due to a bug in the API
    async fn import_via_command(&self, dir: ReadDir) -> EyreResult<Vec<(String, String)>> {
        let mut collection = vec![];
        for file in dir {
            let file = file.unwrap();
            let path = file.path();
            let container_id = path.file_stem().unwrap().to_str().unwrap().to_string();
            let image_id = Command::new("docker").arg("import").arg(path).output()?;
            let image_id = String::from_utf8(image_id.stdout)?
                .trim_matches('\n')
                .replace("sha256:", "");
            collection.push((container_id, image_id));
        }
        Ok(collection)
    }

    #[allow(unused)]
    // TODO: Switch to API when the API is fixed
    async fn import_via_api(&self, dir: ReadDir) -> EyreResult<Vec<(String, String)>> {
        let mut tasks = vec![];
        for container_file in dir {
            let container_file = container_file.unwrap();
            let path = container_file.path();
            let container_id = path
                .file_stem()
                .unwrap()
                .to_str()
                .unwrap()
                .to_string()
                .clone();
            let client_ref = self.async_client.clone();
            tasks.push(tokio::spawn(async move {
                let image_id = load_container_from_file(&client_ref, &path).await.unwrap();
                Ok((container_id, image_id))
            }));
        }
        join_all(tasks)
            .await
            .into_iter()
            .collect::<Result<EyreResult<Vec<_>>, JoinError>>()?
    }

    /// Broadly the restoration of the containers can be split into the following two steps:
    /// 1. Copy the checkpoint files to the target machine
    /// 2. Restore the containers on the target machine using either their docker socket or a cli command
    async fn migrate_all_containers(&mut self, ip_addr: IpAddr) -> EyreResult<()> {
        self.checkpoints = self.checkpoint_all_containers().await?;
        // Connect to the other machine via ssh and continue our checkpoints there
        let ssh_session = get_ssh_session(&ip_addr).await?;
        ssh_session.request_subsystem(true, "sftp").await?;
        let sftp = SftpSession::new(ssh_session.into_stream()).await.unwrap();
        // Annoyingly we need root to access these files at least on the remote machine. See https://github.com/moby/moby/issues/37344
        // This is really annoying especially as the issue has been open for 4 years now
        // Zip the directories in /var/lib/docker/containers/ migrate them and unzip them on the target machine
        // TODO: Maybe we can directly stream the file to the remote machine
        let src_dir = "/var/lib/docker/containers";
        // FIXME: Transfer only checkpoints, not other files in the containers directory
        let dest_file = "./containers.zip";
        zip_dir(src_dir, dest_file)?;
        let mut remote_file = sftp.create(dest_file).await?;
        let mut local_file = tokio::fs::File::open(dest_file).await?;
        tokio::io::copy(&mut local_file, &mut remote_file).await?;
        remote_file.flush().await?;
        let mut new_ssh_session = get_ssh_session(&ip_addr).await?;
        let command = &format!(
            "nohup sudo RUST_BACKTRACE=1 RUST_LOG=info DOCKER_HOST=unix:///var/run/docker.sock ./hydra restore {dest_file} '{}' > output.txt & disown",
            serde_json::to_string(&self.checkpoints).unwrap()
        );
        println!("Running command: {}", command);
        let res = call(&mut new_ssh_session, command).await?;
        println!("Got response to starting hydra on remote: {}", res);
        Ok(())
    }

    pub async fn restore_containers(
        &mut self,
        container_archive: &Path,
        dest: &Path,
        containers: Vec<Checkpoint>,
    ) -> EyreResult<()> {
        thread::scope(|s| {
            s.spawn(move || {
                let file = fs::File::open(container_archive).unwrap();
                let mut archive = zip::ZipArchive::new(file).unwrap();
                let mut container_ids = vec![];
                for i in 0..archive.len() {
                    let mut file = archive.by_index(i).unwrap();
                    let file_name = file.name();
                    let file_path = dest.join(file_name);
                    println!("Extracting: {}", file_path.to_str().unwrap());
                    if file.is_dir() {
                        let container_name = file.name().split("/").last().unwrap();
                        container_ids.push(container_name.to_string());
                        fs::create_dir_all(&file_path).unwrap();
                    } else {
                        let mut dest = fs::File::create(&file_path).unwrap();
                        std::io::copy(&mut file, &mut dest).unwrap();
                    }
                    if let Some(mode) = file.unix_mode() {
                        fs::set_permissions(&file_path, fs::Permissions::from_mode(mode)).unwrap();
                    }
                }

                debug!("Full Containermap: {:?}", self.container_names);
                let res = containers
                    .iter()
                    .map(|checkpoint| {
                        // Move checkpoints into correct directory for new containers
                        let src_dir = format!(
                            "/var/lib/docker/containers/{}/checkpoints",
                            checkpoint.container_id
                        );
                        let dest_dir = format!(
                            "/var/lib/docker/containers/{}/",
                            self.container_names
                                .get(&OldContainerName(checkpoint.container_id.clone()))
                                .unwrap()
                                .0
                                .clone(),
                        );
                        debug!("Moving checkpoints from {} to {}", src_dir, dest_dir);
                        assert!(
                            Path::new(&format!("{}/{}", src_dir, checkpoint.checkpoint_name))
                                .exists(),
                            "Checkpoint directory does not exist when it should"
                        );
                        assert!(
                            read_dir(&src_dir).unwrap().next().is_some(),
                            "Checkpoint directory empty when it should not be"
                        );
                        move_directory(&src_dir, &dest_dir);
                        let mut found =
                            format!("Searching {} in containermap", checkpoint.container_id);
                        if self
                            .container_names
                            .get(&OldContainerName(checkpoint.container_id.clone()))
                            .is_some()
                        {
                            found = found.green().to_string();
                        } else {
                            found = found.red().to_string();
                        }
                        debug!("{}", found);
                        let container_name = self
                            .container_names
                            .get(&OldContainerName(checkpoint.container_id.clone()))
                            .unwrap()
                            .0
                            .clone();
                        debug!(
                            "Starting container {} with checkpoint {}",
                            container_name, checkpoint.checkpoint_name
                        );
                        std::process::Command::new("docker")
                            .arg("start")
                            .arg("--checkpoint")
                            .arg(&checkpoint.checkpoint_name)
                            .arg(&container_name)
                            .output()
                            .unwrap();
                        Ok(())
                    })
                    .collect::<Result<Vec<()>, std::io::Error>>()
                    .unwrap();
                println!("Created {} containers", res.len());
            });
        });
        Ok(())
    }
}
fn move_directory(src: &str, dest: &str) {
    std::process::Command::new("mv")
        .arg(src)
        .arg(dest)
        .output()
        .unwrap();
}
/// Loads a container from a tarball and returns the id of the loaded container
async fn load_container_from_file(client: &bollard::Docker, path: &PathBuf) -> EyreResult<String> {
    let file = tokio::fs::File::open(path).await?;
    let bytes_stream =
        codec::FramedRead::new(file, codec::BytesCodec::new()).map(|r| r.unwrap().freeze());
    debug!("Importing Image from file: {:?}", path);
    let build_info = client
        .import_image_stream(ImportImageOptions { quiet: false }, bytes_stream, None)
        .next()
        .await
        .unwrap()?;
    Ok(build_info.id.unwrap())
}
#[async_trait::async_trait]
impl Migration for DockerBackend {
    async fn checkpoint(&mut self) -> EyreResult<()> {
        self.checkpoints = self.checkpoint_all_containers().await?;
        Ok(())
    }

    async fn migrate(&mut self, ip_addr: IpAddr) -> EyreResult<()> {
        self.export_all_containers().await?;
        self.transfer_containers(ip_addr).await?;
        self.migrate_all_containers(ip_addr).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use std::time::Duration;

    #[tokio::test]
    async fn test_checkpoint_container() {
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
        tokio::time::sleep(Duration::from_secs(4)).await;
        assert!(docker
            .get_containers(false)
            .is_ok_and(|containers| !containers.is_empty()));
        let mut docker_backend = DockerBackend::new().unwrap();
        let checkpoint_all_containers = docker_backend.checkpoint_all_containers().await.unwrap();
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

    #[tokio::test]
    async fn test_export_all_containers() {
        let mut docker_backend = DockerBackend::new().unwrap();
        docker_backend.export_all_containers().await.unwrap();
        let dir = tokio::fs::read_dir("containers").await.unwrap();
    }

    #[tokio::test]
    async fn test_import_all_containers() {
        tracing_subscriber::fmt::init();
        let mut docker_backend = DockerBackend::new().unwrap();
        docker_backend.import_containers().await.unwrap();
    }
}
