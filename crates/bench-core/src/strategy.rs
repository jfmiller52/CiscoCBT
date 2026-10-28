//! Per-family CLI strings and pass checks. The runner is not wired here yet.

use crate::family::DeviceFamily;
use crate::transfer::LinkKind;

/// Commands and output checks for one device family.
pub trait FamilyStrategy: Send + Sync {
    fn assign_address_commands(&self, kind: LinkKind, interface: &str) -> Vec<String>;
    fn clear_address_commands(&self, interface: &str) -> Vec<String>;
    fn copy_tftp_command(&self, filename: &str) -> String;
    fn install_and_reload_commands(&self, image_name: &str) -> Vec<String>;
    fn show_version_command(&self) -> &'static str;
    fn show_running_config_command(&self) -> &'static str;
    fn show_uptime_command(&self) -> &'static str;
    fn list_interfaces_command(&self) -> &'static str;
    fn parse_interface_names(&self, output: &str) -> Vec<String>;
    fn port_passed(&self, kind: LinkKind, port: &str, status_output: &str) -> bool;
    fn port_status_command(&self, kind: LinkKind, port: &str) -> String;
    fn save_startup_command(&self) -> &'static str;
}

/// Strategy for `family`. Access points are owned by Task 6.
pub fn strategy_for(family: DeviceFamily) -> &'static dyn FamilyStrategy {
    match family {
        DeviceFamily::IosXe => &crate::strategy_iosxe::IOS_XE,
        DeviceFamily::Nxos => &crate::strategy_nxos::NX_OS,
        DeviceFamily::AccessPoint => todo!("Task 6"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transfer::{
        assign_address_commands, clear_address_commands, copy_tftp_command, save_startup_command,
    };
    use crate::uptime::Uptime;

    fn ios() -> &'static dyn FamilyStrategy {
        strategy_for(DeviceFamily::IosXe)
    }

    fn nx() -> &'static dyn FamilyStrategy {
        strategy_for(DeviceFamily::Nxos)
    }

    #[test]
    fn iosxe_copy_tftp_command_matches_transfer() {
        let filename = "cat9k_iosxe.17.12.05.SPA.bin";
        assert_eq!(
            ios().copy_tftp_command(filename),
            copy_tftp_command(filename)
        );
        assert_eq!(
            ios().copy_tftp_command(filename),
            "copy tftp://192.0.2.1/cat9k_iosxe.17.12.05.SPA.bin flash:"
        );
    }

    #[test]
    fn iosxe_assign_address_matches_transfer_for_a_switch_port() {
        let interface = "GigabitEthernet1/0/1";
        assert_eq!(
            ios().assign_address_commands(LinkKind::Switch, interface),
            assign_address_commands(LinkKind::Switch, interface)
        );
    }

    #[test]
    fn iosxe_assign_address_matches_transfer_for_a_router_port() {
        let interface = "GigabitEthernet0/0/0";
        assert_eq!(
            ios().assign_address_commands(LinkKind::Router, interface),
            assign_address_commands(LinkKind::Router, interface)
        );
    }

    #[test]
    fn iosxe_assign_address_matches_transfer_for_switch_management() {
        let interface = "Gi0/0";
        assert_eq!(
            ios().assign_address_commands(LinkKind::Switch, interface),
            assign_address_commands(LinkKind::Switch, interface)
        );
    }

    #[test]
    fn iosxe_clear_address_matches_transfer() {
        let interface = "GigabitEthernet1/0/1";
        assert_eq!(
            ios().clear_address_commands(interface),
            clear_address_commands(interface)
        );
    }

    #[test]
    fn iosxe_install_is_the_single_add_activate_commit_command() {
        assert_eq!(
            ios().install_and_reload_commands("cat9k_iosxe.17.12.05.SPA.bin"),
            vec!["install add file flash:cat9k_iosxe.17.12.05.SPA.bin activate commit".to_string()]
        );
    }

    #[test]
    fn iosxe_show_and_save_commands_match_the_runner() {
        assert_eq!(ios().show_version_command(), "show version");
        assert_eq!(ios().show_running_config_command(), "show running-config");
        assert_eq!(ios().show_uptime_command(), "show version | include uptime");
        assert_eq!(ios().list_interfaces_command(), "show ip interface brief");
        assert_eq!(ios().save_startup_command(), save_startup_command());
        assert_eq!(
            ios().save_startup_command(),
            "copy running-config startup-config"
        );
    }

    #[test]
    fn iosxe_port_status_commands_match_the_runner() {
        assert_eq!(
            ios().port_status_command(LinkKind::Switch, "Gi1/0/3"),
            "show interface status"
        );
        assert_eq!(
            ios().port_status_command(LinkKind::Router, "GigabitEthernet0/0/0"),
            "show interfaces GigabitEthernet0/0/0"
        );
    }

    #[test]
    fn iosxe_switch_port_passes_only_when_connected() {
        let table = "Gi1/0/3 connected 1 a-full a-1000\nGi1/0/4 notconnect 1 auto auto\n";
        assert!(ios().port_passed(LinkKind::Switch, "Gi1/0/3", table));
        assert!(!ios().port_passed(LinkKind::Switch, "Gi1/0/4", table));
    }

    #[test]
    fn iosxe_switch_status_column_ignores_a_description() {
        let table = "Port         Name               Status       Vlan       Duplex  Speed Type\nGi1/0/5      connected to core  notconnect   1            auto   auto 10/100/1000BaseTX\n";
        assert!(!ios().port_passed(LinkKind::Switch, "Gi1/0/5", table));
    }

    #[test]
    fn iosxe_router_port_passes_when_line_protocol_is_up() {
        let up = "GigabitEthernet0/0/0 is up, line protocol is up";
        let down = "GigabitEthernet0/0/1 is up, line protocol is down";
        assert!(ios().port_passed(LinkKind::Router, "GigabitEthernet0/0/0", up));
        assert!(!ios().port_passed(LinkKind::Router, "GigabitEthernet0/0/1", down));
    }

    #[test]
    fn iosxe_interface_names_are_the_brief_first_column() {
        let brief = "\
Interface              IP-Address      OK? Method Status                Protocol
Vlan1                  unassigned      YES NVRAM  up                    up
GigabitEthernet0/0     unassigned      YES NVRAM  up                    up
GigabitEthernet1/0/1   unassigned      YES unset  down                  down
";
        assert_eq!(
            ios().parse_interface_names(brief),
            vec![
                "Vlan1".to_string(),
                "GigabitEthernet0/0".to_string(),
                "GigabitEthernet1/0/1".to_string(),
            ]
        );
    }

    #[test]
    fn nxos_copy_tftp_uses_bootflash_without_a_management_vrf() {
        assert_eq!(
            nx().copy_tftp_command("nxos.10.3.6.bin"),
            "copy tftp://192.0.2.1/nxos.10.3.6.bin bootflash:nxos.10.3.6.bin"
        );
    }

    #[test]
    fn nxos_assign_address_is_routed_without_switchport_or_tftp_source() {
        assert_eq!(
            nx().assign_address_commands(LinkKind::Switch, "Ethernet1/1"),
            vec![
                "configure terminal".to_string(),
                "interface Ethernet1/1".to_string(),
                "ip address 192.0.2.2 255.255.255.252".to_string(),
                "no shutdown".to_string(),
                "end".to_string(),
            ]
        );
    }

    #[test]
    fn nxos_router_kind_uses_the_same_address_commands() {
        assert_eq!(
            nx().assign_address_commands(LinkKind::Router, "Ethernet1/2"),
            nx().assign_address_commands(LinkKind::Switch, "Ethernet1/2")
        );
    }

    #[test]
    fn nxos_clear_address_removes_the_ip_and_leaves_the_interface_up() {
        assert_eq!(
            nx().clear_address_commands("Ethernet1/1"),
            vec![
                "configure terminal".to_string(),
                "interface Ethernet1/1".to_string(),
                "no ip address".to_string(),
                "end".to_string(),
            ]
        );
    }

    #[test]
    fn nxos_install_all_reloads_in_that_command() {
        assert_eq!(
            nx().install_and_reload_commands("nxos.10.3.6.bin"),
            vec!["install all nxos bootflash:nxos.10.3.6.bin non-interactive".to_string()]
        );
    }

    #[test]
    fn nxos_show_and_save_commands() {
        assert_eq!(nx().show_version_command(), "show version");
        assert_eq!(nx().show_running_config_command(), "show running-config");
        assert_eq!(nx().show_uptime_command(), "show version | include uptime");
        assert_eq!(nx().list_interfaces_command(), "show interface brief");
        assert_eq!(
            nx().save_startup_command(),
            "copy running-config startup-config"
        );
    }

    #[test]
    fn nxos_port_status_command_is_show_interface() {
        assert_eq!(
            nx().port_status_command(LinkKind::Switch, "Ethernet1/1"),
            "show interface Ethernet1/1"
        );
        assert_eq!(
            nx().port_status_command(LinkKind::Router, "Eth1/2"),
            "show interface Eth1/2"
        );
    }

    /// `show interface` on NX-OS. Operational line is up and admin state is up.
    const NXOS_UP: &str = "\
Ethernet1/1 is up
admin state is up, Dedicated Interface
  Hardware: 100/1000/10000 Ethernet, address: 5254.0012.3456 (bia 5254.0012.3456)
  MTU 1500 bytes, BW 10000000 Kbit, DLY 10 usec
