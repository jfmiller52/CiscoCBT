//! This computer's side of the image cable: its wired ethernet ports, and
//! the temporary TFTP server address on the chosen one.

use std::fs;
use std::io;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use bench_core::server_address;
use netdev::prelude::{Interface, InterfaceType};

/// The wired ethernet ports, named as `add_server_address` takes them.
pub fn ethernet_ports() -> Vec<String> {
    netdev::get_interfaces()
        .iter()
        .filter(|interface| is_wired(interface))
        .map(port_name)
        .collect()
}

/// Gives `port` the server address once an administrator approves. Does
/// nothing when the port already has it, as after a run whose removal was
/// declined. The image copy does not call this: the elevated helper adds
/// the address itself. A bad port name still fails here, before any approval.
#[allow(dead_code)]
pub fn add_server_address(port: &str) -> io::Result<()> {
    if has_server_address(port) {
        return Ok(());
    }
    platform::run_elevated(&platform::add_command(port)?)
}

/// Why the elevated helper did not start listening.
#[derive(Debug, PartialEq, Eq)]
pub enum StartFailure {
    Address(String),
    Bind(String),
}

/// Reads the helper's `error` file.
pub fn read_start_failure(text: &str) -> StartFailure {
    if let Some(detail) = text.strip_prefix("bind\n") {
        StartFailure::Bind(detail.trim().to_string())
    } else if let Some(detail) = text.strip_prefix("address\n") {
        StartFailure::Address(detail.trim().to_string())
    } else {
        StartFailure::Address(text.trim().to_string())
    }
}

/// The command an administrator runs. It starts this program again, and that
/// process binds `bind`. For a device copy, `bind` is UDP 69 at the server
/// address.
pub fn elevated_server_command(exe: &Path, bind: SocketAddr, job: &Path) -> String {
    format!(
        "{} --tftp-serve {} {}",
        shell_quote(&exe.display().to_string()),
        bind,
        shell_quote(&job.display().to_string())
    )
}

fn shell_quote(text: &str) -> String {
    if cfg!(windows) {
        format!("\"{}\"", text.replace('"', "\"\""))
    } else {
        format!("'{}'", text.replace('\'', "'\\''"))
    }
}

/// Adds the server address without another approval prompt. The elevated
/// helper calls this after the administrator has already approved. An
/// address left from a run whose removal was declined is left as it is.
pub fn apply_server_address(port: &str) -> io::Result<()> {
    if has_server_address(port) {
        return Ok(());
    }
    platform::run_direct(&platform::add_command(port)?)
}

/// Removes the server address without another approval prompt. The elevated
/// helper calls this when the server stops or the bind gives up.
pub fn release_server_address(port: &str) -> io::Result<()> {
    platform::run_direct(&platform::remove_command(port)?)
}

/// The elevated process that adds the server address and binds UDP 69.
pub struct ImageServer {
    job: PathBuf,
    helper: Option<JoinHandle<io::Result<()>>>,
}

/// The helper the window must stop if it closes while a copy is in progress.
/// The worker thread is detached, so closing the window does not drop
/// `ImageServer`.
#[derive(Clone)]
struct ActiveServer {
    job: PathBuf,
    /// Set once the elevated process has exited and removed the address.
    finished: Arc<AtomicBool>,
}

static ACTIVE: Mutex<Option<ActiveServer>> = Mutex::new(None);

fn remember_active(job: PathBuf, finished: Arc<AtomicBool>) {
    *ACTIVE.lock().unwrap() = Some(ActiveServer { job, finished });
}

fn forget_active(job: &Path) {
    let mut slot = ACTIVE.lock().unwrap();
    if slot.as_ref().is_some_and(|active| active.job == job) {
        *slot = None;
    }
}

struct HelperFinished(Arc<AtomicBool>);

impl Drop for HelperFinished {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

impl ImageServer {
    /// Waits until the helper is listening, or reports why it will not be.
    pub fn wait_until_listening(&self) -> Result<(), StartFailure> {
        loop {
            if self.job.join("ready").exists() {
                return Ok(());
            }
            if let Some(failure) = self.read_error() {
                return Err(failure);
            }
            if self
                .helper
                .as_ref()
                .is_some_and(|helper| helper.is_finished())
            {
                if let Some(failure) = self.read_error() {
                    return Err(failure);
                }
                return Err(StartFailure::Address(
                    "the privileged TFTP helper exited before it was listening".into(),
                ));
            }
            thread::sleep(std::time::Duration::from_millis(200));
        }
    }

