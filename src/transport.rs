#[cfg(windows)]
use interprocess::local_socket::GenericNamespaced;
use interprocess::local_socket::prelude::*;
#[cfg(unix)]
use interprocess::local_socket::{GenericFilePath, ListenerOptions};
use std::io;
#[cfg(unix)]
use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct Endpoint {
    instance: String,
    #[cfg(unix)]
    path: PathBuf,
    #[cfg(windows)]
    pipe_name: String,
}

impl Endpoint {
    pub fn new(instance: Option<&str>) -> io::Result<Self> {
        let instance = instance.unwrap_or("default");
        validate_instance_name(instance)?;
        #[cfg(unix)]
        {
            let (runtime_dir, used_fallback) = runtime_directory()?;
            if used_fallback {
                eprintln!(
                    "zmux: $XDG_RUNTIME_DIR is missing or unusable; using private runtime directory {}",
                    runtime_dir.display()
                );
            }
            let filename = if instance == "default" {
                "zmux.sock".to_owned()
            } else {
                format!("zmux-{instance}.sock")
            };
            Ok(Self {
                instance: instance.to_owned(),
                path: runtime_dir.join(filename),
            })
        }
        #[cfg(windows)]
        {
            let domain = std::env::var("USERDOMAIN").unwrap_or_default();
            let user = std::env::var("USERNAME").unwrap_or_else(|_| "unknown".to_owned());
            let domain = pipe_component(&domain);
            let user = pipe_component(&user);
            let pipe_name = format!("zmux-{domain}-{user}-{instance}");
            Ok(Self {
                instance: instance.to_owned(),
                pipe_name,
            })
        }
    }

    pub fn instance(&self) -> &str {
        &self.instance
    }

    #[cfg(unix)]
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    fn local_socket_name(&self) -> io::Result<interprocess::local_socket::Name<'_>> {
        #[cfg(unix)]
        {
            self.path.clone().to_fs_name::<GenericFilePath>()
        }
        #[cfg(windows)]
        {
            self.pipe_name.clone().to_ns_name::<GenericNamespaced>()
        }
    }
}

pub fn bind(endpoint: &Endpoint) -> io::Result<LocalSocketListener> {
    let name = endpoint.local_socket_name()?;
    #[cfg(windows)]
    {
        use interprocess::{
            local_socket::ListenerOptions,
            os::windows::{
                local_socket::ListenerOptionsExt, security_descriptor::SecurityDescriptor,
            },
        };
        let descriptor =
            widestring::U16CString::from_str("D:P(A;;GA;;;OW)").map_err(io::Error::other)?;
        let descriptor = SecurityDescriptor::deserialize(descriptor.as_ucstr())?;
        ListenerOptions::new()
            .name(name)
            .reclaim_name(true)
            .security_descriptor(descriptor)
            .create_sync()
    }
    #[cfg(unix)]
    {
        ListenerOptions::new()
            .name(name)
            .reclaim_name(true)
            .create_sync()
    }
}

pub fn connect(endpoint: &Endpoint) -> io::Result<LocalSocketStream> {
    interprocess::local_socket::ConnectOptions::new()
        .name(endpoint.local_socket_name()?)
        .connect_sync()
}

pub fn accept(listener: &LocalSocketListener) -> io::Result<LocalSocketStream> {
    listener.accept()
}

#[cfg(windows)]
fn pipe_component(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect()
}

fn validate_instance_name(name: &str) -> io::Result<()> {
    if name.is_empty()
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "socket instance names may contain only ASCII letters, digits, '-' and '_'",
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn runtime_directory() -> io::Result<(PathBuf, bool)> {
    if let Some(path) = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from)
        && is_secure_runtime_directory(&path).unwrap_or(false)
    {
        return Ok((path, false));
    }

    let user_id = unsafe { libc::geteuid() };
    let fallback = std::env::temp_dir().join(format!("zmux-{user_id}"));
    create_private_directory(&fallback)?;
    Ok((fallback, true))
}

#[cfg(unix)]
fn is_secure_runtime_directory(path: &std::path::Path) -> io::Result<bool> {
    use std::os::unix::fs::MetadataExt;

    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    let user_id = unsafe { libc::geteuid() };
    Ok(metadata.file_type().is_dir()
        && metadata.uid() == user_id
        && metadata.mode() & 0o777 == 0o700)
}

#[cfg(unix)]
fn create_private_directory(path: &std::path::Path) -> io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt};

    match std::fs::DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    let metadata = std::fs::symlink_metadata(path)?;
    let user_id = unsafe { libc::geteuid() };
    if !metadata.file_type().is_dir()
        || metadata.uid() != user_id
        || metadata.mode() & 0o777 != 0o700
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "runtime directory {} is not a private user-owned directory",
                path.display()
            ),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{Endpoint, create_private_directory, validate_instance_name};

    #[cfg(unix)]
    #[test]
    fn independent_server_instances_have_distinct_socket_paths() {
        let default = Endpoint::new(None).expect("default endpoint");
        let named = Endpoint::new(Some("project-1")).expect("named endpoint");

        assert_ne!(default.path(), named.path());
        assert_eq!(default.instance(), "default");
        assert_eq!(named.instance(), "project-1");
    }

    #[cfg(unix)]
    #[test]
    fn fallback_runtime_directory_is_created_private_and_user_owned() {
        use std::os::unix::fs::MetadataExt;

        let path = std::env::temp_dir().join(format!("zmux-private-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        create_private_directory(&path).expect("create private runtime directory");
        let metadata = std::fs::symlink_metadata(&path).expect("inspect runtime directory");

        assert_eq!(metadata.uid(), unsafe { libc::geteuid() });
        assert_eq!(metadata.mode() & 0o777, 0o700);

        std::fs::remove_dir(path).expect("remove private runtime directory");
    }

    #[test]
    fn instance_names_reject_path_separators() {
        assert!(validate_instance_name("safe-name_2").is_ok());
        assert!(validate_instance_name("../unsafe").is_err());
        assert!(validate_instance_name("").is_err());
    }
}
