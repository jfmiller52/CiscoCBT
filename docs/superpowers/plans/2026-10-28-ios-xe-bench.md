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
- A device that rejects `cisco` reprints its prompt. The fallback must answer that prompt, not race it. Pinned in Tasks 6 and 14.
- A file line such as `no shutdown` or `exit` never appears in a running config. It must not fail the configuration check. Pinned in Task 2.

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
- `crates/bench-core/src/present.rs` — headings, status labels, and the password reminder line
- `crates/bench-core/src/console_link.rs` — 9600 8N1
- `crates/bench-core/src/tftp.rs` — TFTP packets and block numbering
- `crates/bench-core/src/session.rs` — the `Console` trait and one IOS-XE console session
- `crates/bench-core/src/scripted.rs` — a scripted `Console` for tests
- `crates/bench-core/src/runner.rs` — the phases carried out on a device
- `crates/bench-app/src/view.rs` — window state and commands
- `crates/bench-app/src/serial.rs`, `tftp.rs`, `nic.rs`, `awake.rs`, `worker.rs` — the real cables and the run thread
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
git add Cargo.toml Cargo.lock crates/bench-core
git commit -m "feat: add bench run record and hour minimum"
```

### Task 2: Configuration presence

**Files:**
- Create: `crates/bench-core/src/config_check.rs`
- Modify: `crates/bench-core/src/lib.rs`

**Interfaces:**
- Consumes: nothing from Task 1
- Produces:
  - `pub fn missing_commands(uploaded: &str, running: &str) -> Vec<String>` — each missing command’s original line, in file order
  - `pub fn canonical_interface_name(name: &str) -> String` — `Gi1/0/3` becomes `GigabitEthernet1/0/3`; a full name is returned unchanged

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

#[test]
fn navigation_lines_and_negations_are_not_missing() {
    let file = "configure terminal\ninterface GigabitEthernet1/0/1\n no shutdown\n exit\nno ip http server\nend\n";
    let running = "interface GigabitEthernet1/0/1\n switchport mode access\n";
    assert!(missing_commands(file, running).is_empty());
    assert_eq!(canonical_interface_name("Gi1/0/3"), "GigabitEthernet1/0/3");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p bench-core --lib config_check::`
Expected: FAIL to compile

- [ ] **Step 3: Implement `missing_commands` in `config_check.rs`**

Ignore blank lines, `!` comments, and running-config header lines (`Building configuration`, `Current configuration :`, `Last configuration change`). `configure terminal`, `conf t`, `end`, and `exit` are navigation, not commands. A line containing the word `password` or `secret` is present when the same section has a running line with the same words up to and including that keyword, whatever type number and value follow, so a cleartext secret matches its stored hash. A `no X` line is present when the same section has `no X` or does not have `X`. `interface range GigabitEthernet1/0/1 - 2` is present when each member exists and carries the range’s sub-commands. Interface names compare by `canonical_interface_name`, which expands `Gi`, `Te`, `Fa`, `Ap`, `Po`, `Twe`, `Fo`, and `Hu` to `GigabitEthernet`, `TenGigabitEthernet`, `FastEthernet`, `AppGigabitEthernet`, `Port-channel`, `TwentyFiveGigE`, `FortyGigabitEthernet`, and `HundredGigE`.

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
- Consumes: `canonical_interface_name` from Task 2
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
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p bench-core --lib ports::`
Expected: FAIL to compile

- [ ] **Step 3: Implement the four functions in `ports.rs`**

Skip names that start with `Loopback`, `Vlan`, `Tunnel`, `Virtual`, `Port-channel`, `BDI`, `NVI`, `Null`, `Dialer`, `AppGigabitEthernet`, or `Bluetooth`. Keep the device’s order. Compare port names by `canonical_interface_name`. On a switch, read the status from the `Status` column located by the header row when the header is present; without a header, the row passes only on a token exactly `connected`. Match the router line for that exact interface.

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

Run: `cargo test -p bench-core --lib firmware::`
Expected: FAIL to compile

- [ ] **Step 3: Implement `version_token` and `firmware_check` in `firmware.rs`**

Take the first `\d+\.\d+\.\d+` token in the file name. `Pass` only when `show_version` contains that exact token.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p bench-core --lib firmware::`
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
  - `pub struct FactorySecrets { pub username: String, pub password: String, pub enable_secret: String }` — derives `Debug, Clone, PartialEq, Eq`
  - `pub enum LoginEvent { InitialDialog, UsernamePrompt, PasswordPrompt, ConfirmPassword, EnableSecretPrompt, UserExec, Rejected { too_short: bool, raw: String }, Privileged }`
  - `pub enum LoginAction { Send(&'static str), Wait, Stop { reason: String }, InPrivileged { reminder: Option<FactorySecrets> } }`
  - `pub struct FactoryLogin { /* private */ }`
  - `impl FactoryLogin { pub fn new() -> Self; pub fn on_event(&mut self, event: LoginEvent) -> LoginAction }`

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn open_console_sends_no_then_enable() {
    let mut login = FactoryLogin::new();
    assert_eq!(login.on_event(LoginEvent::InitialDialog), LoginAction::Send("no"));
    assert_eq!(login.on_event(LoginEvent::UserExec), LoginAction::Send("enable"));
    assert!(matches!(login.on_event(LoginEvent::Privileged), LoginAction::InPrivileged { reminder: None }));
}

