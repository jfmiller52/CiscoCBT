use crate::run::CheckStatus;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportInput {
    pub hostname: String,
    pub serial: String,
    pub config_file: String,
    pub configuration: CheckStatus,
    pub firmware_file: Option<String>,
    pub running_version: Option<String>,
    pub firmware: CheckStatus,
    pub burn_in_hours: Option<u32>,
    pub uptime_line: Option<String>,
    pub burn_in: CheckStatus,
    pub ports: Vec<(String, CheckStatus)>,
    pub capwap: CheckStatus,
    pub capwap_ip: Option<String>,
    pub capwap_mask: Option<String>,
    pub capwap_gateway: Option<String>,
}

/// Render the plain-text validation record the technician saves.
pub fn export_text(input: &ExportInput) -> String {
    let mut out = String::new();
    out.push_str("Device validation record\n");
    out.push_str(&format!("Chassis serial number: {}\n", input.serial));
    out.push_str(&format!("Host name: {}\n", input.hostname));

    if input.configuration != CheckStatus::NotChosen {
        out.push_str("\nConfiguration\n");
        out.push_str(&format!("Configuration file: {}\n", input.config_file));
        push_status(
            &mut out,
            "Running config contains the file's commands",
            &input.configuration,
        );
    }

    if let Some(file) = &input.firmware_file {
        out.push_str("\nFirmware\n");
        out.push_str(&format!("Firmware image file: {file}\n"));
        match &input.running_version {
            Some(version) => out.push_str(&format!("Running version: {version}\n")),
            None => out.push_str("Running version: not reported\n"),
        }
        push_status(&mut out, "Firmware result", &input.firmware);
    }

    if input.capwap != CheckStatus::NotChosen || input.capwap_ip.is_some() {
        out.push_str("\nCAPWAP\n");
        out.push_str(&format!(
            "CAPWAP IP: {}\n",
            input.capwap_ip.as_deref().unwrap_or("not reported")
        ));
        out.push_str(&format!(
            "Subnet mask: {}\n",
            input.capwap_mask.as_deref().unwrap_or("not reported")
        ));
        out.push_str(&format!(
            "Default gateway: {}\n",
            input.capwap_gateway.as_deref().unwrap_or("not reported")
        ));
        push_status(&mut out, "CAPWAP result", &input.capwap);
    }

    if input.burn_in != CheckStatus::NotChosen {
        out.push_str("\nBurn-in\n");
        if let Some(hours) = input.burn_in_hours {
            let unit = if hours == 1 { "hour" } else { "hours" };
            out.push_str(&format!("Burn-in hours entered: {hours} {unit}\n"));
        }
        if let Some(line) = &input.uptime_line {
            out.push_str(&format!("Uptime: {line}\n"));
        }
        push_status(&mut out, "Burn-in result", &input.burn_in);
    }

    if !input.ports.is_empty() {
        out.push_str("\nPorts\n");
        for (name, status) in &input.ports {
            push_status(&mut out, name, status);
        }
    }

    out
}

/// Write `label: word`, followed by indented detail lines for a failure.
fn push_status(out: &mut String, label: &str, status: &CheckStatus) {
    let word = match status {
        CheckStatus::Passed => "Passed",
        CheckStatus::Failed { .. } => "Failed",
        CheckStatus::NotChosen | CheckStatus::NotFinished => "Not finished yet",
    };
    out.push_str(&format!("{label}: {word}\n"));
    if let CheckStatus::Failed { detail } = status {
        for line in detail.lines() {
            out.push_str(&format!("    {line}\n"));
        }
    }
}

/// Find the chassis serial number: first non-empty `SN:` in `show inventory`,
/// else `System Serial Number` in `show version`, else `Processor board ID`.
pub fn chassis_serial(show_inventory: &str, show_version: &str) -> Option<String> {
    inventory_serial(show_inventory)
        .or_else(|| value_after(show_version, "System Serial Number", true))
        .or_else(|| value_after(show_version, "Processor board ID", false))
}

fn inventory_serial(text: &str) -> Option<String> {
    let mut rest = text;
    while let Some(at) = rest.find("SN:") {
        let before = rest[..at].chars().next_back();
        let after = &rest[at + 3..];
        if !before.is_some_and(|c| c.is_alphanumeric()) {
            if let Some(value) = leading_value(after) {
                return Some(value);
            }
        }
        rest = after;
    }
    None
}

