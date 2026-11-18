use crate::GritApp;
use std::{path::PathBuf, process::Command};

#[cfg(unix)]
use std::sync::mpsc::Receiver;

const MAX_PATH_BYTES: usize = 4096;
const WINDOW_ARG: &str = "--new-window";

#[derive(Default)]
pub struct WindowTools {
    pub error: Option<String>,
}

pub enum InstanceStartup {
    Primary(InstanceHandle),
    #[cfg(unix)]
    Forwarded,
    Unavailable(String),
    /// Explicit `--new-window` bypass; this window does not own the IPC endpoint.
    Bypassed,
}

#[cfg(unix)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InstanceMessage {
    Open(PathBuf),
    Focus,
}

#[cfg(unix)]
pub struct InstanceHandle {
    receiver: Receiver<InstanceMessage>,
    alive: std::sync::Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    paths: unix_instance::OwnedSocket,
}

#[cfg(not(unix))]
pub struct InstanceHandle;

impl InstanceHandle {
    /// Drain one pending request without blocking the GUI thread.
    #[cfg(unix)]
    pub fn try_recv(&self) -> Option<InstanceMessage> {
        self.receiver.try_recv().ok()
    }

    pub fn platform_note(&self) -> Option<&'static str> {
        #[cfg(unix)]
        {
            None
        }
        #[cfg(not(unix))]
        {
            Some("Single-instance forwarding is not available on this platform.")
        }
    }
}

#[cfg(unix)]
impl Drop for InstanceHandle {
    fn drop(&mut self) {
        use std::sync::atomic::Ordering;
        self.alive.store(false, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        self.paths.remove_if_owned();
    }
}

impl WindowTools {
    pub fn open_new_window(&mut self, path: Option<PathBuf>) -> Result<(), String> {
        let path =
            path.ok_or_else(|| "Open a repository before opening another window".to_owned())?;
        let path = path
            .canonicalize()
            .map_err(|e| format!("Could not resolve repository path: {e}"))?;
        validate_open_path(&path)?;
        let current_executable = std::env::current_exe()
            .map_err(|e| format!("Could not locate grit executable: {e}"))?;
        let executable = select_new_window_executable(
            std::env::var_os("APPIMAGE").map(PathBuf::from),
            current_executable,
        )?;
        Command::new(executable)
            .args(new_window_args(&path))
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("Could not open new window: {e}"))
    }
}

fn select_new_window_executable(
    appimage: Option<PathBuf>,
    current_executable: PathBuf,
) -> Result<PathBuf, String> {
    let candidate = appimage.unwrap_or(current_executable);
    let canonical = candidate
        .canonicalize()
        .map_err(|e| format!("Could not resolve new-window executable: {e}"))?;
    let metadata = std::fs::metadata(&canonical)
        .map_err(|e| format!("Could not inspect new-window executable: {e}"))?;
    if !metadata.is_file() {
        return Err("New-window executable is not a regular file".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return Err("New-window executable has no execute permission".into());
        }
    }
    Ok(canonical)
}

impl GritApp {
    pub fn open_new_window(&mut self, path: Option<PathBuf>) {
        match self
            .window_tools
            .open_new_window(path.or_else(|| self.path.clone()))
        {
            Ok(()) => {
                self.window_tools.error = None;
                self.notice = "Opened a new grit window".into();
            }
            Err(error) => {
                self.window_tools.error = Some(error.clone());
                self.notice = error;
            }
        }
    }
}

/// Construct the exact argv fragment used by the new-window action.
pub fn new_window_args(path: &std::path::Path) -> Vec<std::ffi::OsString> {
    vec![WINDOW_ARG.into(), path.as_os_str().to_owned()]
}

