//! IOS-XE commands the bench already sends.
//!
//! Install is the single command `install add file flash:{image} activate commit`.
//! The session answers that install's prompt and waits for the reload it starts,
//! so this list does not add a separate `reload`.

use crate::ports::{router_port_passed, switch_port_passed};
use crate::strategy::FamilyStrategy;
use crate::transfer::{
    assign_address_commands, clear_address_commands, copy_tftp_command, save_startup_command,
    LinkKind,
};

pub(crate) struct IosXeStrategy;

pub(crate) static IOS_XE: IosXeStrategy = IosXeStrategy;

impl FamilyStrategy for IosXeStrategy {
    fn assign_address_commands(&self, kind: LinkKind, interface: &str) -> Vec<String> {
        assign_address_commands(kind, interface)
    }

    fn clear_address_commands(&self, interface: &str) -> Vec<String> {
        clear_address_commands(interface)
    }

    fn copy_tftp_command(&self, filename: &str) -> String {
        copy_tftp_command(filename)
    }

    fn install_and_reload_commands(&self, image_name: &str) -> Vec<String> {
        vec![format!(
            "install add file flash:{image_name} activate commit"
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
        // Same first-column read as the runner's private `interface_names`.
        output
            .lines()
            .filter(|line| !line.starts_with("Interface"))
            .filter_map(|line| line.split_whitespace().next())
            .map(String::from)
            .collect()
    }

    fn port_passed(&self, kind: LinkKind, port: &str, status_output: &str) -> bool {
        match kind {
            LinkKind::Switch => switch_port_passed(status_output, port),
            LinkKind::Router => router_port_passed(status_output, port),
        }
    }

    fn port_status_command(&self, kind: LinkKind, port: &str) -> String {
        match kind {
            LinkKind::Switch => "show interface status".to_string(),
            LinkKind::Router => format!("show interfaces {port}"),
        }
    }

    fn save_startup_command(&self) -> &'static str {
        save_startup_command()
    }
}