/// First value following `label` (optionally through a `:`) that is non-empty.
fn value_after(text: &str, label: &str, allow_colon: bool) -> Option<String> {
    let mut rest = text;
    while let Some(at) = rest.find(label) {
        let after = &rest[at + label.len()..];
        let after_sep = if allow_colon {
            after.trim_start().trim_start_matches(':')
        } else {
            after
        };
        if let Some(value) = leading_value(after_sep) {
            return Some(value);
        }
        rest = after;
    }
    None
}

/// Token at the start of `text` (after spaces), ending at whitespace or comma.
fn leading_value(text: &str) -> Option<String> {
    let token: String = text
        .trim_start_matches([' ', '\t'])
        .chars()
        .take_while(|c| !c.is_whitespace() && *c != ',')
        .collect();
    (!token.is_empty()).then_some(token)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> ExportInput {
        ExportInput {
            hostname: "sw1".into(),
            serial: "FOC1".into(),
            config_file: "a.cfg".into(),
            configuration: CheckStatus::Passed,
            firmware_file: None,
            running_version: None,
            firmware: CheckStatus::NotChosen,
            burn_in_hours: None,
            uptime_line: None,
            burn_in: CheckStatus::NotChosen,
            ports: vec![],
            capwap: CheckStatus::NotChosen,
            capwap_ip: None,
            capwap_mask: None,
            capwap_gateway: None,
        }
    }

    #[test]
    fn access_point_export_omits_configuration_and_lists_passed_capwap() {
        let text = export_text(&ExportInput {
            configuration: CheckStatus::NotChosen,
            capwap: CheckStatus::Passed,
            capwap_ip: Some("10.1.1.10".into()),
            capwap_mask: Some("255.255.255.0".into()),
            capwap_gateway: Some("10.1.1.1".into()),
            ..base()
        });
        assert!(!text.contains("Configuration\n"));
        assert!(!text.contains("Configuration file:"));
        assert!(!text.contains("Running config contains"));
        assert!(text.contains("\nCAPWAP\n"));
        assert!(text.contains("CAPWAP IP: 10.1.1.10\n"));
        assert!(text.contains("Subnet mask: 255.255.255.0\n"));
        assert!(text.contains("Default gateway: 10.1.1.1\n"));
        assert!(text.contains("CAPWAP result: Passed\n"));
        assert!(text.contains("Device validation record\n"));
        assert!(text.contains("Chassis serial number: FOC1\n"));
    }

    #[test]
    fn failed_capwap_indents_its_detail() {
        let text = export_text(&ExportInput {
            configuration: CheckStatus::NotChosen,
            capwap: CheckStatus::Failed {
                detail: "Missing default gateway\nshow capwap client config\nIP 10.9.9.9".into(),
            },
            capwap_ip: Some("10.1.1.10".into()),
            capwap_mask: Some("255.255.255.0".into()),
            capwap_gateway: Some("10.1.1.1".into()),
            ..base()
        });
        assert!(text.contains(
            "CAPWAP result: Failed\n    Missing default gateway\n    show capwap client config\n    IP 10.9.9.9\n"
        ));
    }

    #[test]
    fn capwap_section_appears_when_only_the_ip_is_set() {
        let text = export_text(&ExportInput {
            capwap: CheckStatus::NotChosen,
            capwap_ip: Some("10.0.0.5".into()),
            ..base()
        });
        assert!(text.contains("\nCAPWAP\n"));
        assert!(text.contains("CAPWAP IP: 10.0.0.5\n"));
        assert!(text.contains("Subnet mask: not reported\n"));
        assert!(text.contains("Default gateway: not reported\n"));
        assert!(text.contains("CAPWAP result: Not finished yet\n"));
        assert!(text.contains("\nConfiguration\n"));
    }

    #[test]
    fn failed_configuration_lists_missing_commands_indented() {
        let text = export_text(&ExportInput {
            configuration: CheckStatus::Failed {
                detail: "Missing commands:\nhostname sw1\nntp server 10.0.0.1".into(),
            },
            ..base()
        });
        assert!(text.contains("contains the file's commands: Failed\n"));
        assert!(text.contains("    Missing commands:\n    hostname sw1\n    ntp server 10.0.0.1\n"));
    }

    #[test]
    fn unfinished_burn_in_and_ports_say_not_finished_yet() {
        let text = export_text(&ExportInput {
            burn_in_hours: Some(1),
            burn_in: CheckStatus::NotFinished,
            ports: vec![
                ("Gi1/0/1".into(), CheckStatus::NotFinished),
                (
                    "Gi1/0/2".into(),
                    CheckStatus::Failed {
                        detail: "no link".into(),
                    },
                ),
            ],
            ..base()
        });
        assert!(text.contains("Burn-in hours entered: 1 hour\n"));
        assert!(text.contains("Burn-in result: Not finished yet\n"));
        assert!(text.contains("Gi1/0/1: Not finished yet\n"));
        assert!(text.contains("Gi1/0/2: Failed\n    no link\n"));
    }

    #[test]
    fn optional_sections_are_omitted_when_not_chosen() {
        let text = export_text(&base());
        assert!(!text.contains("Firmware"));
        assert!(!text.contains("Burn-in"));
        assert!(!text.contains("Ports"));
        assert!(!text.contains("CAPWAP"));
        assert!(!text.contains('\r'));
        assert!(text.is_ascii());
    }

    #[test]
    fn firmware_section_names_image_and_running_version() {
        let text = export_text(&ExportInput {
            firmware_file: Some("cat9k_iosxe.17.09.04.SPA.bin".into()),
            running_version: Some("17.09.04".into()),
            firmware: CheckStatus::Passed,
            ..base()
        });
        assert!(text.contains("Firmware image file: cat9k_iosxe.17.09.04.SPA.bin\n"));
        assert!(text.contains("Running version: 17.09.04\n"));
        assert!(text.contains("Firmware result: Passed\n"));
    }

    #[test]
    fn failed_firmware_prints_its_detail_indented() {
        let text = export_text(&ExportInput {
            firmware_file: Some("cat9k_iosxe.17.09.04.SPA.bin".into()),
            running_version: Some("17.06.03".into()),
            firmware: CheckStatus::Failed {
                detail: "Running 17.06.03, expected 17.09.04".into(),
            },
            ..base()
        });
        assert!(text.contains("Firmware result: Failed\n    Running 17.06.03, expected 17.09.04\n"));
    }

    #[test]
    fn unfinished_firmware_says_not_finished_yet() {
        let text = export_text(&ExportInput {
            firmware_file: Some("image.bin".into()),
            firmware: CheckStatus::NotFinished,
            ..base()
        });
        assert!(text.contains("Firmware result: Not finished yet\n"));
    }

    #[test]
    fn serial_skips_empty_inventory_values_and_stops_at_comma() {
        let inventory =
            "NAME: \"1\", DESCR: \"x\"\nPID: A , VID: V01 , SN: \nPID: B , VID: V01 , SN: FOC9,\n";
        assert_eq!(chassis_serial(inventory, "").as_deref(), Some("FOC9"));
    }

    #[test]
    fn serial_uses_system_serial_number_before_processor_board_id() {
        let version = "Processor board ID FCW1\nSystem Serial Number     : FOC7\n";
        assert_eq!(chassis_serial("", version).as_deref(), Some("FOC7"));
        assert_eq!(
            chassis_serial("", "System Serial Number FOC8").as_deref(),
            Some("FOC8")
        );
    }

    #[test]
    fn serial_is_none_when_absent() {
        assert_eq!(chassis_serial("nothing", "nothing"), None);
    }

    #[test]
    fn export_quotes_serial_uptime_and_ports() {
        let text = export_text(&ExportInput {
            hostname: "closet-2-sw1".into(),
            serial: "FOC2345L0AB".into(),
            config_file: "closet-access.cfg".into(),
            configuration: CheckStatus::Passed,
            firmware_file: None,
            running_version: None,
            firmware: CheckStatus::NotChosen,
            burn_in_hours: Some(4),
            uptime_line: Some("closet-2-sw1 uptime is 4 hours, 6 minutes".into()),
            burn_in: CheckStatus::Passed,
            ports: vec![("Gi1/0/1".into(), CheckStatus::Passed)],
            capwap: CheckStatus::NotChosen,
            capwap_ip: None,
            capwap_mask: None,
            capwap_gateway: None,
        });
        assert!(text.contains("FOC2345L0AB"));
        assert!(text.contains("closet-access.cfg"));
        assert!(text.contains("4 hours"));
        assert!(text.contains("closet-2-sw1 uptime is 4 hours, 6 minutes"));
        assert!(text.contains("Gi1/0/1"));
    }

    #[test]
    fn serial_comes_from_inventory_then_show_version() {
        let inventory = "PID: C9300-24T , VID: V01 , SN: FOC2345L0AB";
        assert_eq!(
            chassis_serial(inventory, "").as_deref(),
            Some("FOC2345L0AB")
        );
        assert_eq!(
            chassis_serial("", "Processor board ID FCW2145G0AB").as_deref(),
            Some("FCW2145G0AB")
        );
    }
}
