//! Decide whether an uploaded IOS-XE configuration file took effect, by
//! comparing it with `show running-config`. Extra device lines are allowed.

/// Full interface names and the order in which abbreviations resolve.
const INTERFACE_NAMES: [&str; 11] = [
    "GigabitEthernet",
    "TenGigabitEthernet",
    "TwentyFiveGigE",
    "FortyGigabitEthernet",
    "HundredGigE",
    "FastEthernet",
    "AppGigabitEthernet",
    "Port-channel",
    "Loopback",
    "Vlan",
    "Tunnel",
];

/// Largest `interface range` span expanded into members.
const MAX_RANGE_MEMBERS: u32 = 4096;

/// Expand an interface abbreviation (`Gi1/0/3`) to its full name
/// (`GigabitEthernet1/0/3`). Names that are already full, or match nothing,
/// are returned unchanged. Interface tokens anywhere in the text are expanded.
pub fn canonical_interface_name(name: &str) -> String {
    let chars: Vec<char> = name.chars().collect();
    let mut out = String::with_capacity(name.len() + 16);
    let mut i = 0;
    while i < chars.len() {
        let starts_word = i == 0 || !chars[i - 1].is_alphanumeric();
        if !(starts_word && chars[i].is_ascii_alphabetic()) {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        let word_end = scan(&chars, i, |c| c.is_ascii_alphabetic() || c == '-');
        let port_end = scan(&chars, word_end, |c| {
            c.is_ascii_digit() || c == '/' || c == '.'
        });
        let word: String = chars[i..word_end].iter().collect();
        let has_port = port_end > word_end && chars[word_end].is_ascii_digit();
        match full_interface_name(&word).filter(|_| has_port) {
            Some(full) => out.push_str(full),
            None => out.push_str(&word),
        }
        out.extend(&chars[word_end..port_end]);
        i = port_end.max(i + 1);
    }
    out
}

fn scan(chars: &[char], from: usize, keep: impl Fn(char) -> bool) -> usize {
    let mut end = from;
    while end < chars.len() && keep(chars[end]) {
        end += 1;
    }
    end
}

fn full_interface_name(word: &str) -> Option<&'static str> {
    if word.len() < 2 {
        return None;
    }
    let lower = word.to_ascii_lowercase();
    INTERFACE_NAMES
        .into_iter()
        .find(|full| full.to_ascii_lowercase().starts_with(&lower))
}

/// Lines of the uploaded file that are not present in the running config,
/// as written in the file, in file order.
pub fn missing_commands(uploaded: &str, running: &str) -> Vec<String> {
    let running = parse(running);
    let mut missing = Vec::new();
    for entry in parse(uploaded) {
        check_entry(&entry, &running, &mut missing);
    }
    missing
}

/// A top-level command and the indented lines that follow it.
struct Entry {
    header: Line,
    children: Vec<Line>,
}

struct Line {
    original: String,
    normalized: String,
}

fn parse(text: &str) -> Vec<Entry> {
    let mut entries: Vec<Entry> = Vec::new();
    for raw in text.lines() {
        let normalized = normalize(raw);
        if is_ignored(&normalized) {
            continue;
        }
        let line = Line {
            original: raw.to_string(),
            normalized,
        };
        let indented = raw.starts_with(char::is_whitespace);
        match entries.last_mut() {
            Some(entry) if indented => entry.children.push(line),
            _ => entries.push(Entry {
                header: line,
                children: Vec::new(),
            }),
        }
    }
    entries
}

fn normalize(line: &str) -> String {
    let collapsed = line.split_whitespace().collect::<Vec<_>>().join(" ");
    canonical_interface_name(&collapsed)
}

fn is_ignored(normalized: &str) -> bool {
    let lower = normalized.to_ascii_lowercase();
    normalized.is_empty()
        || normalized.starts_with('!')
        || matches!(
            lower.as_str(),
            "configure terminal" | "conf t" | "end" | "exit"
        )
        || lower.starts_with("building configuration")
        || lower.starts_with("current configuration")
        || lower.starts_with("last configuration change")
}