/// Validate paths crossing the local IPC boundary.
pub fn validate_open_path(path: &std::path::Path) -> Result<(), String> {
    if !path.is_absolute() {
        return Err("Repository paths sent to another window must be absolute".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let bytes = path.as_os_str().as_bytes();
        if bytes.len() > MAX_PATH_BYTES {
            return Err(format!(
                "Repository path exceeds the {MAX_PATH_BYTES}-byte IPC limit"
            ));
        }
        if bytes.contains(&0) {
            return Err("Repository path contains a NUL byte".into());
        }
    }
    #[cfg(not(unix))]
    if path.as_os_str().len() > MAX_PATH_BYTES {
        return Err(format!(
            "Repository path exceeds the {MAX_PATH_BYTES}-byte IPC limit"
        ));
    }
    Ok(())
}

/// Initialize single-instance coordination before constructing the native window.
///
/// A primary instance returns a receiver-bearing handle. If another primary is active,
/// this forwards the optional path (or a focus request) and returns `Forwarded`. The
/// explicit new-window flag bypasses coordination. Non-Unix platforms keep working but
/// return a primary handle whose `platform_note()` explains that forwarding is unavailable.
pub fn instance_startup(path: Option<PathBuf>, bypass: bool) -> InstanceStartup {
    if bypass {
        return InstanceStartup::Bypassed;
    }
    let path = path.map(|path| {
        if path.is_absolute() {
            path
        } else {
            match std::env::current_dir() {
                Ok(cwd) => cwd.join(&path),
                Err(_) => path,
            }
        }
    });
    if let Some(path) = &path
        && let Err(error) = validate_open_path(path)
    {
        return InstanceStartup::Unavailable(error);
    }
    #[cfg(unix)]
    {
        match unix_instance::startup(runtime_base(), path) {
            Ok(unix_instance::Startup::Primary(handle)) => InstanceStartup::Primary(handle),
            Ok(unix_instance::Startup::Forwarded) => InstanceStartup::Forwarded,
            Err(error) => InstanceStartup::Unavailable(error),
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        InstanceStartup::Primary(InstanceHandle)
    }
}

#[cfg(unix)]
fn runtime_base() -> Result<std::path::PathBuf, String> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
    let uid = unsafe { geteuid() };
    let base = match std::env::var_os("XDG_RUNTIME_DIR") {
        Some(value) => {
            let path = PathBuf::from(value);
            if !path.is_absolute() {
                return Err("XDG_RUNTIME_DIR must be an absolute directory".into());
            }
            let meta = std::fs::symlink_metadata(&path)
                .map_err(|e| format!("Could not inspect XDG_RUNTIME_DIR: {e}"))?;
            if meta.file_type().is_symlink()
                || !meta.is_dir()
                || meta.uid() != uid
                || meta.permissions().mode() & 0o077 != 0
            {
                return Err("XDG_RUNTIME_DIR must be an owned private directory".into());
            }
            path
        }
        None => std::env::temp_dir(),
    };
    let private = base.join(format!("grit-instance-{uid}"));
    let mut builder = std::fs::DirBuilder::new();
    builder.mode(0o700);
    match builder.create(&private) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(format!("Could not create the instance directory: {e}")),
    }
    let meta = std::fs::symlink_metadata(&private)
        .map_err(|e| format!("Could not inspect the instance directory: {e}"))?;
    if meta.file_type().is_symlink()
        || !meta.is_dir()
        || meta.uid() != uid
        || meta.permissions().mode() & 0o777 != 0o700
    {
        return Err("Instance directory is not a private directory owned by this user".into());
    }
    Ok(private)
}

#[cfg(unix)]
unsafe extern "C" {
    fn geteuid() -> u32;
}