#[test]
fn short_password_retries_with_passw0rd_percent() {
    let mut login = FactoryLogin::new();
    login.on_event(LoginEvent::UsernamePrompt);
    login.on_event(LoginEvent::PasswordPrompt);
    let retry = login.on_event(LoginEvent::Rejected { too_short: true, raw: "% Password rejected. Too short.".into() });
    assert_eq!(retry, LoginAction::Wait);
    assert_eq!(login.on_event(LoginEvent::PasswordPrompt), LoginAction::Send("Passw0rd%"));
    assert_eq!(login.on_event(LoginEvent::ConfirmPassword), LoginAction::Send("Passw0rd%"));
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

`UsernamePrompt` sends `cisco`. The first `PasswordPrompt`, `ConfirmPassword`, and `EnableSecretPrompt` send `cisco`. `Rejected { too_short: true }` switches the password and the enable secret together to `Passw0rd%` and returns `Wait`, because the device prints its own prompt again after a rejection; later password, confirm, and enable-secret prompts send `Passw0rd%`. A second rejection, or a rejection that is not too short, is `Stop` and includes `raw`. `InitialDialog` sends `no`. `UserExec` (a `>` prompt) sends `enable`. `Privileged` after an open console has `reminder: None`. `Privileged` after any account or secret prompt has `reminder: Some` with username `cisco` and the password and enable secret currently in use. `LoginAction` must derive `PartialEq` and `Debug`.

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
  - `pub enum NextLogin { Factory(FactorySecrets), User { username: String, password: String, enable_secret: String }, Stop { lines: Vec<String> } }` — derives `Debug, Clone, PartialEq, Eq`
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
        NextLogin::User { username, password, enable_secret } => {
            assert_eq!(username, "bench");
            assert_eq!(password, "BenchPass");
            assert_eq!(enable_secret, "cisco");
        }
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn two_typable_privilege_15_users_stop() {
    let factory = FactorySecrets { username: "cisco".into(), password: "cisco".into(), enable_secret: "cisco".into() };
    let file = "username a privilege 15 secret 0 One\nusername b privilege 15 secret 0 Two\n";
    match login_after_apply(file, &factory) {
        NextLogin::Stop { lines } => assert_eq!(lines.len(), 2),
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

A value is typable when it has no type number, type `0`, or type `7` (decoded), and does not begin with `$1$`, `$5$`, `$8$`, or `$9$`. Types `5`, `8`, and `9` are hashes. Apply these rules in order. `Stop.lines` holds the offending file lines verbatim.

1. Any `aaa authentication login` line: `Stop`.
2. More than one `username … privilege 15` line with a typable value: `Stop` with those lines.
3. Exactly one: `User` with that username and value. Its `enable_secret` is the file’s typable `enable secret` (or `enable password` when there is no `enable secret`), otherwise the factory enable secret. A hashed enable value does not stop this case, because a privilege-15 login lands at `#`.
4. Any `username` line but no typable privilege-15 user: `Stop` with the `username` lines.
5. A `password` under `line con 0` that is hashed, or an `enable secret` / `enable password` that is hashed: `Stop` with that line.
6. The file sets none of `username`, a `line con 0` password, `enable secret`, or `enable password`: `Factory` with the factory secrets.
7. Otherwise `User`. Fields the file does not set keep the factory values: username is the factory username, password is the typable `line con 0` password or the factory password, and `enable_secret` is the typable enable value or the factory enable secret.

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
  - `pub struct ExportInput { pub hostname: String, pub serial: String, pub config_file: String, pub configuration: CheckStatus, pub firmware_file: Option<String>, pub running_version: Option<String>, pub firmware: CheckStatus, pub burn_in_hours: Option<u32>, pub uptime_line: Option<String>, pub burn_in: CheckStatus, pub ports: Vec<(String, CheckStatus)> }`
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

Run: `cargo test -p bench-core --lib export::`
Expected: FAIL to compile

- [ ] **Step 3: Implement `export_text` in `export.rs`**

Write a plain-text report. Include firmware lines only when `firmware_file` is `Some`. Include burn-in lines only when `burn_in` is not `NotChosen`. Include a port section only when `ports` is non-empty. A `Failed` configuration lists `detail`. `chassis_serial` returns the first `SN:` value in show inventory, otherwise the token after `Processor board ID`.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p bench-core --lib export::`
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

**Interfaces:**
- Consumes: `RunOptions` from Task 1
- Produces:
  - `pub enum Phase { Login, NameTftpPort, CopyImage, InstallAndReboot, ConfirmVersion, ApplyConfig, SaveAndReboot, ValidateConfig, CleanReboot, WaitBurnIn, ReadUptime, Done, Stopped }` — derives `Debug, Clone, Copy, PartialEq, Eq`
  - `pub enum PhaseEvent { LoggedIn, CableConfirmed, ImageOnFlash, Rebooted, VersionMatched, VersionRejected, ConfigApplied, Saved, ComparisonPassed, ComparisonFailed, WaitFinished, UptimePassed, UptimeFailed }` — derives `Debug, Clone, Copy, PartialEq, Eq`
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
fn without_firmware_login_leads_to_apply_and_ends_after_the_clean_reboot() {
    let options = sample_config_only();
    let mut phase = start_phase(&options);
    assert_eq!(phase, Phase::Login);
    phase = next_phase(phase, &options, PhaseEvent::LoggedIn);
    assert_eq!(phase, Phase::ApplyConfig);
    for event in [PhaseEvent::ConfigApplied, PhaseEvent::Saved, PhaseEvent::Rebooted, PhaseEvent::LoggedIn, PhaseEvent::ComparisonPassed, PhaseEvent::Rebooted] {
        phase = next_phase(phase, &options, event);
    }
    assert_eq!(phase, Phase::Done);
}

#[test]
fn a_failed_comparison_stops_before_the_clean_reboot() {
    let options = sample_config_only();
    assert_eq!(next_phase(Phase::ValidateConfig, &options, PhaseEvent::ComparisonFailed), Phase::Stopped);
}
```

Define `sample_with_firmware_and_burn_in` and `sample_config_only` in the same test module. `sample_config_only` has `firmware: None` and `burn_in_hours: None`.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p bench-core --lib phase::`
Expected: FAIL to compile

- [ ] **Step 3: Implement the phase functions in `phase.rs`**

Every run starts at `Login`. Transitions:

| Phase | Event | Next |
|---|---|---|
| `Login` | `LoggedIn` | `NameTftpPort` with firmware, otherwise `ApplyConfig` |
| `NameTftpPort` | `CableConfirmed` | `CopyImage` |
| `CopyImage` | `ImageOnFlash` | `InstallAndReboot` |
| `InstallAndReboot` | `Rebooted` | `ConfirmVersion` |
| `ConfirmVersion` | `VersionMatched` / `VersionRejected` | `ApplyConfig` / `Stopped` |
| `ApplyConfig` | `ConfigApplied` | `SaveAndReboot` |
| `SaveAndReboot` | `Rebooted` | `ValidateConfig` |
| `ValidateConfig` | `ComparisonPassed` / `ComparisonFailed` | `CleanReboot` / `Stopped` |
| `CleanReboot` | `Rebooted` | `WaitBurnIn` with burn-in, otherwise `Done` |
| `WaitBurnIn` | `WaitFinished` | `ReadUptime` |
| `ReadUptime` | `UptimePassed` / `UptimeFailed` | `Done` / `Stopped` |

`LoggedIn` outside `Login`, and `Saved` in `SaveAndReboot`, leave the phase unchanged. Any other pair leaves the phase unchanged.

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
    assert!(cmds.iter().any(|c| c == "ip tftp source-interface GigabitEthernet0/0/0"));
}

#[test]
fn switch_management_port_is_already_routed() {
    let cmds = assign_address_commands(LinkKind::Switch, "GigabitEthernet0/0");
    assert!(!cmds.iter().any(|c| c == "no switchport"));
    assert!(cmds.iter().any(|c| c == "ip tftp source-interface GigabitEthernet0/0"));
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

Both kinds emit, in order: `configure terminal`, `interface <name>`, `no switchport` (switch only), `ip address 192.0.2.2 255.255.255.252`, `no shutdown`, `exit`, `ip tftp source-interface <name>`, `end`. `no switchport` is omitted for `GigabitEthernet0/0` on a switch, because that is the Catalyst management port and is already routed. `clear_address_commands` emits `configure terminal`, `interface <name>`, `no ip address`, `exit`, `no ip tftp source-interface`, `end`. Neither writes memory.

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
- Create: `crates/bench-core/src/present.rs`
- Modify: `crates/bench-core/src/lib.rs`
- Create: `crates/bench-app/Cargo.toml`
- Create: `crates/bench-app/src/main.rs`
- Create: `crates/bench-app/src/view.rs`
- Create: `crates/bench-app/assets/app.css`
- Modify: `Cargo.toml` (workspace member)

**Interfaces:**
- Consumes: `RunRecord`, `CheckStatus`, `ready_to_release`, `RunOptions` (Task 1); `FactorySecrets` (Task 6); `Phase`, `port_walk_available` (Task 9); `StartForm` (Task 11)
- Produces:
  - `pub fn release_heading(record: &RunRecord) -> &'static str`
  - `pub fn reminder_line(secrets: &FactorySecrets) -> String`
  - `pub fn status_label(status: &CheckStatus) -> &'static str` — `"Passed"`, `"Failed"`, `"Not finished"`, `"Not chosen"`
  - In `bench-app/src/view.rs`: `pub struct ViewState { pub form: StartForm, pub serial_devices: Vec<String>, pub ethernet_ports: Vec<String>, pub phase: Option<Phase>, pub record: RunRecord, pub reminder: Option<FactorySecrets>, pub tftp_port: Option<String>, pub ports: Vec<(String, CheckStatus)>, pub current_port: Option<String>, pub burn_in_remaining: Option<std::time::Duration>, pub transcript: String, pub export_text: Option<String>, pub stop_reason: Option<String> }` and `pub enum UiCommand { Start(RunOptions), ConfirmCable, CheckPort(String), Export }`. The window renders a `ViewState` and sends `UiCommand`s on a channel. Task 16 connects that channel to the device.

- [ ] **Step 1: Write the failing test**

In `crates/bench-core/src/present.rs`:

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
    assert_eq!(status_label(&CheckStatus::NotFinished), "Not finished");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p bench-core --lib present::`
Expected: FAIL to compile

- [ ] **Step 3: Implement `present.rs` and the Dioxus shell**

`release_heading` returns `"Ready to release"` when `ready_to_release` is true, otherwise `"Not ready to release"`. `reminder_line` is `Username {username} · password {password} · enable secret {enable_secret}`.

The shell depends on `dioxus` 0.6 with feature `desktop`, and `rfd` for the configuration file, image file, and export save dialogs. One window: dark background near `#181818`, a narrow left activity bar, a sidebar labeled `On the console` showing the serial device, host name, and chosen files, a main area, the reminder across the top whenever `reminder` is `Some`, and a status bar reading `Console` and `9600 8N1`. Main area by state:

- `phase` is `None`: the start form. Serial device and ethernet port are selects filled from `serial_devices` and `ethernet_ports`. Choose file buttons for the configuration and the image. Toggles for firmware, burn-in, and port testing. When burn-in is on, a number field with minus and plus that call `StartForm::step_hours`. Start sends `UiCommand::Start` with `to_options()`, or shows its error.
- `phase` is `NameTftpPort`: `Plug the ethernet cable from this computer’s TFTP port into`, the `tftp_port` name large, and Cable is in, which sends `ConfirmCable`.
- Port testing chosen, `port_walk_available(phase)`, and `current_port` is `Some`: `Connect the secondary switch`, the port name large, Check this port, which sends `CheckPort`, and the ports already passed.
- Otherwise: `release_heading`, then one row per check with `status_label`, the burn-in hours and remaining time when burn-in is on, and the port grid when ports were chosen.

`stop_reason` shows as a banner above the main area. A Console toggle shows `transcript` in a monospace panel. Export validation is enabled once `record.configuration` is `Passed`; it sends `Export`, then saves `export_text` through a save dialog when it arrives. In this task `main` renders a `ViewState` with empty lists and discards commands; Task 16 replaces that.

- [ ] **Step 4: Run test and build**

Run: `cargo test -p bench-core --lib present:: && cargo build -p bench-app`
Expected: PASS, and `bench-app` builds

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock crates/bench-core crates/bench-app
git commit -m "feat: open the bench window on the run record"
```

### Task 13: Serial settings and TFTP packets

**Files:**
- Create: `crates/bench-core/src/console_link.rs`
- Create: `crates/bench-core/src/tftp.rs`
- Modify: `crates/bench-core/src/lib.rs`

**Interfaces:**
- Consumes: nothing
- Produces:
  - `pub struct SerialSettings { pub baud: u32, pub data_bits: u8, pub parity: &'static str, pub stop_bits: u8 }`
  - `pub fn cisco_console() -> SerialSettings` — 9600, 8, `"none"`, 1
  - `pub struct ReadRequest { pub filename: String, pub blksize: Option<u16> }` — derives `Debug, Clone, PartialEq, Eq`
  - `pub fn parse_rrq(packet: &[u8]) -> Option<ReadRequest>` — opcode 1, mode `octet` only (case-insensitive), optional RFC 2348 `blksize`
  - `pub fn serves(request: &ReadRequest, chosen: &str) -> bool` — exact filename match
  - `pub fn negotiated_blksize(requested: Option<u16>) -> u16` — 512 when absent; otherwise the request clamped to 8..=8192
  - `pub fn oack_packet(blksize: u16) -> Vec<u8>`
  - `pub fn data_packet(block: u16, payload: &[u8]) -> Vec<u8>`
  - `pub fn error_packet(code: u16, message: &str) -> Vec<u8>`
  - `pub fn parse_ack(packet: &[u8]) -> Option<u16>`
  - `pub fn block_count(size: u64, blksize: u16) -> u64` — `size / blksize + 1`, because the last block is always shorter than `blksize`, possibly empty
  - `pub fn wire_block(index: u64) -> u16` — 1-based block index to the 16-bit wire number, wrapping

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn console_settings_are_9600_8n1() {
    let settings = cisco_console();
    assert_eq!((settings.baud, settings.data_bits, settings.parity, settings.stop_bits), (9600, 8, "none", 1));
}

#[test]
fn rrq_parses_blksize_and_serves_only_the_chosen_image() {
    let request = parse_rrq(b"\x00\x01cat9k_iosxe.17.12.05.SPA.bin\x00octet\x00blksize\x008192\x00").unwrap();
    assert_eq!(request.blksize, Some(8192));
    assert!(serves(&request, "cat9k_iosxe.17.12.05.SPA.bin"));
    assert!(!serves(&request, "other.bin"));
    assert!(parse_rrq(b"\x00\x01x.bin\x00netascii\x00").is_none());
}

#[test]
fn blocks_wrap_and_always_end_short() {
    assert_eq!(block_count(1024, 512), 3);
    assert_eq!(block_count(1000, 512), 2);
    assert_eq!(wire_block(65535), 65535);
    assert_eq!(wire_block(65536), 0);
    assert_eq!(wire_block(65537), 1);
    assert_eq!(data_packet(1, b"ab"), vec![0, 3, 0, 1, b'a', b'b']);
    assert_eq!(parse_ack(&[0, 4, 0, 7]), Some(7));
    assert_eq!(negotiated_blksize(Some(65000)), 8192);
}
```

The console test lives in `console_link.rs`; the other two live in `tftp.rs`.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p bench-core --lib -- console_link:: tftp::`
Expected: FAIL to compile

- [ ] **Step 3: Implement both modules**

Packet layouts follow RFC 1350 and RFC 2348: DATA is opcode 3, ACK 4, ERROR 5 with a zero-terminated message, OACK 6 with `blksize\0<n>\0`.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p bench-core --lib -- console_link:: tftp::`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/bench-core
git commit -m "feat: add console settings and TFTP packet rules"
```

### Task 14: Console session

**Files:**
- Create: `crates/bench-core/src/session.rs`
- Create: `crates/bench-core/src/scripted.rs`
- Modify: `crates/bench-core/Cargo.toml` (add `regex`)
- Modify: `crates/bench-core/src/lib.rs`

**Interfaces:**
- Consumes: `FactoryLogin`, `LoginEvent`, `LoginAction`, `FactorySecrets` (Task 6); `NextLogin` (Task 7); `save_startup_command` (Task 10)
- Produces:
  - `pub trait Console { fn write(&mut self, text: &str) -> std::io::Result<()>; fn read_until(&mut self, patterns: &[regex::Regex], timeout: std::time::Duration) -> std::io::Result<(usize, String)>; }` — `read_until` consumes unread output through the end of the match that ends first (ties go to the lower index) and returns that pattern’s index and the consumed text. It fails with `ErrorKind::TimedOut` when nothing matches in time.
  - `pub enum SessionError { Io(String), Timeout { waiting_for: String }, Stopped(String) }` — derives `Debug, Clone, PartialEq, Eq`
  - `pub struct Session<C: Console>` with:
    - `pub fn new(console: C) -> Self`
    - `pub fn console(&self) -> &C`
    - `pub fn transcript(&self) -> &str`
    - `pub fn hostname(&self) -> &str`
    - `pub fn login_factory(&mut self) -> Result<Option<FactorySecrets>, SessionError>`
    - `pub fn login_with(&mut self, login: &NextLogin) -> Result<(), SessionError>`
    - `pub fn exec(&mut self, command: &str) -> Result<String, SessionError>`
    - `pub fn configure(&mut self, lines: &[String]) -> Result<Vec<String>, SessionError>` — returns the lines the device rejected
    - `pub fn save(&mut self) -> Result<(), SessionError>`
    - `pub fn reload(&mut self) -> Result<(), SessionError>`
    - `pub fn copy_image(&mut self, command: &str) -> Result<(), SessionError>`
    - `pub fn install(&mut self, image: &str) -> Result<(), SessionError>`
  - `pub struct ScriptedConsole` in `scripted.rs` with `pub fn new(steps: Vec<(&str, &str)>) -> Self` and `pub fn written(&self) -> &[String]`, implementing `Console`. Each step is (text the session must write, with one trailing `\r` removed; device output that becomes readable after that write). A write that does not match the next step fails with an `io::Error` naming both texts. `read_until` with nothing left to match fails at once with `TimedOut`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn open_console_declines_setup_and_enters_enable() {
    let console = ScriptedConsole::new(vec![
        ("", "Would you like to enter the initial configuration dialog? [yes/no]: "),
        ("no", "\r\nSwitch>"),
        ("enable", "\r\nSwitch#"),
        ("terminal length 0", "\r\nSwitch#"),
    ]);
    let mut session = Session::new(console);
    assert_eq!(session.login_factory().unwrap(), None);
    assert_eq!(session.hostname(), "Switch");
}

#[test]
fn short_password_waits_for_the_prompt_then_sends_the_fallback() {
    let console = ScriptedConsole::new(vec![
        ("", "Enter enable secret: "),
        ("cisco", "\r\n% Password rejected. Too short.\r\nEnter enable secret: "),
        ("Passw0rd%", "\r\nConfirm enable secret: "),
        ("Passw0rd%", "\r\nSwitch>"),
        ("enable", "\r\nPassword: "),
        ("Passw0rd%", "\r\nSwitch#"),
        ("terminal length 0", "\r\nSwitch#"),
    ]);
    let mut session = Session::new(console);
    let secrets = session.login_factory().unwrap().unwrap();
    assert_eq!(secrets.enable_secret, "Passw0rd%");
}

#[test]
fn configure_returns_the_rejected_lines() {
    let console = ScriptedConsole::new(vec![
        ("configure terminal", "\r\nSwitch(config)#"),
        ("interface GigabitEthernet1/0/1", "\r\nSwitch(config-if)#"),
        (" bogus", "\r\n% Invalid input detected at '^' marker.\r\nSwitch(config-if)#"),
        ("end", "\r\nSwitch#"),
    ]);
    let mut session = Session::new(console);
    let lines = vec!["configure terminal".to_string(), "interface GigabitEthernet1/0/1".to_string(), " bogus".to_string(), "end".to_string()];
    assert_eq!(session.configure(&lines).unwrap(), vec![" bogus".to_string()]);
}

#[test]
fn reload_answers_no_to_save_and_confirms() {
    let console = ScriptedConsole::new(vec![
        ("reload", "\r\nSystem configuration has been modified. Save? [yes/no]: "),
        ("no", "\r\nProceed with reload? [confirm]"),
        ("", "\r\nReload requested\r\n"),
    ]);
    let mut session = Session::new(console);
    session.reload().unwrap();
    assert_eq!(session.console().written(), &["reload".to_string(), "no".to_string(), "".to_string()]);
}

#[test]
fn login_with_a_file_user_after_reboot() {
    let console = ScriptedConsole::new(vec![
        ("", "\r\nPress RETURN to get started!\r\n"),
        ("", "\r\nUser Access Verification\r\n\r\nUsername: "),
        ("bench", "Password: "),
        ("BenchPass", "\r\ncloset-2-sw1#"),
        ("terminal length 0", "\r\ncloset-2-sw1#"),
    ]);
    let mut session = Session::new(console);
    let login = NextLogin::User { username: "bench".into(), password: "BenchPass".into(), enable_secret: "cisco".into() };
    session.login_with(&login).unwrap();
    assert_eq!(session.hostname(), "closet-2-sw1");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p bench-core --lib session::`
Expected: FAIL to compile

- [ ] **Step 3: Implement `ScriptedConsole` and `Session`**

Every write is the text followed by `\r`. The transcript is every byte written and read. Prompts: privileged `(?m)^([A-Za-z0-9._-]+)#\s*$`, configuration `(?m)^[A-Za-z0-9._-]+\(config[^)]*\)#\s*$`, user exec `(?m)^[A-Za-z0-9._-]+>\s*$`. The host name is the text before the last `#` prompt seen.

- `login_factory` writes an empty line, then answers until the privileged prompt. `initial configuration dialog? [yes/no]` is `InitialDialog`; `terminate autoinstall? [yes]` is answered `yes`; `Press RETURN to get started` is answered with an empty line; `(?i)(enter\b.*\busername|username)\s*:\s*$` is `UsernamePrompt`; `(?i)\b(confirm|re-enter)\b.*\b(password|secret)\b[^:\n]*:\s*$` is `ConfirmPassword`; `(?i)\benter\b.*\benable secret\b[^:\n]*:\s*$` is `EnableSecretPrompt`; `(?i)password\s*:\s*$` is `PasswordPrompt`; `(?im)^%.*\b(password|secret)\b.*$` is `Rejected`, with `too_short` when that line matches `(?i)too short|minimum|at least \d+`. The `>` prompt is `UserExec` and `#` is `Privileged`. `Send` writes, `Wait` reads again, `Stop` returns `SessionError::Stopped`. At `#` it runs `terminal length 0` and returns the reminder. The first read may wait 45 minutes, because this also runs after a reboot.
- `login_with` writes an empty line, answers `Press RETURN` with an empty line, `Username:` with the username, `Password:` with the password before the first `>` or `#` and with the enable secret after `enable`, and the `>` prompt with `enable`. It runs `terminal length 0` at `#`. `NextLogin::Stop` returns `Stopped` without writing. `NextLogin::Factory(s)` uses `s`. The first read may wait 45 minutes.
- `exec` writes the command, reads to the privileged prompt (two minutes), and returns the output without the echoed command line and without the prompt.
- `configure` skips the file’s own `configure terminal`, `conf t`, and `end` lines, writes `configure terminal`, then each remaining line, reading after each to the configuration or privileged prompt. A line whose output has a line starting with `%` is rejected. It writes `end` and reads to `#`.
- `save` runs `save_startup_command()`, answers `Destination filename [startup-config]?` with an empty line, and reads to `#`. Output containing `%Error` is `Stopped`.
- `reload` writes `reload`, answers `Save? [yes/no]` with `no` and `[confirm]` with an empty line, and returns after confirming. The caller logs in again.
- `copy_image` writes the command, answers `Destination filename` with an empty line, and reads to `#` for up to three hours. Output containing `%Error` or `Error opening` is `Stopped`.
- `install` writes `install add file flash:<image> activate commit`, answers `[y/n]` with `y` and `[yes/no]` with `no`, and returns once the output matches `(?i)reload`. Output matching `(?i)FAILED|%Error|Invalid input` is `Stopped`. It may wait 90 minutes.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p bench-core --lib session::`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add Cargo.lock crates/bench-core
git commit -m "feat: drive the IOS-XE console session"
```

### Task 15: Run the phases on a device

**Files:**
- Create: `crates/bench-core/src/runner.rs`
- Modify: `crates/bench-core/src/lib.rs`

**Interfaces:**
- Consumes: `Session`, `Console`, `SessionError`, `ScriptedConsole` (Task 14); `RunOptions`, `RunRecord`, `CheckStatus` (Task 1); `missing_commands` (Task 2); `parse_uptime_lines`, `burn_in_passes` (Task 3); `physical_interfaces`, `first_physical`, `switch_port_passed`, `router_port_passed` (Task 4); `version_token`, `firmware_check`, `FirmwareCheck` (Task 5); `FactorySecrets`, `NextLogin`, `login_after_apply` (Tasks 6 and 7); `ExportInput`, `chassis_serial` (Task 8); `Phase`, `PhaseEvent`, `start_phase`, `next_phase`, `port_walk_available` (Task 9); `LinkKind`, `assign_address_commands`, `copy_tftp_command` (Task 10)
- Produces:
  - `pub struct RunFiles { pub config_name: String, pub config_body: String, pub image_name: Option<String> }`
  - `pub fn device_kind(show_version: &str) -> LinkKind` — `Switch` when the output contains, case-insensitively, `cisco c9`, `cisco ie-`, or `switch ports model`; otherwise `Router`
  - `pub struct Runner<C: Console>` with:
    - `pub fn new(session: Session<C>, options: RunOptions, files: RunFiles) -> Self`
    - `pub fn phase(&self) -> Phase`
    - `pub fn record(&self) -> &RunRecord`
    - `pub fn reminder(&self) -> Option<&FactorySecrets>`
    - `pub fn tftp_port(&self) -> Option<&str>`
    - `pub fn stop_reason(&self) -> Option<&str>`
    - `pub fn session(&self) -> &Session<C>`
    - `pub fn ports(&self) -> &[(String, CheckStatus)]`
    - `pub fn advance(&mut self) -> Phase` — does the console work for the current phase and returns the new phase. It returns without console work at `NameTftpPort` until `confirm_cable`, at `WaitBurnIn` until `finish_wait`, and at `Done` and `Stopped`.
    - `pub fn confirm_cable(&mut self)`
    - `pub fn finish_wait(&mut self)`
    - `pub fn check_port(&mut self, name: &str) -> Result<bool, SessionError>`
    - `pub fn export_input(&self) -> ExportInput`

- [ ] **Step 1: Write the failing test**

Build each `ScriptedConsole` so the device answers the commands below. Drive each run with `advance()` until `Done` or `Stopped`, at most 40 calls.

```rust
#[test]
fn config_only_run_passes_and_reloads_twice() {
    let mut runner = config_only_runner(ok_running_config());
    let phase = drive(&mut runner);
    assert_eq!(phase, Phase::Done);
    assert_eq!(runner.record().configuration, CheckStatus::Passed);
    assert_eq!(reloads(&runner), 2);
}

#[test]
fn a_missing_command_stops_before_the_clean_reboot() {
    let mut runner = config_only_runner(running_config_without_vlan_line());
    assert_eq!(drive(&mut runner), Phase::Stopped);
    assert!(matches!(&runner.record().configuration, CheckStatus::Failed { detail } if detail.contains("switchport access vlan 20")));
    assert_eq!(reloads(&runner), 1);
}

#[test]
fn a_hashed_enable_secret_stops_before_anything_is_applied() {
    let mut runner = runner_for_body("enable secret 9 $9$abc\n");
    assert_eq!(drive(&mut runner), Phase::Stopped);
    let written = runner.session().console().written();
    assert!(!written.iter().any(|w| w == "configure terminal" || w == "reload"));
}

#[test]
fn an_image_without_a_version_stops_before_the_copy() {
    let mut runner = firmware_runner("image.bin");
    runner.advance();
    runner.confirm_cable();
    assert_eq!(drive(&mut runner), Phase::Stopped);
    assert!(!runner.session().console().written().iter().any(|w| w.starts_with("copy tftp")));
}

#[test]
fn switch_port_check_uses_interface_status() {
    let mut runner = done_switch_runner_with_ports(&["GigabitEthernet1/0/1"]);
    assert!(runner.check_port("GigabitEthernet1/0/1").unwrap());
    assert_eq!(runner.record().ports, CheckStatus::Passed);
}
```

`config_only_runner`, `runner_for_body`, `firmware_runner`, `done_switch_runner_with_ports`, `ok_running_config`, `running_config_without_vlan_line`, `drive`, and `reloads` are helpers in the test module. The configuration body used by the config-only helpers is `hostname closet-2-sw1\ninterface GigabitEthernet1/0/1\n switchport access vlan 20\n`. `reloads` counts writes equal to `reload`.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p bench-core --lib runner::`
Expected: FAIL to compile

- [ ] **Step 3: Implement `Runner` and `device_kind`**

Initial record: configuration `NotFinished`; firmware, burn-in, and ports `NotFinished` when chosen, otherwise `NotChosen`. Work per phase:

- `Login`: `login_factory` and keep the reminder. Run `show version` (device kind), `show inventory` (serial through `chassis_serial`), and `show ip interface brief`. The first column of that table, without its header, is the interface list. With port testing, every `physical_interfaces` entry starts `NotFinished`. With firmware, `tftp_port` is `first_physical`. Event `LoggedIn`.
- `NameTftpPort`: `CableConfirmed` once `confirm_cable` was called.
- `CopyImage`: if `version_token` of the image is `None`, stop with `The image file name has no version number.` before writing anything. Otherwise `configure(assign_address_commands(kind, port))`; any rejected line stops. Then `copy_image(copy_tftp_command(image))`. Event `ImageOnFlash`.
- `InstallAndReboot`: `install(image)`. Event `Rebooted`.
- `ConfirmVersion`: `login_factory` again, because the device boots without a saved configuration, and keep the new reminder. `show version` through `firmware_check`: `Pass` marks firmware `Passed` and sends `VersionMatched`; anything else marks it `Failed` and sends `VersionRejected`.
- `ApplyConfig`: compute `login_after_apply(body, factory)` first, where `factory` is the reminder or, for an open console, empty strings. `Stop` ends the run with its lines before anything is written. Then `configure` the body lines; any rejected line marks configuration `Failed` with those lines and stops. Event `ConfigApplied`.
- `SaveAndReboot`: `save`, `Saved`, `reload`, `Rebooted`.
- `ValidateConfig`: `login_with` the computed login, `LoggedIn`, then `show running-config` through `missing_commands`. Empty marks configuration `Passed` and sends `ComparisonPassed`. Otherwise configuration is `Failed` with the missing lines joined by newlines, and the event is `ComparisonFailed`.
- `CleanReboot`: `reload`, then `login_with` the same login. Event `Rebooted`.
- `WaitBurnIn`: `WaitFinished` once `finish_wait` was called.
- `ReadUptime`: `show version | include uptime`. Keep the first line for the export. `burn_in_passes` marks burn-in `Passed` and sends `UptimePassed`, otherwise `Failed` with the output and `UptimeFailed`.

A `SessionError` in any phase moves to `Stopped`, stores the reason, and marks the check that phase belongs to as `Failed`. `check_port` returns `Stopped` unless port testing was chosen and `port_walk_available(phase)`. On a switch it runs `show interface status` and uses `switch_port_passed`; on a router it runs `show interfaces <name>` and uses `router_port_passed`. A retry replaces that port’s result. `record.ports` is `Passed` when every port passed, `NotFinished` while any port has not passed and none failed, and otherwise `Failed`. `export_input` fills from the session host name, the serial, the file names, the version token as running version once firmware passed, the entered hours, the kept uptime line, and the ports.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p bench-core --lib runner::`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/bench-core
git commit -m "feat: run the bench phases on a device"
```

### Task 16: Connect the window to the cables

**Files:**
- Create: `crates/bench-app/src/serial.rs`
- Create: `crates/bench-app/src/tftp.rs`
- Create: `crates/bench-app/src/nic.rs`
- Create: `crates/bench-app/src/awake.rs`
- Create: `crates/bench-app/src/worker.rs`
- Create: `crates/bench-app/Dioxus.toml`
- Modify: `crates/bench-app/src/main.rs`
- Modify: `crates/bench-app/Cargo.toml`

**Interfaces:**
- Consumes: `Console` (Task 14); `Runner`, `RunFiles` (Task 15); `cisco_console` and the TFTP packet functions (Task 13); `server_address`, `mask` (Task 10); `export_text` (Task 8); `first_physical` and `Phase` (Tasks 4 and 9); `ViewState`, `UiCommand` (Task 12)
- Produces:
  - `serial.rs`: `pub struct SerialConsole` implementing `Console` over `serialport` at `cisco_console()`; `pub fn serial_devices() -> Vec<String>`; `pub fn prefer_callout(names: Vec<String>) -> Vec<String>` — on macOS keeps `/dev/cu.*` and drops a `/dev/tty.*` with the same suffix
  - `tftp.rs`: `pub fn serve(path: &std::path::Path, chosen: &str, bind: std::net::SocketAddr, stop: std::sync::Arc<std::sync::atomic::AtomicBool>) -> std::io::Result<u64>` — serves one read of `chosen` and returns the bytes sent
  - `nic.rs`: `pub fn ethernet_ports() -> Vec<String>`, `pub fn add_server_address(port: &str) -> std::io::Result<()>`, `pub fn remove_server_address(port: &str) -> std::io::Result<()>`
  - `awake.rs`: `pub struct KeepAwake` — keeps the computer from sleeping until dropped
  - `worker.rs`: `pub fn spawn(commands: std::sync::mpsc::Receiver<UiCommand>, updates: impl Fn(ViewState) + Send + 'static)`

- [ ] **Step 1: Write the failing test**

In `crates/bench-app/src/serial.rs` and `crates/bench-app/src/tftp.rs`:

```rust
#[test]
fn callout_devices_replace_tty_twins() {
    let names = vec!["/dev/tty.usbserial-1420".to_string(), "/dev/cu.usbserial-1420".to_string(), "COM3".to_string()];
    let kept = prefer_callout(names);
    assert!(kept.contains(&"/dev/cu.usbserial-1420".to_string()));
    assert!(!kept.contains(&"/dev/tty.usbserial-1420".to_string()));
    assert!(kept.contains(&"COM3".to_string()));
}

#[test]
fn serve_sends_the_whole_file_over_loopback() {
    let dir = std::env::temp_dir().join("bench-tftp-test");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("cat9k_iosxe.17.12.05.SPA.bin");
    let bytes: Vec<u8> = (0..1500u32).map(|i| (i % 251) as u8).collect();
    std::fs::write(&path, &bytes).unwrap();
    let received = fetch_over_loopback(&path, "cat9k_iosxe.17.12.05.SPA.bin", 512);
    assert_eq!(received, bytes);
}
```

`fetch_over_loopback` is a test helper that binds `serve` on `127.0.0.1:0`-style free port, sends an RRQ with the given `blksize`, ACKs each DATA, and returns the joined payloads.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p bench-app`
Expected: FAIL to compile

- [ ] **Step 3: Implement the adapters, the worker, and the wiring**

- `SerialConsole::read_until` polls the port with a short read timeout, appends to a buffer, and matches the patterns there under the Task 14 contract.
- `serve` answers an RRQ for any other name with `error_packet(1, "File not found")`. It sends an OACK when `blksize` was requested, then DATA blocks read from the file, using `wire_block` and `block_count`. It retransmits after one second without the expected ACK and gives up after five tries.
- `nic.rs` lists wired interfaces with the `netdev` crate. `add_server_address` assigns `192.0.2.1` with mask `255.255.255.252` to the chosen port with administrator approval: on macOS `osascript` running `ifconfig <port> inet 192.0.2.1 netmask 255.255.255.252 alias` with administrator privileges, on Windows an elevated `netsh interface ipv4 add address "<port>" 192.0.2.1 255.255.255.252`. `remove_server_address` reverses it.
- `KeepAwake` runs `caffeinate -dims -w <pid>` on macOS and calls `SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED)` on Windows through `windows-sys`.
- The worker thread owns the `Runner` and a `KeepAwake` for the run. It reads the configuration file into `RunFiles`. It calls `advance` until the phase blocks, publishing a `ViewState` after each call. Before `CopyImage` it adds the server address and starts `serve` on `192.0.2.1:69` in its own thread; after `ImageOnFlash`, or a stop, it stops the server and removes the address. When the phase becomes `WaitBurnIn`, the deadline is now plus the entered hours. Until then it handles `CheckPort` and publishes the remaining time once a minute, then calls `finish_wait`. `CheckPort` runs whenever `port_walk_available` allows it; `current_port` is the first port that has not passed. `Export` puts `export_text(runner.export_input())` into the next `ViewState`.
- `main.rs` fills `serial_devices` and `ethernet_ports`, opens the window from Task 12, and passes its commands to `worker::spawn`.
- `Dioxus.toml` names the app `IOS-XE Bench` with bundle identifier `com.ciscoconfig.bench` so `dx bundle` can produce a macOS and a Windows package.

- [ ] **Step 4: Run tests and build**

Run: `cargo test -p bench-app && cargo build -p bench-app`
Expected: PASS, and `bench-app` builds on macOS. Then run `cargo check -p bench-app --target x86_64-pc-windows-msvc` if that target is installed and record the result in the report.

- [ ] **Step 5: Commit**

```bash
git add Cargo.lock crates/bench-app
git commit -m "feat: connect the bench window to serial and TFTP"
```