fn check_entry(entry: &Entry, running: &[Entry], missing: &mut Vec<String>) {
    let header = entry.header.normalized.as_str();
    let range = range_headers(header);
    let is_range = range.is_some();
    let targets = range.unwrap_or_else(|| vec![header.to_string()]);
    let top: Vec<&str> = running
        .iter()
        .map(|e| e.header.normalized.as_str())
        .collect();
    let header_present = if is_range {
        targets.iter().all(|t| top.contains(&t.as_str()))
    } else {
        is_present(header, &top)
    };
    if !header_present {
        missing.push(entry.header.original.clone());
        missing.extend(entry.children.iter().map(|c| c.original.clone()));
        return;
    }
    let sections: Vec<Vec<&str>> = targets.iter().map(|t| section_lines(running, t)).collect();
    for child in &entry.children {
        if !sections
            .iter()
            .all(|lines| is_present(&child.normalized, lines))
        {
            missing.push(child.original.clone());
        }
    }
}

/// Children of every running-config top-level line equal to `header`.
fn section_lines<'a>(running: &'a [Entry], header: &str) -> Vec<&'a str> {
    running
        .iter()
        .filter(|e| e.header.normalized == header)
        .flat_map(|e| e.children.iter().map(|c| c.normalized.as_str()))
        .collect()
}

/// Member headers of `interface range ...`, or `None` for any other header.
fn range_headers(header: &str) -> Option<Vec<String>> {
    let spec = header.strip_prefix("interface range ")?;
    let mut headers = Vec::new();
    for item in spec.split(',') {
        let members =
            expand_range_item(item.trim()).unwrap_or_else(|| vec![item.trim().to_string()]);
        headers.extend(members.into_iter().map(|m| format!("interface {m}")));
    }
    Some(headers)
}

/// `GigabitEthernet1/0/1 - 2` -> `[...1/0/1, ...1/0/2]`; the hyphen applies to
/// the last number of the name. `None` when the item is not a range.
fn expand_range_item(item: &str) -> Option<Vec<String>> {
    let (left, right) = item.rsplit_once('-')?;
    let (left, right) = (left.trim(), right.trim());
    if right.is_empty() || !right.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let digits = left.len() - left.trim_end_matches(|c: char| c.is_ascii_digit()).len();
    if digits == 0 {
        return None;
    }
    let (prefix, first) = left.split_at(left.len() - digits);
    let (first, last): (u32, u32) = (first.parse().ok()?, right.parse().ok()?);
    if last < first || last - first >= MAX_RANGE_MEMBERS {
        return None;
    }
    Some((first..=last).map(|n| format!("{prefix}{n}")).collect())
}

/// Whether `line` is satisfied by `section` (normalized running lines).
fn is_present(line: &str, section: &[&str]) -> bool {
    match line.strip_prefix("no ") {
        Some(command) => section.contains(&line) || !command_remains(command, section),
        None => has_command(line, section),
    }
}

fn has_command(command: &str, section: &[&str]) -> bool {
    match secret_prefix(command) {
        Some(wanted) => section.iter().any(|l| secret_prefix(l) == Some(wanted)),
        None => section.contains(&command),
    }
}

/// Whether `command` is still configured, with or without further arguments.
fn command_remains(command: &str, section: &[&str]) -> bool {
    has_command(command, section)
        || section.iter().any(|l| {
            l.strip_prefix(command)
                .is_some_and(|rest| rest.starts_with(' '))
        })
}

