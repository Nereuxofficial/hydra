//! Simple representation of files for the purpose of transferring them via ssh.

use russh_sftp::client::SftpSession;
use std::path::PathBuf;
use tokio::io::AsyncWriteExt;

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct Entry {
    pub(crate) path: PathBuf,
    pub(crate) inner: FileInner,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum FileInner {
    Directory(Vec<Entry>),
    File(Vec<u8>),
}

impl Entry {
    pub(crate) fn from_directory(path: PathBuf) -> std::io::Result<Self> {
        let res = std::fs::read_dir(path.clone())?;
        let entries = res
            .map(|entry| {
                let entry = entry?;
                let path = entry.path();
                if path.is_dir() {
                    Entry::from_directory(path)
                } else {
                    Ok(Entry {
                        path: path.clone(),
                        inner: FileInner::File(std::fs::read(&path)?),
                    })
                }
            })
            .collect::<std::io::Result<Vec<_>>>()?;
        Ok(Entry {
            path,
            inner: FileInner::Directory(entries),
        })
    }

    /// Transfers a folder including its contents via SFTP
    /// This function is recursive and will transfer all files and subdirectories
    /// in the folder.
    /// Depending on the location of the folder it might be necessary to have a root session remotely
    /// Also this does not transfer the permissions of the files...
    pub(crate) async fn transfer_via_ssh(
        &self,
        session: &SftpSession,
    ) -> Result<(), russh_sftp::client::error::Error> {
        let path_string = self.path.to_str().unwrap().to_string();
        match &self.inner {
            FileInner::Directory(entries) => {
                session.create_dir(&path_string).await?;
                for entry in entries {
                    // TODO: Eliminate the recursion by using a stack and parallelize the transfer
                    Box::pin(entry.transfer_via_ssh(session)).await?;
                }
            }
            FileInner::File(data) => {
                let mut file = session.create(&path_string).await?;
                file.write_all(data).await?;
                file.sync_all().await?;
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_entry_from_directory() {
        let entry = Entry::from_directory(PathBuf::from("src/")).unwrap();
        if let FileInner::Directory(entries) = entry.inner {
            assert!(entries.len() > 5);
        } else {
            panic!("Expected directory");
        }
    }
}
