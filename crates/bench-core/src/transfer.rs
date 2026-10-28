use crate::config_check::canonical_interface_name;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkKind {
    Switch,
    Router,
}

pub fn device_address() -> &'static str {
    "192.0.2.2"
}

pub fn server_address() -> &'static str {
    "192.0.2.1"
}

pub fn mask() -> &'static str {
    "255.255.255.252"
}

pub fn assign_address_commands(kind: LinkKind, interface: &str) -> Vec<String> {
    let ip_line = format!("ip address {} {}", device_address(), mask());
    let tftp_line = format!("ip tftp source-interface {interface}");

    let mut cmds = vec![
        "configure terminal".into(),
        format!("interface {interface}"),
    ];

    let needs_no_switchport = matches!(kind, LinkKind::Switch)
        && canonical_interface_name(interface) != "GigabitEthernet0/0";
    if needs_no_switchport {
        cmds.push("no switchport".into());
    }

    cmds.extend([
        ip_line,
        "no shutdown".into(),
        "exit".into(),
        tftp_line,
        "end".into(),
    ]);

    cmds
}

pub fn clear_address_commands(interface: &str) -> Vec<String> {
    vec![
        "configure terminal".into(),
        format!("interface {interface}"),
        "no ip address".into(),
        "exit".into(),
        "no ip tftp source-interface".into(),
        "end".into(),
    ]
}

pub fn copy_tftp_command(filename: &str) -> String {
    format!("copy tftp://{}/{} flash:", server_address(), filename)
}

pub fn save_startup_command() -> &'static str {
    "copy running-config startup-config"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn switch_assignment_is_routed_and_uses_the_documentation_prefix() {
        let cmds = assign_address_commands(LinkKind::Switch, "GigabitEthernet1/0/1");
        assert!(cmds.iter().any(|c| c == "no switchport"));
        assert!(cmds
            .iter()
            .any(|c| c == "ip address 192.0.2.2 255.255.255.252"));
        assert!(!cmds.iter().any(|c| c.contains("write")));
    }

    #[test]
    fn router_assignment_has_no_switchport_command() {
        let cmds = assign_address_commands(LinkKind::Router, "GigabitEthernet0/0/0");
        assert!(!cmds.iter().any(|c| c == "no switchport"));
        assert!(cmds
            .iter()
            .any(|c| c == "ip tftp source-interface GigabitEthernet0/0/0"));
    }

    #[test]
    fn switch_management_port_is_already_routed() {
        let cmds = assign_address_commands(LinkKind::Switch, "GigabitEthernet0/0");
        assert!(!cmds.iter().any(|c| c == "no switchport"));
        assert!(cmds
            .iter()
            .any(|c| c == "ip tftp source-interface GigabitEthernet0/0"));
    }

    #[test]
    fn copy_targets_the_laptop_address_and_flash() {
        assert_eq!(
            copy_tftp_command("cat9k_iosxe.17.12.05.SPA.bin"),
            "copy tftp://192.0.2.1/cat9k_iosxe.17.12.05.SPA.bin flash:"
        );
        assert_eq!(save_startup_command(), "copy running-config startup-config");
    }

    #[test]
    fn switch_management_port_abbreviation_skips_no_switchport() {
        let cmds = assign_address_commands(LinkKind::Switch, "Gi0/0");
        assert!(!cmds.iter().any(|c| c == "no switchport"));
        assert!(cmds.iter().any(|c| c == "interface Gi0/0"));
    }
}
