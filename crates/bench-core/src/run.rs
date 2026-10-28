#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FirmwareChoice {
    pub image_path: String,
    pub local_ethernet: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunOptions {
    pub serial_device: String,
    pub config_path: String,
    pub firmware: Option<FirmwareChoice>,
    pub burn_in_hours: Option<u32>,
    pub port_testing: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckStatus {
    NotChosen,
    NotFinished,
    Passed,
    Failed { detail: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunRecord {
    pub configuration: CheckStatus,
    pub firmware: CheckStatus,
    pub burn_in: CheckStatus,
    pub ports: CheckStatus,
}

/// A device may be released only when configuration passed and every other
/// check either passed or was not chosen.
pub fn ready_to_release(record: &RunRecord) -> bool {
    let settled =
        |status: &CheckStatus| matches!(status, CheckStatus::Passed | CheckStatus::NotChosen);
    record.configuration == CheckStatus::Passed
        && settled(&record.firmware)
        && settled(&record.burn_in)
        && settled(&record.ports)
}

/// Burn-in must run for at least one hour.
pub fn validate_burn_in_hours(hours: u32) -> Result<u32, ()> {
    if hours >= 1 {
        Ok(hours)
    } else {
        Err(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hours_below_one_are_rejected() {
        assert!(validate_burn_in_hours(0).is_err());
        assert_eq!(validate_burn_in_hours(1).unwrap(), 1);
    }

    #[test]
    fn release_waits_for_every_chosen_check() {
        let pending = RunRecord {
            configuration: CheckStatus::Passed,
            firmware: CheckStatus::NotChosen,
            burn_in: CheckStatus::NotFinished,
            ports: CheckStatus::Passed,
        };
        assert!(!ready_to_release(&pending));
        let done = RunRecord {
            burn_in: CheckStatus::NotChosen,
            ..pending
        };
        assert!(ready_to_release(&done));
    }
}
