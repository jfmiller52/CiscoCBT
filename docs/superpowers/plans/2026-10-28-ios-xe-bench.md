# IOS-XE Bench Configurator Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A macOS and Windows desktop app that drives a factory IOS-XE switch or router over the console, optionally installs an image over TFTP, and writes a validation file for the configuration, burn-in, and ports the technician chose.

**Architecture:** A UI-free Rust library (`bench-core`) decides every check, login, and phase. A Dioxus desktop shell draws the Cursor-like window and calls that library. Serial and TFTP are adapters behind traits so the run order is tested without a device.

**Tech Stack:** Rust edition 2021, `bench-core` with no UI crates, Dioxus 0.6 desktop for `bench-app`, `serialport` for the console, a small in-process UDP TFTP server.

**Spec:** `docs/superpowers/specs/2026-10-28-ios-xe-bench-design.md`

## Global Constraints

- First version is IOS-XE switches and routers only. Devices are fresh out of the box or factory reset.
- Ship for macOS and Windows from one session core. Commit on `main`. Do not create a feature branch.
- Console settings are 9600 8N1. The technician picks the local serial device.
- Firmware images go by TFTP on a second cable, never down the console. The configuration file goes over the console.
- Factory account, when first boot asks, is username `cisco`, password `cisco`, enable secret `cisco`. If that password is rejected as too short, password and enable secret become `Passw0rd%`. A further rejection stops the run.
- Burn-in hours are whole numbers, minimum 1. Pass when every `show version | include uptime` line is at least that many hours, read only after the clean-slate reboot and the wait.
- A switch port passes only on `show interface status` status `connected`. A router port passes only when `show interfaces <name>` reports the interface up and line protocol up.
- Configuration passes when every uploaded command took effect. Extra device lines are allowed. A byte-for-byte match is not required.
- Do not reboot into a console password the program cannot type. Do not save the temporary TFTP address.
- The on-screen reminder shows the factory username, password, and enable secret that were accepted, for the whole run.

## Review Focus

- A cleartext `secret 0` or `password` in the uploaded file is stored by IOS-XE as a hash. The configuration check must still pass. Pinned in Task 2.
- An uploaded file whose only console secret is `enable secret 9 …` must stop before the post-apply reboot. Pinned in Task 7.
- `interface range` in the file and member interfaces in the running config must count as present. Pinned in Task 2.
- Two uptime lines, one shorter than the entered hours, fail the burn-in even if the other line is long enough. Pinned in Task 3.
- `Loopback0` and `Vlan1` are not the TFTP port and not in the port walk. `GigabitEthernet0/0` is physical and wins when the device lists it first. Pinned in Task 4.

## File structure

- `Cargo.toml` — workspace (`bench-core`, `bench-app`)
- `crates/bench-core/src/lib.rs` — re-exports
- `crates/bench-core/src/run.rs` — options, check status, release
- `crates/bench-core/src/config_check.rs` — uploaded file versus running config
- `crates/bench-core/src/uptime.rs` — uptime parse and burn-in pass
- `crates/bench-core/src/ports.rs` — physical ports and link checks
- `crates/bench-core/src/firmware.rs` — version token and firmware pass
- `crates/bench-core/src/login.rs` — factory login and post-apply login
- `crates/bench-core/src/export.rs` — validation text
- `crates/bench-core/src/phase.rs` — reboot order
- `crates/bench-core/src/transfer.rs` — temporary address and TFTP copy command
- `crates/bench-core/src/form.rs` — start-of-run field rules
- `crates/bench-core/src/present.rs` — headings and the password reminder line
- `crates/bench-core/src/console_link.rs` — 9600 8N1 and the TFTP read check
- `crates/bench-app/` — Dioxus desktop shell
- Tests live in each `bench-core` module under `#[cfg(test)]`

---

### Task 1: Workspace and release record

**Files:**
- Create: `Cargo.toml`
- Create: `crates/bench-core/Cargo.toml`
- Create: `crates/bench-core/src/lib.rs`
- Create: `crates/bench-core/src/run.rs`

