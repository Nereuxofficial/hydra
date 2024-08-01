use color_eyre::eyre::ContextCompat;
use std::fs::File;
use std::io::{Read, Seek, Write};
use std::path::Path;
use tracing::debug;
use walkdir::DirEntry;
use walkdir::WalkDir;
use zip::result::ZipError;
use zip::write::SimpleFileOptions;
use zip::CompressionMethod;

pub fn zip_dir(src: &str, dest: &str) -> color_eyre::Result<()> {
    let src_path = Path::new(src);
    // Zstd should be pretty fast
    let method = CompressionMethod::Zstd;
    if !src_path.is_dir() {
        return Err(ZipError::FileNotFound.into());
    }
    let path = Path::new(dest);
    let file_handle = File::create(path)?;

    let walk_dir = WalkDir::new(src);
    let it = walk_dir.into_iter();

    zip_directory(&mut it.map(|i| i.unwrap()), src_path, file_handle, method)?;
    Ok(())
}

fn zip_directory<T>(
    it: &mut dyn Iterator<Item = DirEntry>,
    prefix: &Path,
    writer: T,
    method: CompressionMethod,
) -> color_eyre::Result<()>
where
    T: Write + Seek,
{
    let mut zip = zip::ZipWriter::new(writer);
    let options = SimpleFileOptions::default()
        .compression_method(method)
        .unix_permissions(0o755);

    let prefix = Path::new(prefix);
    let mut buffer = Vec::new();
    for entry in it {
        let path = entry.path();
        let name = path
            .strip_prefix(prefix)
            .unwrap_or_else(|path| panic!("{:?} is not a prefix of {:?}", prefix, path));
        let path_as_string = name
            .to_str()
            .map(str::to_owned)
            .with_context(|| format!("{name:?} Is a Non UTF-8 Path"))?;

        // Write file or directory explicitly
        // Some unzip tools unzip files with directory paths correctly, some do not!
        if path.is_file() {
            debug!("adding file {path:?} as {name:?} ...");
            zip.start_file(path_as_string, options)?;
            let mut f = File::open(path)?;

            f.read_to_end(&mut buffer)?;
            zip.write_all(&buffer)?;
            buffer.clear();
        } else if !name.as_os_str().is_empty() {
            // Only if not root! Avoids path spec / warning
            // and mapname conversion failed error on unzip
            debug!("adding dir {path_as_string:?} as {name:?} ...");
            zip.add_directory(path_as_string, options)?;
        }
    }
    zip.finish()?;
    Ok(())
}

#[cfg(test)]
mod test {
    use super::*;
    use std::fs::remove_file;

    #[test]
    fn test_zip_dir() {
        let src_dir = "src";
        let dest_dir = "src.zip";
        zip_dir(src_dir, dest_dir).unwrap();
        remove_file(dest_dir).unwrap();
    }
}
