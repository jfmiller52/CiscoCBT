//! This computer's side of the image cable: its wired ethernet ports, and
//! the temporary TFTP server address on the chosen one.

use std::io;
use std::net::Ipv4Addr;

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
/// declined.
pub fn add_server_address(port: &str) -> io::Result<()> {
    if has_server_address(port) {
        return Ok(());
    }
    platform::run_elevated(&platform::add_command(port)?)
}

/// Takes the server address off `port` once an administrator approves.
pub fn remove_server_address(port: &str) -> io::Result<()> {
    platform::run_elevated(&platform::remove_command(port)?)
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
        let script = format!("do shell script \"{command}\" with administrator privileges");
        let output = Command::new("osascript").arg("-e").arg(script).output()?;
        if output.status.success() {
            Ok(())
        } else {
            let message = String::from_utf8_lossy(&output.stderr);
            Err(io::Error::other(message.trim().to_string()))
        }
    }
}

#[cfg(windows)]
mod platform {
    use std::ffi::OsStr;
    use std::io;
    use std::mem;
    use std::os::windows::ffi::OsStrExt;

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
        let verb = wide("runas");
        let file = wide("netsh.exe");
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
            return Err(io::Error::other("netsh did not start"));
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