**Interfaces:**
- Consumes: nothing
- Produces:
  - `pub struct FirmwareChoice { pub image_path: String, pub local_ethernet: String }`
  - `pub struct RunOptions { pub serial_device: String, pub config_path: String, pub firmware: Option<FirmwareChoice>, pub burn_in_hours: Option<u32>, pub port_testing: bool }`
  - `pub enum CheckStatus { NotChosen, NotFinished, Passed, Failed { detail: String } }`
  - `pub struct RunRecord { pub configuration: CheckStatus, pub firmware: CheckStatus, pub burn_in: CheckStatus, pub ports: CheckStatus }`
  - `pub fn ready_to_release(record: &RunRecord) -> bool`
  - `pub fn validate_burn_in_hours(hours: u32) -> Result<u32, ()>` — `Ok` only when `hours >= 1`

- [ ] **Step 1: Write the failing test**

```rust
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
    let done = RunRecord { burn_in: CheckStatus::NotChosen, ..pending };
    assert!(ready_to_release(&done));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p bench-core --lib run::`
Expected: FAIL to compile, symbols missing

- [ ] **Step 3: Implement the types and the two functions in `crates/bench-core/src/run.rs`**

`ready_to_release` is true only when every status is `Passed` or `NotChosen`, and `configuration` is `Passed`.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p bench-core --lib run::`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml crates/bench-core
git commit -m "feat: add bench run record and hour minimum"
```

### Task 2: Configuration presence

**Files:**
- Create: `crates/bench-core/src/config_check.rs`
- Modify: `crates/bench-core/src/lib.rs`

