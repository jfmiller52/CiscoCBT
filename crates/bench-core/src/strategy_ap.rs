//! CAPWAP access-point commands for the bench TFTP link (192.0.2.2/30 toward 192.0.2.1).
//!
//! The uplink is a routed ethernet port. On current CAPWAP AP CLI that port is
//! `GigabitEthernet0` (older APs may print `FastEthernet0`). Address assignment
//! does not send `no switchport` or `ip tftp source-interface`. It also does
//! not send `capwap ap ip`: that command is the permanent CAPWAP address, and
//! the runner applies it later. The temporary address is on-link to the TFTP
//! server, so no default gateway is set.
//!
//! # Install
//!
//! One command: `archive download-sw /overwrite /reload flash:{file}`.
//! The TFTP copy has already placed `{file}` on flash. `/overwrite` installs
//! that flash image in place of the running image, and `/reload` reloads the
//! AP so the new image becomes active. The list does not append a separate
//! `reload`. `ap-type` is not sent: this upgrades an AP that is already in
//! CAPWAP mode, and mode conversion is not part of the firmware upgrade.
//! Install rejection handling stays in the runner.
//!
//! # Interfaces
//!
//! `show ip interface brief`. Only ethernet uplinks are returned
//! (`GigabitEthernet`, `FastEthernet`, multi-gig ethernet names, and `wired`),
//! so the first physical name is `GigabitEthernet0` rather than `BVI1` or
//! `Dot11Radio`.
//!
//! # Ports
//!
//! The runner does not walk ports on an access point. `port_passed` is false
//! and `port_status_command` is empty. `show running-config` is unused on the
//! AP path.

use crate::strategy::FamilyStrategy;
use crate::transfer::{device_address, mask, server_address, LinkKind};

pub(crate) struct AccessPointStrategy;

pub(crate) static ACCESS_POINT: AccessPointStrategy = AccessPointStrategy;

impl FamilyStrategy for AccessPointStrategy {
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
        format!("copy tftp://{server}/{filename} flash:{filename}")
    }

    fn install_and_reload_commands(&self, image_name: &str) -> Vec<String> {
        vec![format!(
            "archive download-sw /overwrite /reload flash:{image_name}"
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
        "show ip interface brief"
    }

    fn parse_interface_names(&self, output: &str) -> Vec<String> {
        output
            .lines()
            .filter_map(|line| line.split_whitespace().next())
            .filter(|token| is_ethernet_uplink(token))
            .map(str::to_string)
            .collect()
    }

    fn port_passed(&self, _kind: LinkKind, _port: &str, _status_output: &str) -> bool {
        false
    }

    fn port_status_command(&self, _kind: LinkKind, _port: &str) -> String {
        String::new()
    }

    fn save_startup_command(&self) -> &'static str {
        "copy running-config startup-config"
    }
}

/// Ethernet uplinks on CAPWAP AP `show ip interface brief`. A digit must follow
/// the family name so header words are skipped.
fn is_ethernet_uplink(token: &str) -> bool {
    let lower = token.to_ascii_lowercase();
    const PREFIXES: &[&str] = &[
        "gigabitethernet",
        "tengigabitethernet",
        "twogigabitethernet",
        "fivegigabitethernet",
        "fastethernet",
        "ethernet",
        "wired",
    ];
    PREFIXES.iter().any(|prefix| {
        lower
            .strip_prefix(prefix)
            .is_some_and(|rest| rest.chars().next().is_some_and(|c| c.is_ascii_digit()))
    })
}