";

    /// Admin state stays up; the link itself is down.
    const NXOS_DOWN: &str = "\
Ethernet1/2 is down (Link not connected)
admin state is up, Dedicated Interface
  Hardware: 100/1000/10000 Ethernet, address: 5254.0012.3457 (bia 5254.0012.3457)
";

    const NXOS_ADMIN_DOWN: &str = "\
Ethernet1/3 is down (Administratively down)
admin state is down, Dedicated Interface
";

    #[test]
    fn nxos_port_passed_when_admin_and_operational_state_are_up() {
        assert!(nx().port_passed(LinkKind::Switch, "Ethernet1/1", NXOS_UP));
        assert!(nx().port_passed(LinkKind::Switch, "Eth1/1", NXOS_UP));
        assert!(nx().port_passed(LinkKind::Router, "Ethernet1/1", NXOS_UP));
    }

    #[test]
    fn nxos_port_failed_when_the_link_is_down() {
        assert!(!nx().port_passed(LinkKind::Switch, "Ethernet1/2", NXOS_DOWN));
        assert!(!nx().port_passed(LinkKind::Switch, "Eth1/2", NXOS_DOWN));
    }

    #[test]
    fn nxos_port_failed_when_admin_state_is_down() {
        assert!(!nx().port_passed(LinkKind::Switch, "Ethernet1/3", NXOS_ADMIN_DOWN));
    }

    #[test]
    fn nxos_port_failed_without_both_up_lines() {
        assert!(!nx().port_passed(LinkKind::Switch, "Ethernet1/1", "Ethernet1/1 is up\n"));
        assert!(!nx().port_passed(
            LinkKind::Switch,
            "Ethernet1/1",
            "admin state is up, Dedicated Interface\n"
        ));
    }

    #[test]
    fn nxos_interface_brief_keeps_ethernet_and_mgmt_once() {
        let brief = "\
--------------------------------------------------------------------------------
Port   VRF          Status IP Address                              Speed    MTU
--------------------------------------------------------------------------------
mgmt0  --           up     192.0.2.2                               1000     1500

--------------------------------------------------------------------------------
Ethernet      VLAN    Type Mode   Status  Reason                   Speed     Port
Interface                                                                    Ch #
--------------------------------------------------------------------------------
Eth1/1        1       eth  routed up      none                       10G(D)  --
Eth1/2        1       eth  access down    Link not connected         auto(D) --
Ethernet1/3   --      eth  routed up      none                       10G(D)  --

--------------------------------------------------------------------------------
Port-channel VLAN    Type Mode   Status  Reason                    Speed   Protocol
Interface
--------------------------------------------------------------------------------
Po1          1       eth  trunk  down    No operational members      auto    none

--------------------------------------------------------------------------------
Interface     Status     Description
--------------------------------------------------------------------------------
loopback0     up
Vlan1         down
port-channel1 down
mgmt0         --
";
        assert_eq!(
            nx().parse_interface_names(brief),
            vec![
                "mgmt0".to_string(),
                "Eth1/1".to_string(),
                "Eth1/2".to_string(),
                "Ethernet1/3".to_string(),
            ]
        );
    }

    #[test]
    fn nxos_uptime_strips_parenthesized_plurals_before_the_ios_parser() {
        let output = "Kernel uptime is 1 day(s), 2 hour(s), 3 minute(s), 12 second(s)";
        assert_eq!(
            crate::strategy_nxos::parse_uptime(output),
            vec![Uptime {
                minutes_total: 24 * 60 + 2 * 60 + 3
            }]
        );
    }

    #[test]
    #[should_panic(expected = "Task 6")]
    fn access_point_strategy_is_left_for_task_6() {
        strategy_for(DeviceFamily::AccessPoint);
    }
}