**Interfaces:**
- Consumes: nothing from Task 1
- Produces: `pub fn missing_commands(uploaded: &str, running: &str) -> Vec<String>` — each missing command’s original line, in file order

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn extra_device_lines_do_not_fail() {
    let file = "hostname closet-2-sw1\ninterface GigabitEthernet1/0/1\n switchport mode access\n";
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
    let file = "hostname closet-2-sw1\ninterface GigabitEthernet1/0/1\n switchport access vlan 20\n";
    let running = "hostname closet-2-sw1\ninterface GigabitEthernet1/0/1\n switchport mode access\n";
    assert_eq!(missing_commands(file, running), vec![" switchport access vlan 20".to_string()]);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p bench-core --lib config_check::`
Expected: FAIL to compile

- [ ] **Step 3: Implement `missing_commands` in `config_check.rs`**

Ignore blank lines, `!` comments, and running-config header lines (`Building configuration`, `Current configuration :`, `Last configuration change`). A `secret 0` or `password` line is present when the same username, or enable secret, exists in the running config even if the stored value is a hash. `interface range GigabitEthernet1/0/1 - 2` is present when each member exists and carries the range’s sub-commands. Abbreviate `GigabitEthernet`, `TenGigabitEthernet`, `FastEthernet`, `AppGigabitEthernet`, and `Port-channel` the way IOS does (`Gi`, `Te`, `Fa`, `Ap`, `Po`).

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p bench-core --lib config_check::`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/bench-core
git commit -m "feat: compare uploaded commands to the running config"
```

### Task 3: Burn-in uptime

**Files:**
- Create: `crates/bench-core/src/uptime.rs`
- Modify: `crates/bench-core/src/lib.rs`

**Interfaces:**
- Consumes: nothing
- Produces:
  - `pub struct Uptime { pub minutes_total: u64 }`
  - `pub fn parse_uptime_lines(output: &str) -> Vec<Uptime>`
  - `pub fn burn_in_passes(entered_hours: u32, lines: &[Uptime]) -> bool` — false when `lines` is empty

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn four_hours_six_minutes_passes_a_four_hour_entry() {
    let lines = parse_uptime_lines("closet-2-sw1 uptime is 4 hours, 6 minutes");
    assert!(burn_in_passes(4, &lines));
}

#[test]
fn three_hours_fifty_eight_minutes_fails() {
    let lines = parse_uptime_lines("Switch uptime is 3 hours, 58 minutes");
    assert!(!burn_in_passes(4, &lines));
}

#[test]
fn one_short_line_fails_the_burn_in() {
    let lines = parse_uptime_lines("Switch uptime is 1 day, 2 hours\nSwitch uptime is 3 hours, 1 minute");
    assert!(!burn_in_passes(4, &lines));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p bench-core --lib uptime::`
Expected: FAIL to compile

- [ ] **Step 3: Implement parse and compare in `uptime.rs`**

Accept `uptime is` lines with optional days, hours, and minutes. Compare total minutes to `entered_hours * 60`. Every parsed line must clear that bar.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p bench-core --lib uptime::`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/bench-core
git commit -m "feat: pass burn-in from system uptime"
```

### Task 4: Physical ports and link checks

**Files:**
- Create: `crates/bench-core/src/ports.rs`
- Modify: `crates/bench-core/src/lib.rs`

**Interfaces:**
- Consumes: nothing
- Produces:
  - `pub fn physical_interfaces(names: &[String]) -> Vec<String>`
  - `pub fn first_physical(names: &[String]) -> Option<String>`
  - `pub fn switch_port_passed(show_interface_status: &str, port: &str) -> bool`
  - `pub fn router_port_passed(show_interfaces: &str, port: &str) -> bool`

- [ ] **Step 1: Write the failing test**

```rust
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
}

#[test]
fn router_line_protocol_up_passes() {
    let up = "GigabitEthernet0/0/0 is up, line protocol is up";
    let down = "GigabitEthernet0/0/1 is up, line protocol is down";
    assert!(router_port_passed(up, "GigabitEthernet0/0/0"));
    assert!(!router_port_passed(down, "GigabitEthernet0/0/1"));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p bench-core --lib ports::`
Expected: FAIL to compile

- [ ] **Step 3: Implement the four functions in `ports.rs`**

Skip names that start with `Loopback`, `Vlan`, `Tunnel`, or `Virtual`. Match switch rows on the port token and status `connected` only. Match the router line for that exact interface.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p bench-core --lib ports::`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/bench-core
git commit -m "feat: classify ports and link status"
```

### Task 5: Firmware version token

**Files:**
- Create: `crates/bench-core/src/firmware.rs`
- Modify: `crates/bench-core/src/lib.rs`

**Interfaces:**
- Consumes: nothing
- Produces:
  - `pub enum FirmwareCheck { Pass, VersionMismatch, NoVersionToken }`
  - `pub fn version_token(filename: &str) -> Option<String>` — `17.12.05` from `cat9k_iosxe.17.12.05.SPA.bin`
  - `pub fn firmware_check(filename: &str, show_version: &str) -> FirmwareCheck`

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn image_filename_version_must_appear_in_show_version() {
    assert_eq!(firmware_check("cat9k_iosxe.17.12.05.SPA.bin", "Cisco IOS XE Software, Version 17.12.05"), FirmwareCheck::Pass);
    assert_eq!(firmware_check("cat9k_iosxe.17.12.05.SPA.bin", "Version 17.12.04"), FirmwareCheck::VersionMismatch);
    assert_eq!(firmware_check("image.bin", "Version 17.12.05"), FirmwareCheck::NoVersionToken);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p bench-core --lib image_filename_version_must_appear_in_show_version`
Expected: FAIL to compile

- [ ] **Step 3: Implement `version_token` and `firmware_check` in `firmware.rs`**

Take the first `\d+\.\d+\.\d+` token in the file name. `Pass` only when `show_version` contains that exact token.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p bench-core --lib image_filename_version_must_appear_in_show_version`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/bench-core
git commit -m "feat: match the image filename to show version"
```

### Task 6: Factory login

**Files:**
- Create: `crates/bench-core/src/login.rs`
- Modify: `crates/bench-core/src/lib.rs`

**Interfaces:**
- Consumes: nothing
- Produces:
  - `pub struct FactorySecrets { pub username: String, pub password: String, pub enable_secret: String }`
  - `pub enum LoginEvent { InitialDialog, UsernamePrompt, PasswordPrompt, ConfirmPassword, EnableSecretPrompt, Rejected { too_short: bool, raw: String }, Privileged }`
  - `pub enum LoginAction { Send(&'static str), Stop { reason: String }, InPrivileged { reminder: Option<FactorySecrets> } }`
  - `pub struct FactoryLogin { /* private */ }`
  - `impl FactoryLogin { pub fn new() -> Self; pub fn on_event(&mut self, event: LoginEvent) -> LoginAction }`

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn open_console_sends_no_then_enable() {
    let mut login = FactoryLogin::new();
    assert_eq!(login.on_event(LoginEvent::InitialDialog), LoginAction::Send("no"));
    assert!(matches!(login.on_event(LoginEvent::Privileged), LoginAction::InPrivileged { reminder: None }));
}

#[test]
fn short_password_retries_with_passw0rd_percent() {
    let mut login = FactoryLogin::new();
    login.on_event(LoginEvent::UsernamePrompt);
    login.on_event(LoginEvent::PasswordPrompt);
    let retry = login.on_event(LoginEvent::Rejected { too_short: true, raw: "% Password rejected. Too short.".into() });
    assert_eq!(retry, LoginAction::Send("Passw0rd%"));
    let done = login.on_event(LoginEvent::Privileged);
    match done {
        LoginAction::InPrivileged { reminder: Some(secrets) } => {
            assert_eq!(secrets.username, "cisco");
            assert_eq!(secrets.password, "Passw0rd%");
            assert_eq!(secrets.enable_secret, "Passw0rd%");
        }
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn a_rejection_that_is_not_too_short_stops() {
    let mut login = FactoryLogin::new();
    login.on_event(LoginEvent::PasswordPrompt);
    let action = login.on_event(LoginEvent::Rejected { too_short: false, raw: "% Password rejected.".into() });
    assert!(matches!(action, LoginAction::Stop { .. }));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p bench-core --lib login::`
Expected: FAIL to compile

- [ ] **Step 3: Implement `FactoryLogin` in `login.rs`**

`UsernamePrompt` sends `cisco`. The first `PasswordPrompt`, `ConfirmPassword`, and `EnableSecretPrompt` send `cisco`. After `Rejected { too_short: true }`, those three send `Passw0rd%`. A second rejection, or a rejection that is not too short, is `Stop` and includes `raw`. `InitialDialog` sends `no`. `Privileged` after an open console has `reminder: None`. `Privileged` after an account was entered has `reminder: Some` with the values the device accepted. `LoginAction` must derive `PartialEq` and `Debug`.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p bench-core --lib login::`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/bench-core
git commit -m "feat: answer factory console login"
```

### Task 7: Login after the configuration is applied

**Files:**
- Modify: `crates/bench-core/src/login.rs`

**Interfaces:**
- Consumes: `FactorySecrets` from Task 6
- Produces:
  - `pub enum NextLogin { Factory(FactorySecrets), User { username: String, password: String }, Stop { lines: Vec<String> } }`
  - `pub fn login_after_apply(uploaded: &str, factory: &FactorySecrets) -> NextLogin`
  - `pub fn decode_type7(encoded: &str) -> Option<String>`

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn hashed_enable_secret_stops_before_reboot() {
    let factory = FactorySecrets { username: "cisco".into(), password: "cisco".into(), enable_secret: "cisco".into() };
    let file = "enable secret 9 $9$abc\n";
    assert!(matches!(login_after_apply(file, &factory), NextLogin::Stop { .. }));
}

#[test]
fn one_privilege_15_user_is_used() {
    let factory = FactorySecrets { username: "cisco".into(), password: "cisco".into(), enable_secret: "cisco".into() };
    let file = "username bench privilege 15 password BenchPass\n";
    match login_after_apply(file, &factory) {
        NextLogin::User { username, password } => {
            assert_eq!(username, "bench");
            assert_eq!(password, "BenchPass");
        }
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn type7_cisco_decodes() {
    assert_eq!(decode_type7("02050D480809").as_deref(), Some("cisco"));
}

#[test]
fn unchanged_console_keeps_the_factory_login() {
    let factory = FactorySecrets { username: "cisco".into(), password: "Passw0rd%".into(), enable_secret: "Passw0rd%".into() };
    match login_after_apply("hostname closet-2-sw1\n", &factory) {
        NextLogin::Factory(secrets) => assert_eq!(secrets.password, "Passw0rd%"),
        other => panic!("unexpected {other:?}"),
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p bench-core --lib login::`
Expected: FAIL to compile

- [ ] **Step 3: Implement `decode_type7` and `login_after_apply`**

Type 7: the first two hex digits are an index into `b"dsfd;kfoA,.iyewrkldJKDHSUBsgvca69834ncx"`. Each following pair is one password byte XORed with the key from that index, wrapping the key. `02050D480809` is `cisco`.

`login_after_apply` returns `Factory` when the file has no `username`, `enable secret`, `enable password`, `aaa authentication`, or `line con` password. It returns `User` for exactly one `privilege 15` user with a typable password (cleartext, `secret 0`, or type 7). It returns `Stop` with the offending lines for a hash-only secret, more than one typable privilege-15 user, or `aaa authentication login` on the console. A lone typable `line con 0` / `password` is `User` with an empty username.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p bench-core --lib login::`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/bench-core
git commit -m "feat: choose the post-apply console login"
```

### Task 8: Validation export

**Files:**
- Create: `crates/bench-core/src/export.rs`
- Modify: `crates/bench-core/src/lib.rs`

**Interfaces:**
- Consumes: `CheckStatus` from Task 1
- Produces:
  - `pub struct ExportInput { pub hostname: String, pub serial: String, pub config_file: String, pub configuration: CheckStatus, pub firmware_file: Option<String>, pub running_version: Option<String>, pub burn_in_hours: Option<u32>, pub uptime_line: Option<String>, pub burn_in: CheckStatus, pub ports: Vec<(String, CheckStatus)> }`
  - `pub fn export_text(input: &ExportInput) -> String`
  - `pub fn chassis_serial(show_inventory: &str, show_version: &str) -> Option<String>`

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn export_quotes_serial_uptime_and_ports() {
    let text = export_text(&ExportInput {
        hostname: "closet-2-sw1".into(),
        serial: "FOC2345L0AB".into(),
        config_file: "closet-access.cfg".into(),
        configuration: CheckStatus::Passed,
        firmware_file: None,
        running_version: None,
        burn_in_hours: Some(4),
        uptime_line: Some("closet-2-sw1 uptime is 4 hours, 6 minutes".into()),
        burn_in: CheckStatus::Passed,
        ports: vec![("Gi1/0/1".into(), CheckStatus::Passed)],
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
    assert_eq!(chassis_serial(inventory, "").as_deref(), Some("FOC2345L0AB"));
    assert_eq!(chassis_serial("", "Processor board ID FCW2145G0AB").as_deref(), Some("FCW2145G0AB"));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p bench-core --lib export_quotes_serial_uptime_and_ports`
Expected: FAIL to compile

- [ ] **Step 3: Implement `export_text` in `export.rs`**

Write a plain-text report. Include firmware lines only when `firmware_file` is `Some`. Include burn-in lines only when `burn_in` is not `NotChosen`. Include a port section only when `ports` is non-empty. A `Failed` configuration lists `detail`. `chassis_serial` returns the first `SN:` value in show inventory, otherwise the token after `Processor board ID`.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p bench-core --lib export_quotes_serial_uptime_and_ports`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/bench-core
git commit -m "feat: render the validation export"
```

### Task 9: Run order

**Files:**
- Create: `crates/bench-core/src/phase.rs`
- Modify: `crates/bench-core/src/lib.rs`
- Modify: `crates/bench-core/src/run.rs` (use `RunOptions`)

**Interfaces:**
- Consumes: `RunOptions` from Task 1
- Produces:
  - `pub enum Phase { Login, NameTftpPort, CopyImage, InstallAndReboot, ConfirmVersion, ApplyConfig, SaveAndReboot, ValidateConfig, CleanReboot, WaitBurnIn, ReadUptime, Done, Stopped }`
  - `pub enum PhaseEvent { LoggedIn, CableConfirmed, ImageOnFlash, Rebooted, VersionMatched, VersionRejected, ConfigApplied, Saved, ComparisonPassed, ComparisonFailed, WaitFinished, UptimePassed, UptimeFailed }`
  - `pub fn start_phase(options: &RunOptions) -> Phase`
  - `pub fn next_phase(current: Phase, options: &RunOptions, event: PhaseEvent) -> Phase`
  - `pub fn port_walk_available(current: Phase) -> bool` — true at `CleanReboot`, `WaitBurnIn`, `ReadUptime`, and `Done`

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn firmware_run_reboots_three_times_before_the_wait() {
    let options = sample_with_firmware_and_burn_in();
    let mut phase = start_phase(&options);
    assert_eq!(phase, Phase::Login);
    for event in [PhaseEvent::LoggedIn, PhaseEvent::CableConfirmed, PhaseEvent::ImageOnFlash, PhaseEvent::Rebooted, PhaseEvent::LoggedIn, PhaseEvent::VersionMatched, PhaseEvent::ConfigApplied, PhaseEvent::Saved, PhaseEvent::Rebooted, PhaseEvent::LoggedIn, PhaseEvent::ComparisonPassed, PhaseEvent::Rebooted, PhaseEvent::WaitFinished, PhaseEvent::UptimePassed] {
        phase = next_phase(phase, &options, event);
    }
    assert_eq!(phase, Phase::Done);
    assert!(port_walk_available(Phase::WaitBurnIn));
    assert!(!port_walk_available(Phase::ApplyConfig));
}

#[test]
fn without_firmware_the_run_starts_at_apply() {
    let options = sample_config_only();
    assert_eq!(start_phase(&options), Phase::ApplyConfig);
}
```

Define `sample_with_firmware_and_burn_in` and `sample_config_only` in the same test module. `sample_config_only` has `firmware: None` and `burn_in_hours: None`.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p bench-core --lib phase::`
Expected: FAIL to compile

- [ ] **Step 3: Implement the phase functions in `phase.rs`**

With firmware, the successful event order is the list in the test: login, cable, copy, install reboot, login, version, apply, save, reboot, login, comparison, clean reboot, wait, uptime. Without firmware, start at `ApplyConfig` and then use the same tail. `ComparisonFailed`, `VersionRejected`, and `UptimeFailed` go to `Stopped`. When burn-in is off, the clean reboot’s next successful event goes to `Done` instead of `WaitBurnIn`.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p bench-core --lib phase::`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/bench-core
git commit -m "feat: encode the console run order"
```

### Task 10: TFTP link commands

**Files:**
- Create: `crates/bench-core/src/transfer.rs`
- Modify: `crates/bench-core/src/lib.rs`

**Interfaces:**
- Consumes: nothing
- Produces:
  - `pub enum LinkKind { Switch, Router }`
  - `pub fn device_address() -> &'static str` — `"192.0.2.2"`
  - `pub fn server_address() -> &'static str` — `"192.0.2.1"`
  - `pub fn mask() -> &'static str` — `"255.255.255.252"`
  - `pub fn assign_address_commands(kind: LinkKind, interface: &str) -> Vec<String>`
  - `pub fn clear_address_commands(interface: &str) -> Vec<String>`
  - `pub fn copy_tftp_command(filename: &str) -> String`
  - `pub fn save_startup_command() -> &'static str` — `copy running-config startup-config`

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn switch_assignment_is_routed_and_uses_the_documentation_prefix() {
    let cmds = assign_address_commands(LinkKind::Switch, "GigabitEthernet1/0/1");
    assert!(cmds.iter().any(|c| c == "no switchport"));
    assert!(cmds.iter().any(|c| c == "ip address 192.0.2.2 255.255.255.252"));
    assert!(!cmds.iter().any(|c| c.contains("write")));
}

#[test]
fn router_assignment_has_no_switchport_command() {
    let cmds = assign_address_commands(LinkKind::Router, "GigabitEthernet0/0/0");
    assert!(!cmds.iter().any(|c| c == "no switchport"));
}

#[test]
fn copy_targets_the_laptop_address_and_flash() {
    assert_eq!(copy_tftp_command("cat9k_iosxe.17.12.05.SPA.bin"), "copy tftp://192.0.2.1/cat9k_iosxe.17.12.05.SPA.bin flash:");
    assert_eq!(save_startup_command(), "copy running-config startup-config");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p bench-core --lib transfer::`
Expected: FAIL to compile

- [ ] **Step 3: Implement the command builders in `transfer.rs`**

Both kinds emit `configure terminal`, `interface <name>`, the address, `no shutdown`, and `end`. Only `LinkKind::Switch` emits `no switchport`. `clear_address_commands` emits `no ip address` on that interface and does not write memory.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p bench-core --lib transfer::`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/bench-core
git commit -m "feat: build the temporary TFTP address commands"
```

### Task 11: Start-of-run form

**Files:**
- Create: `crates/bench-core/src/form.rs`
- Modify: `crates/bench-core/src/lib.rs`

**Interfaces:**
- Consumes: `RunOptions`, `FirmwareChoice`, `validate_burn_in_hours` from Task 1
- Produces:
  - `pub struct StartForm { pub serial_device: String, pub config_path: String, pub firmware_on: bool, pub image_path: String, pub local_ethernet: String, pub burn_in_on: bool, pub hours: u32, pub port_testing: bool }`
  - `impl StartForm { pub fn new() -> Self; pub fn step_hours(&mut self, delta: i32); pub fn to_options(&self) -> Result<RunOptions, &'static str> }`

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn hours_step_by_one_and_never_below_one() {
    let mut form = StartForm::new();
    form.burn_in_on = true;
    form.step_hours(-1);
    assert_eq!(form.hours, 1);
    form.step_hours(1);
    assert_eq!(form.hours, 2);
}

#[test]
fn firmware_off_omits_the_image() {
    let mut form = StartForm::new();
    form.serial_device = "/dev/tty.usbserial".into();
    form.config_path = "closet-access.cfg".into();
    form.firmware_on = false;
    let options = form.to_options().unwrap();
    assert!(options.firmware.is_none());
    assert!(!options.port_testing);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p bench-core --lib form::`
Expected: FAIL to compile

- [ ] **Step 3: Implement `StartForm` in `form.rs`**

`new` sets `hours` to 1 and both toggles off. `step_hours` changes `hours` by 1 and clamps at 1. `to_options` returns an error if `serial_device` or `config_path` is empty, or if `firmware_on` and either path is empty. `burn_in_hours` is `Some(hours)` only when `burn_in_on`.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p bench-core --lib form::`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/bench-core
git commit -m "feat: validate the start-of-run choices"
```

### Task 12: Desktop window

**Files:**
- Create: `crates/bench-app/Cargo.toml`
- Create: `crates/bench-app/src/main.rs`
- Create: `crates/bench-app/assets/app.css`
- Modify: `Cargo.toml` (workspace member)

**Interfaces:**
- Consumes: `RunRecord`, `CheckStatus`, `ready_to_release`, `FactorySecrets`, `StartForm` from `bench-core`
- Produces: a `dx`/`cargo` desktop binary `bench-app` that renders the session

- [ ] **Step 1: Write the failing test**

Add `crates/bench-core/src/present.rs` with:

```rust
pub fn release_heading(record: &RunRecord) -> &'static str
pub fn reminder_line(secrets: &FactorySecrets) -> String
```

Test:

```rust
#[test]
fn heading_and_reminder_use_the_accepted_secrets() {
    let record = RunRecord {
        configuration: CheckStatus::Passed,
        firmware: CheckStatus::NotChosen,
        burn_in: CheckStatus::NotChosen,
        ports: CheckStatus::NotChosen,
    };
    assert_eq!(release_heading(&record), "Ready to release");
    let secrets = FactorySecrets { username: "cisco".into(), password: "Passw0rd%".into(), enable_secret: "Passw0rd%".into() };
    let line = reminder_line(&secrets);
    assert!(line.contains("cisco"));
    assert!(line.contains("Passw0rd%"));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p bench-core --lib heading_and_reminder_use_the_accepted_secrets`
Expected: FAIL to compile

- [ ] **Step 3: Implement `present.rs` and the Dioxus shell**

`release_heading` returns `"Ready to release"` when `ready_to_release` is true, otherwise `"Not ready to release"`. `reminder_line` is `Username {username} · password {password} · enable secret {enable_secret}`.

The shell depends on `dioxus` 0.6 with feature `desktop`. One window: dark background near `#181818`, a narrow left activity bar, a sidebar labeled `On the console`, a main record of the four checks, the reminder across the top when secrets exist, and a status bar reading `Console` and `9600 8N1`. The start form uses `StartForm` for the hour stepper and the firmware, burn-in, and port-testing toggles. File and serial pickers call the platform dialogs; their selected paths go into `StartForm`. While `port_walk_available` is true and port testing was chosen, the main area names one physical port, says to connect the secondary switch, and offers Check this port. While the phase is `NameTftpPort`, the main area names `first_physical` and offers Cable is in. Apply sends `configure terminal`, the file body, and `end`. Save sends `save_startup_command()` and only then the reboot.

- [ ] **Step 4: Run test and build**

Run: `cargo test -p bench-core --lib heading_and_reminder_use_the_accepted_secrets && cargo build -p bench-app`
Expected: PASS, and `bench-app` builds

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml crates/bench-core crates/bench-app
git commit -m "feat: open the bench window on the run record"
```

### Task 13: Serial settings and TFTP serve

**Files:**
- Create: `crates/bench-core/src/console_link.rs`
- Create: `crates/bench-app/src/tftp.rs`
- Modify: `crates/bench-app/Cargo.toml` (add `serialport`)
- Modify: `crates/bench-core/src/lib.rs`

**Interfaces:**
- Consumes: `server_address`, `copy_tftp_command` from Task 10
- Produces:
  - `pub struct SerialSettings { pub baud: u32, pub data_bits: u8, pub parity: &'static str, pub stop_bits: u8 }`
  - `pub fn cisco_console() -> SerialSettings` — 9600, 8, `"none"`, 1
  - `pub fn tftp_rrq_response(filename: &str, served_name: &str, file_bytes: &[u8]) -> Option<Vec<u8>>` — `Some` DATA packet only when `filename == served_name`

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn console_settings_are_9600_8n1() {
    let settings = cisco_console();
    assert_eq!((settings.baud, settings.data_bits, settings.parity, settings.stop_bits), (9600, 8, "none", 1));
}

#[test]
fn tftp_serves_only_the_chosen_image() {
    assert!(tftp_rrq_response("other.bin", "cat9k_iosxe.17.12.05.SPA.bin", b"img").is_none());
    assert!(tftp_rrq_response("cat9k_iosxe.17.12.05.SPA.bin", "cat9k_iosxe.17.12.05.SPA.bin", b"img").is_some());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p bench-core --lib console_link::`
Expected: FAIL to compile. Both functions live in `bench-core` so this filter covers them. `bench-app` calls them.

- [ ] **Step 3: Implement settings, the RRQ check, and the adapters**

`bench-app` opens the chosen serial port with `cisco_console()` via `serialport` on both macOS and Windows. The TFTP socket binds `192.0.2.1:69` on the chosen ethernet port when firmware is on, and answers a read of the chosen filename with the file bytes. It does not answer any other name.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p bench-core --lib console_link:: && cargo build -p bench-app`
Expected: PASS, and `bench-app` builds

- [ ] **Step 5: Commit**

```bash
git add crates/bench-core crates/bench-app
git commit -m "feat: open the console and serve the image over TFTP"
```