/// The words up to and including the first `password`/`secret` keyword.
/// `description` lines are free text, so they have no secret prefix.
fn secret_prefix(line: &str) -> Option<&str> {
    if line.split(' ').next() == Some("description") {
        return None;
    }
    let mut end = 0;
    for word in line.split(' ') {
        end += word.len();
        if word == "password" || word == "secret" {
            return Some(&line[..end]);
        }
        end += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extra_device_lines_do_not_fail() {
        let file =
            "hostname closet-2-sw1\ninterface GigabitEthernet1/0/1\n switchport mode access\n";
        let running = "Building configuration...\nCurrent configuration : 4821 bytes\nhostname closet-2-sw1\nntp clock-period 36029123\ninterface GigabitEthernet1/0/1\n switchport mode access\n";
        assert!(missing_commands(file, running).is_empty());
    }

    #[test]
    fn abbreviated_names_and_ranges_count_as_present() {
        let file = "interface Gi1/0/3\n switchport mode access\ninterface range GigabitEthernet1/0/1 - 2\n switchport access vlan 20\n";
        let running = "interface GigabitEthernet1/0/1\n switchport access vlan 20\ninterface GigabitEthernet1/0/2\n switchport access vlan 20\ninterface GigabitEthernet1/0/3\n switchport mode access\n";
        assert!(missing_commands(file, running).is_empty());
    }

    #[test]
    fn cleartext_secret_matches_stored_hash() {
        let file = "username cisco privilege 15 secret 0 cisco\n";
        let running = "username cisco privilege 15 secret 9 $9$abc\n";
        assert!(missing_commands(file, running).is_empty());
    }

    #[test]
    fn a_missing_command_is_named() {
        let file =
            "hostname closet-2-sw1\ninterface GigabitEthernet1/0/1\n switchport access vlan 20\n";
        let running =
            "hostname closet-2-sw1\ninterface GigabitEthernet1/0/1\n switchport mode access\n";
        assert_eq!(
            missing_commands(file, running),
            vec![" switchport access vlan 20".to_string()]
        );
    }

    #[test]
    fn navigation_lines_and_negations_are_not_missing() {
        let file = "configure terminal\ninterface GigabitEthernet1/0/1\n no shutdown\n exit\nno ip http server\nend\n";
        let running = "interface GigabitEthernet1/0/1\n switchport mode access\n";
        assert!(missing_commands(file, running).is_empty());
        assert_eq!(canonical_interface_name("Gi1/0/3"), "GigabitEthernet1/0/3");
    }

    fn missing(file: &str, running: &str) -> Vec<String> {
        missing_commands(file, running)
    }

    #[test]
    fn interface_names_expand_by_prefix_in_order() {
        let cases = [
            ("Gi1/0/3", "GigabitEthernet1/0/3"),
            ("gig1/0/3", "GigabitEthernet1/0/3"),
            ("Te1/1/1", "TenGigabitEthernet1/1/1"),
            ("Tw1/0/1", "TwentyFiveGigE1/0/1"),
            ("Twe1/0/1", "TwentyFiveGigE1/0/1"),
            ("Fo1/1/1", "FortyGigabitEthernet1/1/1"),
            ("Hu1/0/1", "HundredGigE1/0/1"),
            ("Fa0/1", "FastEthernet0/1"),
            ("Ap1/0/1", "AppGigabitEthernet1/0/1"),
            ("Po5", "Port-channel5"),
            ("Lo0", "Loopback0"),
            ("Vl20", "Vlan20"),
            ("Tu1", "Tunnel1"),
            ("Vlan20", "Vlan20"),
            ("GigabitEthernet1/0/3", "GigabitEthernet1/0/3"),
            ("Subinterface7", "Subinterface7"),
            ("G1/0/3", "G1/0/3"),
            ("Gi1/0/3.100", "GigabitEthernet1/0/3.100"),
        ];
        for (input, expected) in cases {
            assert_eq!(canonical_interface_name(input), expected, "{input}");
        }
    }

    #[test]
    fn interface_names_expand_inside_lines() {
        assert_eq!(
            canonical_interface_name("channel-group 1 mode on ! uplink Te1/1/1 and Po1"),
            "channel-group 1 mode on ! uplink TenGigabitEthernet1/1/1 and Port-channel1"
        );
    }

    #[test]
    fn whitespace_is_collapsed_but_original_line_is_reported() {
        let file = "interface  Gi1/0/1 
   switchport   mode  access
   spanning-tree portfast
";
        let running = "interface GigabitEthernet1/0/1
 switchport mode access
";
        assert_eq!(
            missing(file, running),
            vec!["   spanning-tree portfast".to_string()]
        );
    }

    #[test]
    fn missing_header_reports_header_and_children() {
        let file = "interface Gi1/0/9
 description uplink
 switchport mode trunk
hostname sw1
";
        let running = "hostname sw1
interface GigabitEthernet1/0/1
 description uplink
";
        assert_eq!(
            missing(file, running),
            vec![
                "interface Gi1/0/9".to_string(),
                " description uplink".to_string(),
                " switchport mode trunk".to_string()
            ]
        );
    }

    #[test]
    fn children_only_match_their_own_section() {
        let file = "interface Gi1/0/1
 switchport mode access
";
        let running = "interface GigabitEthernet1/0/1
 description x
interface GigabitEthernet1/0/2
 switchport mode access
";
        assert_eq!(
            missing(file, running),
            vec![" switchport mode access".to_string()]
        );
    }

    #[test]
    fn top_level_command_does_not_match_a_child_line() {
        let running = "interface GigabitEthernet1/0/1
 ip http server
";
        assert_eq!(
            missing(
                "ip http server
",
                running
            ),
            vec!["ip http server".to_string()]
        );
    }

    #[test]
    fn range_requires_every_member_header_and_child() {
        let file = "interface range Gi1/0/1-3
 switchport access vlan 20
";
        let all = "interface GigabitEthernet1/0/1
 switchport access vlan 20
interface GigabitEthernet1/0/2
 switchport access vlan 20
interface GigabitEthernet1/0/3
 switchport access vlan 20
";
        assert!(missing(file, all).is_empty());
        let one_wrong = all.replace(
            "vlan 20
interface GigabitEthernet1/0/3",
            "vlan 30
interface GigabitEthernet1/0/3",
        );
        assert_eq!(
            missing(file, &one_wrong),
            vec![" switchport access vlan 20".to_string()]
        );
        let no_member = "interface GigabitEthernet1/0/1
 switchport access vlan 20
interface GigabitEthernet1/0/2
 switchport access vlan 20
";
        assert_eq!(
            missing(file, no_member),
            vec![
                "interface range Gi1/0/1-3".to_string(),
                " switchport access vlan 20".to_string()
            ]
        );
    }

    #[test]
    fn range_supports_comma_lists() {
        let file = "interface range Gi1/0/1 - 2 , Gi1/0/5
 shutdown
";
        let running = "interface GigabitEthernet1/0/1
 shutdown
interface GigabitEthernet1/0/2
 shutdown
interface GigabitEthernet1/0/5
 shutdown
";
        assert!(missing(file, running).is_empty());
        let without_five = running.replace(
            "interface GigabitEthernet1/0/5
 shutdown
",
            "",
        );
        assert_eq!(missing(file, &without_five).len(), 2);
    }

    #[test]
    fn no_command_fails_when_the_command_is_present() {
        let running = "interface GigabitEthernet1/0/1
 shutdown
";
        let file = "interface Gi1/0/1
 no shutdown
";
        assert_eq!(missing(file, running), vec![" no shutdown".to_string()]);
        let explicit = "interface GigabitEthernet1/0/1
 no shutdown
";
        assert!(missing(file, explicit).is_empty());
        assert_eq!(
            missing(
                "no ip http server
",
                "ip http server
"
            ),
            vec!["no ip http server".to_string()]
        );
    }

    #[test]
    fn secrets_must_match_up_to_the_keyword() {
        let running = "username cisco privilege 15 secret 9 $9$abc
line vty 0 4
 password 7 0822455D
 enable secret 5 x
";
        assert!(missing(
            "line vty 0 4
 password cisco
",
            running
        )
        .is_empty());
        assert_eq!(
            missing(
                "username admin privilege 15 secret 0 pw
",
                running
            ),
            vec!["username admin privilege 15 secret 0 pw".to_string()]
        );
        assert_eq!(
            missing(
                "username cisco privilege 1 secret 0 pw
",
                running
            ),
            vec!["username cisco privilege 1 secret 0 pw".to_string()]
        );
        assert_eq!(
            missing(
                "enable secret 0 pw
",
                running
            ),
            vec!["enable secret 0 pw".to_string()],
            "enable secret is not in the vty section"
        );
        assert_eq!(
            missing(
                "no username cisco privilege 15 secret
",
                running
            )
            .len(),
            1
        );
    }

    #[test]
    fn ignored_lines_are_not_commands() {
        let file = "! comment

  
conf t
Building configuration...
Current configuration : 12 bytes
!
end
";
        assert!(missing(file, "").is_empty());
    }

    #[test]
    fn no_ip_address_is_missing_when_an_address_remains() {
        let file = "interface Gi1/0/1\n no ip address\n";
        let with_address = "interface GigabitEthernet1/0/1\n ip address 10.0.0.1 255.255.255.0\n";
        assert_eq!(
            missing(file, with_address),
            vec![" no ip address".to_string()]
        );
        let without =
            "interface GigabitEthernet1/0/1\n ip addresses-other 1\n switchport mode access\n";
        assert!(missing(file, without).is_empty());
        let shut = "interface GigabitEthernet1/0/1\n switchport mode access\n";
        assert!(missing("interface Gi1/0/1\n no shutdown\n", shut).is_empty());
    }

    #[test]
    fn descriptions_mentioning_password_compare_exactly() {
        let running = "interface GigabitEthernet1/0/1\n description password reset kiosk\n";
        assert!(missing(
            "interface Gi1/0/1\n description password reset kiosk\n",
            running
        )
        .is_empty());
        assert_eq!(
            missing("interface Gi1/0/1\n description password\n", running),
            vec![" description password".to_string()]
        );
        assert_eq!(
            missing("interface Gi1/0/1\n description secret lab\n", running),
            vec![" description secret lab".to_string()]
        );
    }
}
