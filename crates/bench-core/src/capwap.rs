use crate::CapwapChoice;

pub fn capwap_apply_command(choice: &CapwapChoice) -> String {
    format!(
        "capwap ap ip {} {} {}",
        choice.ip.trim(),
        choice.mask.trim(),
        choice.gateway.trim()
    )
}

pub fn capwap_passed(show_capwap_client_config: &str, choice: &CapwapChoice) -> bool {
    let ip = choice.ip.trim();
    let mask = choice.mask.trim();
    let gateway = choice.gateway.trim();
    has_token(show_capwap_client_config, ip)
        && has_token(show_capwap_client_config, mask)
        && has_token(show_capwap_client_config, gateway)
}

pub fn capwap_failure_detail(show: &str, choice: &CapwapChoice) -> String {
    let mut missing = Vec::new();
    if !has_token(show, choice.ip.trim()) {
        missing.push("IP");
    }
    if !has_token(show, choice.mask.trim()) {
        missing.push("mask");
    }
    if !has_token(show, choice.gateway.trim()) {
        missing.push("gateway");
    }
    missing.join(", ")
}

fn has_token(show: &str, value: &str) -> bool {
    show.split_whitespace().any(|token| token == value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CapwapChoice;

    #[test]
    fn apply_command_joins_three_fields() {
        let c = CapwapChoice {
            ip: "10.1.1.10".into(),
            mask: "255.255.255.0".into(),
            gateway: "10.1.1.1".into(),
        };
        assert_eq!(
            capwap_apply_command(&c),
            "capwap ap ip 10.1.1.10 255.255.255.0 10.1.1.1"
        );
    }

    #[test]
    fn validation_requires_ip_mask_and_gateway() {
        let c = CapwapChoice {
            ip: "10.1.1.10".into(),
            mask: "255.255.255.0".into(),
            gateway: "10.1.1.1".into(),
        };
        let ok = "IP Address: 10.1.1.10\nNetmask: 255.255.255.0\nDefault Gateway: 10.1.1.1\n";
        assert!(capwap_passed(ok, &c));
        assert!(!capwap_passed(
            "IP Address: 10.1.1.10\nNetmask: 255.255.255.0\n",
            &c
        ));
        assert!(!capwap_passed(
            "IP Address: 10.9.9.9\nNetmask: 255.255.255.0\nDefault Gateway: 10.1.1.1\n",
            &c
        ));
    }
}
