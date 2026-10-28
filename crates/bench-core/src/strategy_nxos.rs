//! NX-OS commands for the bench TFTP link (192.0.2.2/30 toward 192.0.2.1).
//!
//! Address assignment treats Ethernet as already routed, so it does not send
//! `no switchport`. It also does not send `ip tftp source-interface`: that
//! global command is IOS-XE. The copy stays on the default VRF, where the
//! temporary address lives, and does not add `vrf management`.
//!
//! # Install
//!
//! One command: `install all nxos bootflash:{file} non-interactive`.
//! `install all` reloads the switch as part of the upgrade, so the returned
//! list does not append a separate `reload`.
//!
//! # Port pass rule
//!
//! `show interface {port}` passes only when both are true:
//! the header line for that port is operationally up (`{port} is up`, and that
//! line does not contain `down`), and some line contains `admin state is up`.
//! `Ethernet` and `Eth` name the same port. [`LinkKind`] does not change the rule.
//!
//! # Uptime
//!
//! The show command is `show version | include uptime`. NX-OS prints units as
//! `day(s)`. [`parse_uptime`] strips `(s)` and then calls
//! [`parse_uptime_lines`](crate::uptime::parse_uptime_lines).

use crate::strategy::FamilyStrategy;
use crate::transfer::{device_address, mask, server_address, LinkKind};
use crate::uptime::{parse_uptime_lines, Uptime};

pub(crate) struct NxosStrategy;

pub(crate) static NX_OS: NxosStrategy = NxosStrategy;

/// Parses `show version | include uptime` after turning `day(s)` into `day`.
pub fn parse_uptime(output: &str) -> Vec<Uptime> {
    parse_uptime_lines(&output.replace("(s)", ""))
}

impl FamilyStrategy for NxosStrategy {
    fn assign_address_commands(&self, kind: LinkKind, interface: &str) -> Vec<String> {
        match kind {
            LinkKind::Switch | LinkKind::Router => vec![
                "configure terminal".to_string(),
                format!("interface {interface}"),
                format!("ip address {} {}", device_address(), mask()),
                "no shutdown".to_string(),
                "end".to_string(),
            ],
        }
    }

    fn clear_address_commands(&self, interface: &str) -> Vec<String> {
        vec![
            "configure terminal".to_string(),
            format!("interface {interface}"),
            "no ip address".to_string(),
            "end".to_string(),
        ]
    }

    fn copy_tftp_command(&self, filename: &str) -> String {
        let server = server_address();
        format!("copy tftp://{server}/{filename} bootflash:{filename}")
    }

    fn install_and_reload_commands(&self, image_name: &str) -> Vec<String> {
        vec![format!(
            "install all nxos bootflash:{image_name} non-interactive"
        )]
    }

    fn show_version_command(&self) -> &'static str {
        "show version"
    }

    fn show_running_config_command(&self) -> &'static str {
        "show running-config"
    }

    fn show_uptime_command(&self) -> &'static str {
        "show version | include uptime"
    }

    fn list_interfaces_command(&self) -> &'static str {
        "show interface brief"
    }

    fn parse_interface_names(&self, output: &str) -> Vec<String> {
        let mut names = Vec::new();
        for line in output.lines() {
            let Some(token) = line.split_whitespace().next() else {
                continue;
            };
            if !is_ethernet_or_mgmt(token) {
                continue;
            }
            if names
                .iter()
                .any(|kept: &String| kept.eq_ignore_ascii_case(token))
            {
                continue;
            }
            names.push(token.to_string());
        }
        names
    }

    fn port_passed(&self, kind: LinkKind, port: &str, status_output: &str) -> bool {
        match kind {
            LinkKind::Switch | LinkKind::Router => {
                header_operationally_up(status_output, port) && admin_state_is_up(status_output)
            }
        }
    }

    fn port_status_command(&self, kind: LinkKind, port: &str) -> String {
        match kind {
            LinkKind::Switch | LinkKind::Router => format!("show interface {port}"),
        }
    }

    fn save_startup_command(&self) -> &'static str {
        "copy running-config startup-config"
    }
}

fn is_ethernet_or_mgmt(token: &str) -> bool {
    let lower = token.to_ascii_lowercase();
    let rest = if let Some(rest) = lower.strip_prefix("ethernet") {
        rest
    } else if let Some(rest) = lower.strip_prefix("eth") {
        rest
    } else if let Some(rest) = lower.strip_prefix("management") {
        rest
    } else if let Some(rest) = lower.strip_prefix("mgmt") {
        rest
    } else {
        return false;
    };
    rest.chars().next().is_some_and(|c| c.is_ascii_digit())
}

/// `Ethernet1/1` and `Eth1/1` compare equal. Other names compare in lowercase.
fn port_key(name: &str) -> String {
    let lower = name.trim().to_ascii_lowercase();
    match lower.strip_prefix("ethernet") {
        Some(rest) => format!("eth{rest}"),
        None => lower,
    }
}

fn header_operationally_up(output: &str, port: &str) -> bool {
    let wanted = port_key(port);
    output.lines().any(|line| {
        let Some(first) = line.split_whitespace().next() else {
            return false;
        };
        if port_key(first) != wanted {
            return false;
        }
        let lower = line.to_ascii_lowercase();
        lower.contains(" is up") && !lower.contains("down")
    })
}

fn admin_state_is_up(output: &str) -> bool {
    output
        .lines()
        .any(|line| line.to_ascii_lowercase().contains("admin state is up"))
}
