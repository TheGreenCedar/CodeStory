use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, Read};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy)]
pub(crate) struct ExpectedModel<'a> {
    pub(crate) size: u64,
    pub(crate) sha256: &'a str,
}

pub(crate) fn verify_model(path: &Path, expected: ExpectedModel<'_>) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if !metadata.file_type().is_file() {
        return Err("path is not a regular file".into());
    }
    if metadata.len() != expected.size {
        return Err(format!(
            "size mismatch: expected {} bytes, found {}",
            expected.size,
            metadata.len()
        ));
    }

    let digest = sha256_file(path).map_err(|error| error.to_string())?;
    if digest != expected.sha256 {
        return Err(format!(
            "SHA-256 mismatch: expected {}, found {digest}",
            expected.sha256
        ));
    }
    Ok(())
}

pub(crate) fn stage_model(
    source: &Path,
    destination: &Path,
    expected: ExpectedModel<'_>,
) -> Result<(), String> {
    stage_model_with_faults(source, destination, expected, io::copy, |_, _| Ok(()))
}

fn stage_model_with_faults<CopyModel, BeforePublish>(
    source: &Path,
    destination: &Path,
    expected: ExpectedModel<'_>,
    copy_model: CopyModel,
    before_publish: BeforePublish,
) -> Result<(), String>
where
    CopyModel: FnOnce(&mut File, &mut File) -> io::Result<u64>,
    BeforePublish: FnOnce(&Path, &Path) -> io::Result<()>,
{
    match fs::symlink_metadata(destination) {
        Ok(metadata) if !metadata.file_type().is_file() => {
            return Err(format!(
                "destination {} exists and is not a regular file",
                destination.display()
            ));
        }
        Ok(_) => {
            verify_model(destination, expected)
                .map_err(|error| format!("existing destination is invalid: {error}"))?;
            return Ok(());
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.to_string()),
    }

    let (temporary, mut output) = create_temporary(destination)?;
    let result = (|| {
        let mut input = File::open(source).map_err(|error| error.to_string())?;
        if !input
            .metadata()
            .map_err(|error| error.to_string())?
            .is_file()
        {
            return Err("opened source is not a regular file".into());
        }
        let copied = copy_model(&mut input, &mut output).map_err(|error| error.to_string())?;
        if copied != expected.size {
            return Err(format!(
                "short copy: expected {} bytes, copied {copied}",
                expected.size
            ));
        }
        output.sync_all().map_err(|error| error.to_string())?;
        drop(output);

        verify_model(&temporary, expected)
            .map_err(|error| format!("temporary staged model failed verification: {error}"))?;
        match fs::symlink_metadata(destination) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Ok(_) => {
                return Err(format!(
                    "destination {} appeared during staging; refusing to overwrite it",
                    destination.display()
                ));
            }
            Err(error) => return Err(error.to_string()),
        }
        before_publish(&temporary, destination).map_err(|error| error.to_string())?;
        fs::hard_link(&temporary, destination).map_err(|error| {
            if error.kind() == io::ErrorKind::AlreadyExists {
                format!(
                    "destination {} appeared during publication; refusing to overwrite it",
                    destination.display()
                )
            } else {
                error.to_string()
            }
        })?;
        if let Err(error) = verify_model(destination, expected) {
            let _ = fs::remove_file(destination);
            return Err(format!(
                "published staged model failed verification: {error}"
            ));
        }
        fs::remove_file(&temporary).map_err(|error| error.to_string())?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn create_temporary(destination: &Path) -> Result<(PathBuf, File), String> {
    let parent = destination
        .parent()
        .ok_or_else(|| "embedded model destination has no parent".to_string())?;
    let file_name = destination
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "embedded model destination has no UTF-8 file name".to_string())?;
    for attempt in 0..32 {
        let temporary = parent.join(format!(".{file_name}.{attempt}.partial"));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
        {
            Ok(file) => return Ok((temporary, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.to_string()),
        }
    }
    Err("failed to create a unique embedded model staging file".into())
}

fn sha256_file(path: &Path) -> io::Result<String> {
    let mut reader = BufReader::new(File::open(path)?);
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::tempdir;

    const MODEL: &[u8] = b"deterministic synthetic embedding model";

    fn expected(digest: &str) -> ExpectedModel<'_> {
        ExpectedModel {
            size: MODEL.len() as u64,
            sha256: digest,
        }
    }

    fn digest() -> String {
        format!("{:x}", Sha256::digest(MODEL))
    }

    fn assert_no_partial(directory: &Path) {
        let partials = fs::read_dir(directory)
            .expect("read staging directory")
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".partial"))
            .collect::<Vec<_>>();
        assert!(partials.is_empty(), "staging partial was not cleaned up");
    }

    #[test]
    fn short_copy_never_publishes_partial_data() {
        let root = tempdir().expect("tempdir");
        let source = root.path().join("source.gguf");
        let destination = root.path().join("published.gguf");
        fs::write(&source, MODEL).expect("write source");
        let digest = digest();

        let error = stage_model_with_faults(
            &source,
            &destination,
            expected(&digest),
            |input, output| {
                let mut partial = input.take((MODEL.len() - 1) as u64);
                io::copy(&mut partial, output)
            },
            |_, _| Ok(()),
        )
        .expect_err("short copy must fail");

        assert!(error.contains("short copy"), "unexpected error: {error}");
        assert!(!destination.exists(), "partial model was published");
        assert_no_partial(root.path());
    }

    #[test]
    fn partial_write_failure_never_publishes_partial_data() {
        let root = tempdir().expect("tempdir");
        let source = root.path().join("source.gguf");
        let destination = root.path().join("published.gguf");
        fs::write(&source, MODEL).expect("write source");
        let digest = digest();

        let error = stage_model_with_faults(
            &source,
            &destination,
            expected(&digest),
            |input, output| {
                let mut bytes = [0_u8; 8];
                input.read_exact(&mut bytes)?;
                output.write_all(&bytes)?;
                Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "injected partial write",
                ))
            },
            |_, _| Ok(()),
        )
        .expect_err("partial write must fail");

        assert!(
            error.contains("injected partial write"),
            "unexpected error: {error}"
        );
        assert!(!destination.exists(), "partial model was published");
        assert_no_partial(root.path());
    }

    #[test]
    fn competing_destination_appearance_is_never_clobbered() {
        let root = tempdir().expect("tempdir");
        let source = root.path().join("source.gguf");
        let destination = root.path().join("published.gguf");
        fs::write(&source, MODEL).expect("write source");
        let digest = digest();
        let competing_bytes = b"racing publisher owns this path";

        let error = stage_model_with_faults(
            &source,
            &destination,
            expected(&digest),
            |input, output| io::copy(input, output),
            |_, destination| fs::write(destination, competing_bytes),
        )
        .expect_err("competing destination must fail");

        assert!(
            error.contains("appeared during publication"),
            "unexpected error: {error}"
        );
        assert_eq!(
            fs::read(&destination).expect("read competing destination"),
            competing_bytes,
            "competing destination was clobbered"
        );
        assert_no_partial(root.path());
    }
}
