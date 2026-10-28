pub mod config_check;
pub mod run;

pub use config_check::{canonical_interface_name, missing_commands};
pub use run::{
    ready_to_release, validate_burn_in_hours, CheckStatus, FirmwareChoice, RunOptions, RunRecord,
};
