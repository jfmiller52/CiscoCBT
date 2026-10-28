//! Pick physical interfaces from `show ip interface brief` names and decide
//! whether a port's link is up from `show interfaces` / `show interfaces status`.

use crate::config_check::canonical_interface_name;

/// Interface families that are not physical ports (compared case-insensitively
/// against the canonical name).
const NON_PHYSICAL_PREFIXES: [&str; 11] = [
    "loopback",
    "vlan",
    "tunnel",
    "virtual",
    "port-channel",
    "bdi",
    "nvi",
    "null",
    "dialer",
    "appgigabitethernet",
    "bluetooth",
];

/// Interface names that are physical ports, in the order given.
pub fn physical_interfaces(names: &[String]) -> Vec<String> {
    names.iter().filter(|name| is_physical(name)).cloned().collect()
}

/// The first physical interface in device order, if any.
pub fn first_physical(names: &[String]) -> Option<String> {
    names.iter().find(|name| is_physical(name)).cloned()
}

fn is_physical(name: &str) -> bool {
    let canonical = canonical_interface_name(name).to_ascii_lowercase();
    !NON_PHYSICAL_PREFIXES.iter().any(|prefix| canonical.starts_with(prefix))
}

/// Whether `port` shows as `connected` in `show interfaces status` output.
///
/// With a header row (first token `Port`, containing `Status`) the status is
/// the token at the header's `Status` column; without one, any token of the
/// port's row that is exactly `connected` passes. A missing port fails.
pub fn switch_port_passed(show_interface_status: &str, port: &str) -> bool {
    let wanted = canonical_interface_name(port.trim());
    let mut lines = show_interface_status.lines();
    let mut status_column = None;
    let mut rows: Vec<&str> = Vec::new();
    for line in lines.by_ref() {
        if let Some(column) = header_status_column(line) {
            status_column = Some(column);
            break;
        }
        rows.push(line);
    }
    match status_column {
        Some(column) => {
            let row = lines.find(|line| row_is_for(line, &wanted));
            row.and_then(|line| token_at(line, column)) == Some("connected")
        }
        None => rows
            .into_iter()
            .find(|line| row_is_for(line, &wanted))
            .is_some_and(|line| line.split_whitespace().any(|token| token == "connected")),
    }
}

/// Character offset of `Status` when `line` is the table header.
fn header_status_column(line: &str) -> Option<usize> {
    if line.split_whitespace().next() != Some("Port") {
        return None;
    }
    line.find("Status").map(|byte| line[..byte].chars().count())
}

fn row_is_for(line: &str, wanted: &str) -> bool {
    line.split_whitespace()
        .next()
        .is_some_and(|first| canonical_interface_name(first) == wanted)
}

/// The whitespace-delimited token that starts at or spans character `column`.
fn token_at(line: &str, column: usize) -> Option<&str> {
    let chars: Vec<(usize, char)> = line.char_indices().collect();
    if chars.get(column)?.1.is_whitespace() {
        return None;
    }
    let mut first = column;
    while first > 0 && !chars[first - 1].1.is_whitespace() {
        first -= 1;
    }
    let mut last = column;
    while last + 1 < chars.len() && !chars[last + 1].1.is_whitespace() {
        last += 1;
    }
    let end = chars.get(last + 1).map_or(line.len(), |&(byte, _)| byte);
    Some(&line[chars[first].0..end])
}

