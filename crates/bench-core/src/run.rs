#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FirmwareChoice {
    pub image_path: String,
    pub local_ethernet: String,
}

use crate::family::{CapwapChoice, DeviceFamily};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunOptions {
    pub serial_device: String,
    pub config_path: String,
    pub family: DeviceFamily,
    pub firmware: Option<FirmwareChoice>,
    pub burn_in_hours: Option<u32>,
    pub port_testing: bool,
    pub capwap: Option<CapwapChoice>,
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
    pub capwap: CheckStatus,
}

/// A device may be released when configuration and every other check either
/// passed or was not chosen.
pub fn ready_to_release(record: &RunRecord) -> bool {
    let settled =
        |status: &CheckStatus| matches!(status, CheckStatus::Passed | CheckStatus::NotChosen);
    settled(&record.configuration)
        && settled(&record.firmware)
        && settled(&record.burn_in)
        && settled(&record.ports)
        && settled(&record.capwap)
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
    fn ap_release_with_only_capwap_passed() {
        let record = RunRecord {
            configuration: CheckStatus::NotChosen,
            firmware: CheckStatus::NotChosen,
            burn_in: CheckStatus::NotChosen,
            ports: CheckStatus::NotChosen,
            capwap: CheckStatus::Passed,
        };
        assert!(ready_to_release(&record));
    }

    #[test]
    fn unfinished_configuration_blocks_release() {
        let record = RunRecord {
            configuration: CheckStatus::NotFinished,
            firmware: CheckStatus::NotChosen,
            burn_in: CheckStatus::NotChosen,
            ports: CheckStatus::NotChosen,
            capwap: CheckStatus::NotChosen,
        };
        assert!(!ready_to_release(&record));
    }

    #[test]
    fn unfinished_capwap_blocks_release_when_config_passed() {
        let record = RunRecord {
            configuration: CheckStatus::Passed,
            firmware: CheckStatus::NotChosen,
            burn_in: CheckStatus::NotChosen,
            ports: CheckStatus::NotChosen,
            capwap: CheckStatus::NotFinished,
        };
        assert!(!ready_to_release(&record));
    }

    #[test]
    fn release_waits_for_every_chosen_check() {
        let pending = RunRecord {
            configuration: CheckStatus::Passed,
            firmware: CheckStatus::NotChosen,
            burn_in: CheckStatus::NotFinished,
            ports: CheckStatus::Passed,
            capwap: CheckStatus::NotChosen,
        };
        assert!(!ready_to_release(&pending));
        let done = RunRecord {
            burn_in: CheckStatus::NotChosen,
            ..pending
        };
        assert!(ready_to_release(&done));
    }
}