#[cfg(unix)]
mod unix_instance {
    use super::{InstanceMessage, MAX_PATH_BYTES};
    use std::{
        fs::{self, File, OpenOptions},
        io::{Read, Write},
        os::unix::{
            ffi::{OsStrExt, OsStringExt},
            fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt},
            net::{UnixListener, UnixStream},
        },
        path::{Path, PathBuf},
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
            mpsc,
        },
        thread,
        time::Duration,
    };

    const MAGIC: &[u8; 5] = b"GRIT1";
    const OPEN: u8 = 1;
    const FOCUS: u8 = 2;
    const OK: u8 = 1;
    const REJECT: u8 = 0;

    pub enum Startup {
        Primary(super::InstanceHandle),
        Forwarded,
    }

    pub struct OwnedSocket {
        directory: PathBuf,
        lock_path: PathBuf,
        socket_path: PathBuf,
        dev: u64,
        ino: u64,
    }

    impl OwnedSocket {
        pub fn remove_if_owned(&self) {
            let Ok(lock) = open_lock(&self.directory, &self.lock_path) else {
                return;
            };
            if lock.lock().is_err() {
                return;
            }
            let Ok(meta) = fs::symlink_metadata(&self.socket_path) else {
                return;
            };
            let uid = unsafe { super::geteuid() };
            if meta.file_type().is_socket()
                && meta.uid() == uid
                && meta.dev() == self.dev
                && meta.ino() == self.ino
            {
                let _ = fs::remove_file(&self.socket_path);
            }
        }
    }

    pub fn startup(
        base: Result<PathBuf, String>,
        path: Option<PathBuf>,
    ) -> Result<Startup, String> {
        let directory = base?;
        let lock_path = directory.join("instance.lock");
        let socket_path = directory.join("instance.sock");
        if socket_path.as_os_str().as_bytes().len() > 100 {
            return Err("Runtime directory path is too long for a Unix-domain socket".into());
        }
        let lock = open_lock(&directory, &lock_path)?;
        lock.lock()
            .map_err(|e| format!("Could not lock the instance directory: {e}"))?;

        let socket_exists = match fs::symlink_metadata(&socket_path) {
            Ok(meta)
                if !meta.file_type().is_socket()
                    || meta.uid() != unsafe { super::geteuid() }
                    || meta.permissions().mode() & 0o077 != 0 =>
            {
                return Err("Refusing to use an unexpected instance socket path".into());
            }
            Ok(_) => true,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(error) => return Err(format!("Could not inspect instance socket path: {error}")),
        };

        if socket_exists {
            match UnixStream::connect(&socket_path) {
                Ok(mut stream) => {
                    stream.set_read_timeout(Some(Duration::from_secs(2))).ok();
                    send_request(&mut stream, path.as_deref())?;
                    let mut response = [0_u8; 1];
                    stream.read_exact(&mut response).map_err(|e| {
                        format!("Active grit instance did not acknowledge the request: {e}")
                    })?;
                    if response[0] != OK {
                        return Err("Active grit instance rejected the open request".into());
                    }
                    return Ok(Startup::Forwarded);
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) if error.kind() == std::io::ErrorKind::ConnectionRefused => {
                    remove_stale_socket(&socket_path)?;
                }
                Err(error) => {
                    return Err(format!(
                        "Could not connect to the existing grit instance: {error}"
                    ));
                }
            }
        }

        let listener = UnixListener::bind(&socket_path)
            .map_err(|e| format!("Could not bind the grit instance socket: {e}"))?;
        fs::set_permissions(&socket_path, fs::Permissions::from_mode(0o600))
            .map_err(|e| format!("Could not secure the grit instance socket: {e}"))?;
        listener
            .set_nonblocking(true)
            .map_err(|e| format!("Could not configure the grit instance socket: {e}"))?;
        let meta = fs::symlink_metadata(&socket_path).map_err(|e| e.to_string())?;
        let owned = OwnedSocket {
            directory: directory.clone(),
            lock_path,
            socket_path,
            dev: meta.dev(),
            ino: meta.ino(),
        };
        drop(lock);

        let (sender, receiver) = mpsc::channel();
        let alive = Arc::new(AtomicBool::new(true));
        let worker_alive = alive.clone();
        let thread = thread::Builder::new()
            .name("grit-instance-ipc".into())
            .spawn(move || listener_loop(listener, sender, worker_alive))
            .map_err(|e| format!("Could not start the instance listener: {e}"))?;
        Ok(Startup::Primary(super::InstanceHandle {
            receiver,
            alive,
            thread: Some(thread),
            paths: owned,
        }))
    }

    fn open_lock(directory: &Path, lock_path: &Path) -> Result<File, String> {
        let file = match OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(lock_path)
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                let link_meta = fs::symlink_metadata(lock_path)
                    .map_err(|e| format!("Could not inspect instance lock: {e}"))?;
                if link_meta.file_type().is_symlink() || !link_meta.is_file() {
                    return Err("Instance lock is not a regular file".into());
                }
                OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(lock_path)
                    .map_err(|e| format!("Could not open instance lock: {e}"))?
            }
            Err(error) => return Err(format!("Could not create instance lock: {error}")),
        };
        let dir_meta = fs::symlink_metadata(directory).map_err(|e| e.to_string())?;
        let meta = file.metadata().map_err(|e| e.to_string())?;
        let uid = unsafe { super::geteuid() };
        if !dir_meta.is_dir()
            || dir_meta.file_type().is_symlink()
            || dir_meta.uid() != uid
            || dir_meta.permissions().mode() & 0o777 != 0o700
            || !meta.is_file()
            || meta.uid() != uid
            || meta.permissions().mode() & 0o777 != 0o600
        {
            return Err("Instance lock or directory has unsafe ownership or permissions".into());
        }
        Ok(file)
    }

    fn remove_stale_socket(path: &Path) -> Result<(), String> {
        match fs::symlink_metadata(path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(format!("Could not inspect stale instance socket: {error}")),
            Ok(meta) => {
                let uid = unsafe { super::geteuid() };
                if !meta.file_type().is_socket() || meta.uid() != uid {
                    return Err("Refusing to remove an unexpected instance socket path".into());
                }
                fs::remove_file(path)
                    .map_err(|e| format!("Could not remove stale instance socket: {e}"))
            }
        }
    }

    fn send_request(stream: &mut UnixStream, path: Option<&Path>) -> Result<(), String> {
        stream.write_all(MAGIC).map_err(|e| e.to_string())?;
        match path {
            None => stream.write_all(&[FOCUS]).map_err(|e| e.to_string()),
            Some(path) => {
                super::validate_open_path(path)?;
                let bytes = path.as_os_str().as_bytes();
                stream.write_all(&[OPEN]).map_err(|e| e.to_string())?;
                stream
                    .write_all(&(bytes.len() as u32).to_be_bytes())
                    .map_err(|e| e.to_string())?;
                stream.write_all(bytes).map_err(|e| e.to_string())
            }
        }
    }

    fn listener_loop(
        listener: UnixListener,
        sender: mpsc::Sender<InstanceMessage>,
        alive: Arc<AtomicBool>,
    ) {
        while alive.load(Ordering::Acquire) {
            match listener.accept() {
                Ok((mut stream, _)) => handle_request(&mut stream, &sender),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(30))
                }
                Err(_) => thread::sleep(Duration::from_millis(50)),
            }
        }
    }

    fn handle_request(stream: &mut UnixStream, sender: &mpsc::Sender<InstanceMessage>) {
        stream.set_read_timeout(Some(Duration::from_secs(2))).ok();
        let result = read_request(stream).and_then(|request| {
            sender
                .send(request)
                .map_err(|_| "Primary window is closing".to_owned())
        });
        let _ = stream.write_all(&[if result.is_ok() { OK } else { REJECT }]);
    }

    fn read_request(stream: &mut UnixStream) -> Result<InstanceMessage, String> {
        let mut header = [0_u8; 6];
        stream.read_exact(&mut header).map_err(|e| e.to_string())?;
        if &header[..5] != MAGIC {
            return Err("Invalid instance request signature".into());
        }
        match header[5] {
            FOCUS => Ok(InstanceMessage::Focus),
            OPEN => {
                let mut length = [0_u8; 4];
                stream.read_exact(&mut length).map_err(|e| e.to_string())?;
                let length = u32::from_be_bytes(length) as usize;
                if length == 0 || length > MAX_PATH_BYTES {
                    return Err("Invalid repository path length".into());
                }
                let mut bytes = vec![0_u8; length];
                stream.read_exact(&mut bytes).map_err(|e| e.to_string())?;
                if bytes.contains(&0) {
                    return Err("Repository path contains a NUL byte".into());
                }
                let path = PathBuf::from(std::ffi::OsString::from_vec(bytes));
                super::validate_open_path(&path)?;
                Ok(InstanceMessage::Open(path))
            }
            _ => Err("Unknown instance request type".into()),
        }
    }

    #[cfg(test)]
    pub fn socket_path_for_test(directory: &Path) -> PathBuf {
        directory.join("instance.sock")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn new_window_argv_and_path_validation() {
        let path = PathBuf::from("/tmp/repo with spaces");
        assert_eq!(
            new_window_args(&path),
            vec!["--new-window".into(), path.as_os_str().to_owned()]
        );
        assert!(validate_open_path(&path).is_ok());
        assert!(validate_open_path(Path::new("relative/repo")).is_err());
    }

    #[test]
    fn new_window_uses_valid_appimage_then_falls_back_to_current_executable() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "grit-executable-test-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let appimage = dir.join("application.AppImage");
        let development = dir.join("grit");
        std::fs::write(&appimage, b"appimage").unwrap();
        std::fs::write(&development, b"binary").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&appimage, std::fs::Permissions::from_mode(0o755)).unwrap();
            std::fs::set_permissions(&development, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        assert_eq!(
            select_new_window_executable(Some(appimage.clone()), development.clone()).unwrap(),
            appimage.canonicalize().unwrap()
        );
        assert_eq!(
            select_new_window_executable(None, development.clone()).unwrap(),
            development.canonicalize().unwrap()
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&appimage, std::fs::Permissions::from_mode(0o644)).unwrap();
            assert!(select_new_window_executable(Some(appimage), development).is_err());
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn local_instances_forward_open_and_reject_malformed_requests() {
        use std::{
            io::{Read, Write},
            os::unix::{fs::PermissionsExt, net::UnixStream},
            time::{Duration, SystemTime, UNIX_EPOCH},
        };
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let base =
            std::env::temp_dir().join(format!("grit-instance-test-{}-{nonce}", std::process::id()));
        std::fs::create_dir_all(&base).unwrap();
        std::fs::set_permissions(&base, std::fs::Permissions::from_mode(0o700)).unwrap();
        let primary = match unix_instance::startup(Ok(base.clone()), None).unwrap() {
            unix_instance::Startup::Primary(handle) => handle,
            unix_instance::Startup::Forwarded => panic!("first instance forwarded"),
        };
        let socket_path = unix_instance::socket_path_for_test(&base);

        let mut malformed = UnixStream::connect(&socket_path).unwrap();
        malformed.write_all(b"BAD!!\x02").unwrap();
        let mut reply = [1_u8; 1];
        malformed.read_exact(&mut reply).unwrap();
        assert_eq!(reply[0], 0);
        assert!(primary.try_recv().is_none());

        let mut oversized = UnixStream::connect(&socket_path).unwrap();
        oversized.write_all(b"GRIT1\x01").unwrap();
        oversized
            .write_all(&((MAX_PATH_BYTES as u32) + 1).to_be_bytes())
            .unwrap();
        oversized.read_exact(&mut reply).unwrap();
        assert_eq!(reply[0], 0);
        assert!(primary.try_recv().is_none());

        let target = base.join("opened repository");
        std::fs::create_dir(&target).unwrap();
        assert!(matches!(
            unix_instance::startup(Ok(base.clone()), Some(target.clone())).unwrap(),
            unix_instance::Startup::Forwarded
        ));
        let started = std::time::Instant::now();
        loop {
            if primary.try_recv() == Some(InstanceMessage::Open(target.clone())) {
                break;
            }
            assert!(
                started.elapsed() < Duration::from_secs(2),
                "open request not delivered"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(matches!(
            unix_instance::startup(Ok(base.clone()), None).unwrap(),
            unix_instance::Startup::Forwarded
        ));
        assert_eq!(primary.try_recv(), Some(InstanceMessage::Focus));
        drop(primary);
        std::fs::remove_dir_all(base).unwrap();
    }
}