    /// Asks the helper to stop and waits until it has removed the address.
    pub fn finish(mut self) -> io::Result<()> {
        self.stop_helper()
    }

    /// Writes the stop file and waits until the helper has exited. The helper
    /// removes the laptop address on its way out.
    fn stop_helper(&mut self) -> io::Result<()> {
        let _ = fs::write(self.job.join("stop"), b"");
        let joined = match self.helper.take() {
            Some(helper) => match helper.join() {
                Ok(result) => result,
                Err(_) => Err(io::Error::other("the TFTP server stopped unexpectedly")),
            },
            None => Ok(()),
        };
        forget_active(&self.job);
        let _ = fs::remove_dir_all(&self.job);
        joined
    }

    fn read_error(&self) -> Option<StartFailure> {
        fs::read_to_string(self.job.join("error"))
            .ok()
            .map(|text| read_start_failure(&text))
    }
}

impl Drop for ImageServer {
    fn drop(&mut self) {
        let _ = self.stop_helper();
    }
}

/// Stops the image server if a copy is in progress. Closing the window
/// exits the process without dropping the worker's `ImageServer`, so this
/// writes the same stop file and waits until the elevated process has
/// removed the address.
pub fn stop_active_image_server() {
    let active = ACTIVE.lock().unwrap().clone();
    let Some(active) = active else {
        return;
    };
    let _ = fs::write(active.job.join("stop"), b"");
    let deadline = Instant::now() + Duration::from_secs(5);
    while !active.finished.load(Ordering::SeqCst) {
        if Instant::now() >= deadline {
            return;
        }
        thread::sleep(Duration::from_millis(50));
    }
    forget_active(&active.job);
    let _ = fs::remove_dir_all(&active.job);
}

/// Starts the elevated helper that adds `port`'s server address and binds
/// UDP 69 there. The administrator is asked once. This returns before that
/// approval; wait with [`ImageServer::wait_until_listening`].
pub fn spawn_image_server(port: &str, image: &Path, chosen: &str) -> io::Result<ImageServer> {
    let _ = platform::add_command(port)?;
    let job = create_job(port, image, chosen)?;
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(err) => {
            let _ = fs::remove_dir_all(&job);
            return Err(err);
        }
    };
    let command = elevated_server_command(&exe, crate::tftp::device_bind(), &job);
    let job_for_thread = job.clone();
    let finished = Arc::new(AtomicBool::new(false));
    remember_active(job.clone(), Arc::clone(&finished));
    let helper = thread::spawn(move || {
        let _finished = HelperFinished(finished);
        let result = platform::run_elevated_shell(&command);
        if let Err(ref err) = result {
            let path = job_for_thread.join("error");
            if !path.exists() {
                let _ = fs::write(path, format!("address\n{err}"));
            }
        }
        result
    });
    Ok(ImageServer {
        job,
        helper: Some(helper),
    })
}

fn create_job(port: &str, image: &Path, chosen: &str) -> io::Result<PathBuf> {
    let dir = std::env::temp_dir().join(format!(
        "bench-tftp-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or(0)
    ));
    fs::create_dir_all(&dir)?;
    let created = (|| {
        // The elevated helper is denied Downloads, Documents, Desktop, and
        // removable volumes. The app process can read the picked file, so
        // the helper serves this copy.
        let copy = dir.join("firmware");
        fs::copy(image, &copy)?;
        let copy = copy
            .to_str()
            .ok_or_else(|| io::Error::other("the copied image path is not valid text"))?;
        fs::write(dir.join("port"), port)?;
        fs::write(dir.join("image"), copy)?;
        fs::write(dir.join("chosen"), chosen)?;
        Ok(dir.clone())
    })();
    if created.is_err() {
        let _ = fs::remove_dir_all(&dir);
    }
    created
}