/// Whether `show interfaces` output has the line for `port` reporting
/// `is up, line protocol is up`.
pub fn router_port_passed(show_interfaces: &str, port: &str) -> bool {
    let wanted = canonical_interface_name(port.trim());
    show_interfaces.lines().any(|line| {
        row_is_for(line, &wanted) && line.contains("is up, line protocol is up")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_physical_keeps_management_and_skips_virtual() {
        let names = vec!["Loopback0".into(), "Vlan1".into(), "GigabitEthernet0/0".into(), "GigabitEthernet1/0/1".into()];
        assert_eq!(first_physical(&names).as_deref(), Some("GigabitEthernet0/0"));
        assert!(!physical_interfaces(&names).iter().any(|n| n == "Loopback0" || n == "Vlan1"));
    }

    #[test]
    fn switch_connected_passes_and_notconnect_fails() {
        let table = "Gi1/0/3 connected 1 a-full a-1000\nGi1/0/4 notconnect 1 auto auto\n";
        assert!(switch_port_passed(table, "Gi1/0/3"));
        assert!(!switch_port_passed(table, "Gi1/0/4"));
        assert!(switch_port_passed(table, "GigabitEthernet1/0/3"));
    }

    #[test]
    fn a_description_does_not_shift_the_status_column() {
        let table = "Port         Name               Status       Vlan       Duplex  Speed Type\nGi1/0/5      connected to core  notconnect   1            auto   auto 10/100/1000BaseTX\n";
        assert!(!switch_port_passed(table, "Gi1/0/5"));
    }

    #[test]
    fn router_line_protocol_up_passes() {
        let up = "GigabitEthernet0/0/0 is up, line protocol is up";
        let down = "GigabitEthernet0/0/1 is up, line protocol is down";
        assert!(router_port_passed(up, "GigabitEthernet0/0/0"));
        assert!(!router_port_passed(down, "GigabitEthernet0/0/1"));
    }

    #[test]
    fn skip_prefixes_are_case_insensitive_and_keep_order() {
        let names: Vec<String> = [
            "loopback1", "TUNNEL0", "Virtual-Access1", "Port-channel1", "BDI10", "NVI0", "Null0",
            "Dialer1", "AppGigabitEthernet1/0/1", "Bluetooth0/4", "FastEthernet0", "TenGigabitEthernet1/1/1",
            "GigabitEthernet1/0/2",
        ]
        .into_iter()
        .map(String::from)
        .collect();
        assert_eq!(
            physical_interfaces(&names),
            vec!["FastEthernet0", "TenGigabitEthernet1/1/1", "GigabitEthernet1/0/2"]
        );
    }

    #[test]
    fn abbreviated_virtual_names_are_skipped() {
        let names: Vec<String> = ["Vl1", "Po2", "Lo0", "Gi1/0/1"].into_iter().map(String::from).collect();
        assert_eq!(physical_interfaces(&names), vec!["Gi1/0/1"]);
    }

    #[test]
    fn no_physical_interface_gives_none() {
        assert_eq!(first_physical(&[]), None);
        assert_eq!(first_physical(&["Vlan1".to_string()]), None);
    }

    #[test]
    fn switch_header_status_column_is_used() {
        let table = "Port      Name   Status     Vlan\nGi1/0/1   x      connected  1\nGi1/0/2   y      notconnect 1\nGi1/0/3   z      connected\n";
        assert!(switch_port_passed(table, "Gi1/0/1"));
        assert!(!switch_port_passed(table, "Gi1/0/2"));
        assert!(switch_port_passed(table, "Gi1/0/3"));
    }

    #[test]
    fn switch_status_token_spanning_the_column_offset_counts() {
        let table = "Port      Name   Status     Vlan\nGi1/0/1   x     connected  1\n";
        assert!(switch_port_passed(table, "Gi1/0/1"));
    }

    #[test]
    fn switch_missing_port_or_malformed_input_fails() {
        let table = "Gi1/0/3 connected 1 a-full a-1000\n";
        assert!(!switch_port_passed(table, "Gi1/0/9"));
        assert!(!switch_port_passed(table, ""));
        assert!(!switch_port_passed("", "Gi1/0/3"));
        assert!(!switch_port_passed("Port Name Status\n", "Gi1/0/3"));
        assert!(!switch_port_passed("Port Name Status\nGi1/0/3\n", "Gi1/0/3"));
        assert!(!switch_port_passed("Port Name Status\nGi1/0/3 a\n", "Gi1/0/3"));
    }

    #[test]
    fn switch_without_header_needs_a_token_exactly_connected() {
        assert!(!switch_port_passed("Gi1/0/3 notconnected 1\n", "Gi1/0/3"));
        assert!(!switch_port_passed("Gi1/0/3 err-disabled 1\n", "Gi1/0/3"));
    }

    #[test]
    fn switch_header_is_the_first_port_line_containing_status() {
        let table = "Port Name\nPort      Name   Status     Vlan\nGi1/0/1   x      connected  1\n";
        assert!(switch_port_passed(table, "Gi1/0/1"));
    }

    #[test]
    fn router_down_variants_and_missing_line_fail() {
        let text = "GigabitEthernet0/0/0 is administratively down, line protocol is down\nGigabitEthernet0/0/1 is down, line protocol is down (notconnect)\nGigabitEthernet0/0/2 is up, line protocol is up (connected)\n";
        assert!(!router_port_passed(text, "GigabitEthernet0/0/0"));
        assert!(!router_port_passed(text, "GigabitEthernet0/0/1"));
        assert!(router_port_passed(text, "Gi0/0/2"));
        assert!(!router_port_passed(text, "GigabitEthernet0/0/3"));
        assert!(!router_port_passed("", "GigabitEthernet0/0/0"));
        assert!(!router_port_passed(text, ""));
    }

    #[test]
    fn router_matches_the_exact_interface() {
        let text = "GigabitEthernet0/0/10 is up, line protocol is up\n";
        assert!(!router_port_passed(text, "GigabitEthernet0/0/1"));
    }
}