/// A wired port the system names for its user. macOS also has unnamed
/// internal ethernet interfaces, such as `anpi0`, that no cable reaches.
fn is_wired(interface: &Interface) -> bool {
    matches!(
        interface.if_type,
        InterfaceType::Ethernet
            | InterfaceType::Ethernet3Megabit
            | InterfaceType::FastEthernetT
            | InterfaceType::FastEthernetFx
            | InterfaceType::GigabitEthernet
    ) && interface.is_physical()
        && interface.friendly_name.is_some()
}

/// `netsh` takes an adapter's friendly name and `ifconfig` its BSD name.
fn port_name(interface: &Interface) -> String {
    match &interface.friendly_name {
        Some(friendly) if cfg!(windows) => friendly.clone(),
        _ => interface.name.clone(),
    }
}

fn has_server_address(port: &str) -> bool {
    let address: Ipv4Addr = server_address()
        .parse()
        .expect("the server address is an IPv4 address");
    netdev::get_interfaces()
        .iter()
        .any(|interface| port_name(interface) == port && interface.ipv4_addrs().contains(&address))
}

fn bad_port(port: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        format!("{port:?} is not an ethernet port name"),
    )
}

#[cfg(target_os = "macos")]
mod platform {
    use std::io;
    use std::process::Command;

    use bench_core::{mask, server_address};

    pub fn add_command(port: &str) -> io::Result<String> {
        Ok(format!(
            "ifconfig {} inet {} netmask {} alias",
            checked(port)?,
            server_address(),
            mask()
        ))
    }

    pub fn remove_command(port: &str) -> io::Result<String> {
        Ok(format!(
            "ifconfig {} inet {} -alias",
            checked(port)?,
            server_address()
        ))
    }

    /// The name goes into a root shell, so it may hold only the characters
    /// of a BSD interface name.
    fn checked(port: &str) -> io::Result<&str> {
        let allowed = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-');
        if !port.is_empty() && port.chars().all(allowed) {
            Ok(port)
        } else {
            Err(super::bad_port(port))
        }
    }

    /// Runs `command` as root after macOS asks for an administrator's password.
    pub fn run_elevated(command: &str) -> io::Result<()> {
        let escaped = command.replace('\\', "\\\\").replace('"', "\\\"");
        let script = format!("do shell script \"{escaped}\" with administrator privileges");
        let output = Command::new("osascript").arg("-e").arg(script).output()?;
        if output.status.success() {
            Ok(())
        } else {
            let message = String::from_utf8_lossy(&output.stderr);
            Err(io::Error::other(message.trim().to_string()))
        }
    }

    pub fn run_elevated_shell(command: &str) -> io::Result<()> {
        run_elevated(command)
    }

    /// Runs `command` as this process. The elevated helper is already root.
    pub fn run_direct(command: &str) -> io::Result<()> {
        let output = Command::new("sh").arg("-c").arg(command).output()?;
        if output.status.success() {
            Ok(())
        } else {
            let message = String::from_utf8_lossy(&output.stderr);
            let message = message.trim();
            if message.is_empty() {
                Err(io::Error::other(format!(
                    "command exited with {}",
                    output.status
                )))
            } else {
                Err(io::Error::other(message.to_string()))
            }
        }
    }
}

#[cfg(windows)]
mod platform {
    use std::ffi::OsStr;
    use std::io;
    use std::mem;
    use std::os::windows::ffi::OsStrExt;
    use std::process::Command;

    use bench_core::{mask, server_address};
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, WaitForSingleObject, INFINITE,
    };
    use windows_sys::Win32::UI::Shell::{
        ShellExecuteExW, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_HIDE;

    /// `store=active` keeps the address only until the next reboot.
    pub fn add_command(port: &str) -> io::Result<String> {
        Ok(format!(
            "interface ipv4 add address \"{}\" {} {} store=active",
            checked(port)?,
            server_address(),
            mask()
        ))
    }

    /// Deletes from the active store, the only one the address was added to.
    pub fn remove_command(port: &str) -> io::Result<String> {
        Ok(format!(
            "interface ipv4 delete address \"{}\" {} store=active",
            checked(port)?,
            server_address()
        ))
    }

    /// The name is quoted on the `netsh` command line, so it cannot hold a quote.
    fn checked(port: &str) -> io::Result<&str> {
        if !port.is_empty() && !port.contains('"') {
            Ok(port)
        } else {
            Err(super::bad_port(port))
        }
    }

    /// Runs `netsh <parameters>` elevated, after Windows asks for approval.
    pub fn run_elevated(parameters: &str) -> io::Result<()> {
        elevate("netsh.exe", parameters)
    }

    pub fn run_elevated_shell(command: &str) -> io::Result<()> {
        elevate("cmd.exe", &format!("/s /c \"{command}\""))
    }

    /// Runs `netsh <parameters>` as this process. The elevated helper is
    /// already an administrator.
    pub fn run_direct(parameters: &str) -> io::Result<()> {
        let output = Command::new("cmd")
            .args(["/c", &format!("netsh {parameters}")])
            .output()?;
        if output.status.success() {
            Ok(())
        } else {
            let message = String::from_utf8_lossy(&output.stderr);
            let message = message.trim();
            if message.is_empty() {
                Err(io::Error::other(format!(
                    "netsh exited with {}",
                    output.status
                )))
            } else {
                Err(io::Error::other(message.to_string()))
            }
        }
    }

    /// Runs `file parameters` elevated, after Windows asks for approval.
    fn elevate(file: &str, parameters: &str) -> io::Result<()> {
        let verb = wide("runas");
        let file = wide(file);
        let arguments = wide(parameters);
        // SAFETY: an all-zero SHELLEXECUTEINFOW is a valid empty request.
        let mut info: SHELLEXECUTEINFOW = unsafe { mem::zeroed() };
        info.cbSize = mem::size_of::<SHELLEXECUTEINFOW>() as u32;
        info.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC;
        info.lpVerb = verb.as_ptr();
        info.lpFile = file.as_ptr();
        info.lpParameters = arguments.as_ptr();
        info.nShow = SW_HIDE;
        // SAFETY: the strings `info` points to outlive the call.
        if unsafe { ShellExecuteExW(&mut info) } == 0 {
            return Err(io::Error::last_os_error());
        }
        if info.hProcess.is_null() {
            return Err(io::Error::other("the elevated program did not start"));
        }
        let mut code = 1;
        // SAFETY: `hProcess` is the open process handle ShellExecuteExW
        // returned, and it is closed exactly once.
        unsafe {
            WaitForSingleObject(info.hProcess, INFINITE);
            GetExitCodeProcess(info.hProcess, &mut code);
            CloseHandle(info.hProcess);
        }
        if code == 0 {
            Ok(())
        } else {
            Err(io::Error::other(format!(
                "netsh {parameters} exited with code {code}"
            )))
        }
    }

    fn wide(text: &str) -> Vec<u16> {
        OsStr::new(text).encode_wide().chain(Some(0)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::platform::*;
    use std::fs;
    use std::io;
    use std::path::Path;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::thread;
    use std::time::{Duration, Instant};

    #[test]
    fn the_elevated_helper_binds_the_device_tftp_port() {
        let bind = crate::tftp::device_bind();
        assert_eq!(bind.to_string(), "192.0.2.1:69");
        let exe = if cfg!(windows) {
            r"C:\Program Files\bench-app.exe"
        } else {
            "/Applications/Cisco CBT App.app/Contents/MacOS/bench"
        };
        let job = if cfg!(windows) {
            r"C:\bench-job"
        } else {
            "/tmp/bench-job"
        };
        let command = super::elevated_server_command(Path::new(exe), bind, Path::new(job));
        let expected = if cfg!(windows) {
            r#""C:\Program Files\bench-app.exe" --tftp-serve 192.0.2.1:69 "C:\bench-job""#
        } else {
            "'/Applications/Cisco CBT App.app/Contents/MacOS/bench' --tftp-serve 192.0.2.1:69 '/tmp/bench-job'"
        };
        assert_eq!(command, expected);
    }

    #[test]
    fn a_helper_bind_error_is_a_bind_failure() {
        let job = std::env::temp_dir().join(format!("bench-tftp-wait-{}", std::process::id()));
        let _ = fs::remove_dir_all(&job);
        fs::create_dir_all(&job).unwrap();
        let posted = job.clone();
        let helper = thread::spawn(move || {
            fs::write(posted.join("error"), "bind\nAddress not available").unwrap();
            Err(io::Error::other("helper exited"))
        });
        let server = super::ImageServer {
            job: job.clone(),
            helper: Some(helper),
        };
        let failure = server.wait_until_listening().unwrap_err();
        assert_eq!(
            failure,
            super::StartFailure::Bind("Address not available".into())
        );
        let _ = server.finish();
    }

    #[test]
    fn the_helper_receives_a_copy_of_the_picked_image() {
        let src = std::env::temp_dir().join(format!("bench-image-src-{}", std::process::id()));
        let _ = fs::remove_dir_all(&src);
        fs::create_dir_all(&src).unwrap();
        let picked = src.join("closet.17.12.05.bin");
        fs::write(&picked, b"firmware-bytes").unwrap();
        let job = super::create_job("en5", &picked, "closet.17.12.05.bin").unwrap();
        let handed = fs::read_to_string(job.join("image")).unwrap();
        let handed = Path::new(handed.trim());
        assert_eq!(handed, job.join("firmware"));
        assert_ne!(handed, picked);
        assert_eq!(fs::read(handed).unwrap(), b"firmware-bytes");
        fs::write(&picked, b"changed").unwrap();
        assert_eq!(fs::read(handed).unwrap(), b"firmware-bytes");
        let missing = src.join("missing.bin");
        assert!(super::create_job("en5", &missing, "missing.bin").is_err());
        let _ = fs::remove_dir_all(&job);
        let _ = fs::remove_dir_all(&src);
    }

    #[test]
    fn dropping_the_image_server_stops_the_helper() {
        let job = std::env::temp_dir().join(format!("bench-tftp-drop-{}", std::process::id()));
        let _ = fs::remove_dir_all(&job);
        fs::create_dir_all(&job).unwrap();
        let posted = job.clone();
        let saw_stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&saw_stop);
        let helper = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(2);
            loop {
                if posted.join("stop").exists() {
                    thread::sleep(Duration::from_millis(50));
                    flag.store(true, Ordering::SeqCst);
                    return Ok(());
                }
                if Instant::now() >= deadline {
                    return Err(io::Error::other("the stop file never appeared"));
                }
                thread::sleep(Duration::from_millis(20));
            }
        });
        drop(super::ImageServer {
            job,
            helper: Some(helper),
        });
        assert!(saw_stop.load(Ordering::SeqCst));
    }

    #[test]
    fn closing_the_window_stops_the_active_helper() {
        let job = std::env::temp_dir().join(format!("bench-tftp-quit-{}", std::process::id()));
        let _ = fs::remove_dir_all(&job);
        fs::create_dir_all(&job).unwrap();
        let posted = job.clone();
        let finished = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&finished);
        let helper = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(2);
            loop {
                if posted.join("stop").exists() {
                    flag.store(true, Ordering::SeqCst);
                    return Ok(());
                }
                if Instant::now() >= deadline {
                    return Err(io::Error::other("the stop file never appeared"));
                }
                thread::sleep(Duration::from_millis(20));
            }
        });
        super::remember_active(job.clone(), Arc::clone(&finished));
        super::stop_active_image_server();
        assert!(finished.load(Ordering::SeqCst));
        assert!(helper.join().unwrap().is_ok());
        super::forget_active(&job);
        let _ = fs::remove_dir_all(&job);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_adds_and_removes_the_server_alias_with_ifconfig() {
        assert_eq!(
            add_command("en5").unwrap(),
            "ifconfig en5 inet 192.0.2.1 netmask 255.255.255.252 alias"
        );
        assert_eq!(
            remove_command("en5").unwrap(),
            "ifconfig en5 inet 192.0.2.1 -alias"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_refuses_a_port_name_a_shell_would_read_as_more() {
        for port in ["", "en5; reboot", "en5\" & \"x", "en 5"] {
            assert_eq!(
                add_command(port).unwrap_err().kind(),
                std::io::ErrorKind::InvalidInput
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_adds_and_removes_the_server_address_with_netsh() {
        assert_eq!(
            add_command("Ethernet 2").unwrap(),
            "interface ipv4 add address \"Ethernet 2\" 192.0.2.1 255.255.255.252 store=active"
        );
        assert_eq!(
            remove_command("Ethernet 2").unwrap(),
            "interface ipv4 delete address \"Ethernet 2\" 192.0.2.1 store=active"
        );
        assert!(add_command("Ethernet \"2\"").is_err());
    }
}
