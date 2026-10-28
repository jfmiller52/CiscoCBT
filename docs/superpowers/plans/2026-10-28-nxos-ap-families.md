# NX-OS and Access-Point Families Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Extend Cisco CBT App so the start form chooses IOS-XE, NX-OS, or Access point; NX-OS runs the full checklist with NX-OS CLI; access points run optional firmware and/or CAPWAP addressing with `show capwap client config` validation.

**Architecture:** Add a `DeviceFamily` on run options and a family strategy in `bench-core` that supplies CLI strings and pass helpers. Keep one `Runner` / phase machine; AP skips config/burn-in/ports and uses CAPWAP phases. `bench-app` shows family-specific start fields and record rows.

**Tech Stack:** Existing Rust 2021 workspace (`bench-core`, Dioxus 0.6 `bench-app`). No new crates.

**Spec:** `docs/superpowers/specs/2026-10-28-nxos-ap-families-design.md` (builds on `docs/superpowers/specs/2026-10-28-ios-xe-bench-design.md`)

## Global Constraints

- Commit on `main`. Do not create a feature branch.
- Factory login stays username `cisco` / password `cisco` / enable `cisco`, then `Passw0rd%` if too short, on all three families.
- Family is chosen on the start form before Start; it does not change mid-run.
- NX-OS: configuration file required; optional firmware, burn-in, port testing — same release rules as IOS-XE.
- Access point: no config apply, no burn-in, no port walk. Firmware and/or CAPWAP only; Start requires at least one.
- CAPWAP apply: `capwap ap ip <ip> <mask> <gateway>`. Validate with `show capwap client config`; IP, mask, and gateway must match the form (trimmed IPv4 strings).
- When both AP firmware and CAPWAP are chosen: firmware first, then CAPWAP.
- AP firmware uses the same TFTP path (chosen laptop ethernet, cable into first physical port).
- Temporary TFTP addressing must not become part of CAPWAP or configuration validation.

## Review Focus

- AP Start with neither firmware nor CAPWAP must be rejected. Pinned in Task 3.
- CAPWAP pass requires all three of IP, mask, and gateway in `show capwap client config`. Pinned in Task 2.
- AP `ready_to_release` with configuration `NotChosen` and only CAPWAP passed. Pinned in Task 1.
- Switching the start form to Access point must not leave a required NX-OS config path enforceable. Pinned in Task 3.
- NX-OS port pass uses NX-OS status output, not IOS-XE-only parsing assumptions. Pinned in Task 5.

## File structure

- Create: `crates/bench-core/src/family.rs` — `DeviceFamily` enum
- Create: `crates/bench-core/src/capwap.rs` — apply command + validate `show capwap client config`
- Create: `crates/bench-core/src/strategy.rs` — `FamilyStrategy` trait + `strategy_for(family)`
- Create: `crates/bench-core/src/strategy_iosxe.rs` — current IOS-XE commands (moved out of runner)
- Create: `crates/bench-core/src/strategy_nxos.rs` — NX-OS commands
- Create: `crates/bench-core/src/strategy_ap.rs` — AP TFTP/install/version helpers
- Modify: `crates/bench-core/src/run.rs` — family, `CapwapChoice`, `RunRecord.capwap`, `ready_to_release`
- Modify: `crates/bench-core/src/phase.rs` — AP path: `ApplyCapwap`, `ValidateCapwap`
- Modify: `crates/bench-core/src/form.rs` — family + CAPWAP fields + `to_options`
- Modify: `crates/bench-core/src/export.rs` — CAPWAP section; omit Configuration when not chosen
- Modify: `crates/bench-core/src/runner.rs` — use strategy; CAPWAP phases; AP record init
- Modify: `crates/bench-core/src/lib.rs` — re-exports
- Modify: `crates/bench-app/src/view.rs`, `main.rs`, `worker.rs` — form UI, record, export gate
- Modify: `README.md` — three families

---

### Task 1: Family on the release record

**Files:**
- Create: `crates/bench-core/src/family.rs`
- Modify: `crates/bench-core/src/run.rs`
- Modify: `crates/bench-core/src/lib.rs`

**Interfaces:**
- Consumes: existing `CheckStatus`
- Produces:
  - `pub enum DeviceFamily { IosXe, Nxos, AccessPoint }` — `Copy`, `Eq`, `Debug`, `Default` → `IosXe`
  - `pub struct CapwapChoice { pub ip: String, pub mask: String, pub gateway: String }`
  - `RunOptions` gains `pub family: DeviceFamily`, `pub capwap: Option<CapwapChoice>`; `config_path: String` may be empty for AP
  - `RunRecord` gains `pub capwap: CheckStatus`
  - `ready_to_release`: configuration must be `Passed` **or** `NotChosen`; firmware, burn_in, ports, and **capwap** each `Passed` or `NotChosen`

- [ ] **Step 1: Write the failing tests**

```rust
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
```

- [ ] **Step 2: Run tests — expect FAIL** (missing `capwap` / new `ready_to_release` rules)

Run: `cargo test -p bench-core ready_to_release -- --nocapture`

- [ ] **Step 3: Implement `DeviceFamily`, `CapwapChoice`, extend `RunOptions` / `RunRecord`, update `ready_to_release`**

- [ ] **Step 4: Run tests — expect PASS**

- [ ] **Step 5: Commit**

```bash
git add crates/bench-core/src/family.rs crates/bench-core/src/run.rs crates/bench-core/src/lib.rs
git commit -m "feat: device family and CAPWAP on the release record"
```

---

### Task 2: CAPWAP apply and validation

**Files:**
- Create: `crates/bench-core/src/capwap.rs`
- Modify: `crates/bench-core/src/lib.rs`

**Interfaces:**
- Consumes: `CapwapChoice`
- Produces:
  - `pub fn capwap_apply_command(choice: &CapwapChoice) -> String` → `capwap ap ip {ip} {mask} {gateway}` with trimmed fields
  - `pub fn capwap_passed(show_capwap_client_config: &str, choice: &CapwapChoice) -> bool` — true only when the output contains each of the three trimmed values as whole tokens (split on whitespace); all three required
  - `pub fn capwap_failure_detail(show: &str, choice: &CapwapChoice) -> String` — names which of IP/mask/gateway were missing

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn apply_command_joins_three_fields() {
    let c = CapwapChoice { ip: "10.1.1.10".into(), mask: "255.255.255.0".into(), gateway: "10.1.1.1".into() };
    assert_eq!(capwap_apply_command(&c), "capwap ap ip 10.1.1.10 255.255.255.0 10.1.1.1");
}

#[test]
fn validation_requires_ip_mask_and_gateway() {
    let c = CapwapChoice { ip: "10.1.1.10".into(), mask: "255.255.255.0".into(), gateway: "10.1.1.1".into() };
    let ok = "IP Address: 10.1.1.10\nNetmask: 255.255.255.0\nDefault Gateway: 10.1.1.1\n";
    assert!(capwap_passed(ok, &c));
    assert!(!capwap_passed("IP Address: 10.1.1.10\nNetmask: 255.255.255.0\n", &c));
    assert!(!capwap_passed("IP Address: 10.9.9.9\nNetmask: 255.255.255.0\nDefault Gateway: 10.1.1.1\n", &c));
}
```

- [ ] **Step 2: Run tests — expect FAIL**

Run: `cargo test -p bench-core capwap -- --nocapture`

- [ ] **Step 3: Implement `capwap.rs` and re-export**

- [ ] **Step 4: Run tests — expect PASS**

- [ ] **Step 5: Commit**

```bash
git add crates/bench-core/src/capwap.rs crates/bench-core/src/lib.rs
git commit -m "feat: CAPWAP apply command and client-config validation"
```

---

### Task 3: Start form family and CAPWAP fields

**Files:**
- Modify: `crates/bench-core/src/form.rs`

**Interfaces:**
- Consumes: `DeviceFamily`, `CapwapChoice`, `FirmwareChoice`, `validate_burn_in_hours`
- Produces: `StartForm` gains `family`, `capwap_on`, `capwap_ip`, `capwap_mask`, `capwap_gateway`; `to_options` rules:
  - **IosXe / Nxos:** unchanged required serial + config; burn-in/ports/firmware as today; `capwap: None`; set `family`
  - **AccessPoint:** require serial; reject if `!firmware_on && !capwap_on` with `"Choose firmware upgrade and/or CAPWAP addressing."`; if firmware on, same image/ethernet errors; if CAPWAP on, each of IP/mask/gateway non-empty or `"Enter the CAPWAP IP, subnet mask, and default gateway."`; `config_path` may be empty; `burn_in_hours: None`; `port_testing: false`; `configuration` path not required

- [ ] **Step 1: Write the failing tests** — AP neither option errs; AP CAPWAP-only yields `capwap: Some` and empty config ok; AP firmware-only ok; IosXe still requires config; setting family to AP in a helper that clears switch-only fields is form logic (`clear_for_family` optional — if added, test it)

- [ ] **Step 2: Run — expect FAIL**

Run: `cargo test -p bench-core form -- --nocapture`

- [ ] **Step 3: Implement form fields and `to_options`**

Add `pub fn on_family_changed(&mut self)` that when switching to `AccessPoint` sets `burn_in_on = false`, `port_testing = false`, clears `config_path` optional; when switching away from AP sets `capwap_on = false` and clears CAPWAP strings. Call from UI later.

- [ ] **Step 4: Run — expect PASS**

- [ ] **Step 5: Commit**

```bash
git add crates/bench-core/src/form.rs
git commit -m "feat: start form device family and CAPWAP fields"
```

---

### Task 4: Phase machine for CAPWAP

**Files:**
- Modify: `crates/bench-core/src/phase.rs`

**Interfaces:**
- Consumes: `RunOptions` (with `family`, `firmware`, `capwap`)
- Produces:
  - `Phase::ApplyCapwap`, `Phase::ValidateCapwap`
  - `PhaseEvent::CapwapApplied`, `PhaseEvent::CapwapValidated`, `PhaseEvent::CapwapFailed`
  - After `Login` + `LoggedIn`: if firmware → `NameTftpPort`; else if AP and CAPWAP → `ApplyCapwap`; else if AP and neither (should not happen) → `Stopped`; else → `ApplyConfig` (switch families)
  - After firmware `VersionMatched`: if AP and CAPWAP → `ApplyCapwap`; else if AP → `Done`; else → `ApplyConfig`
  - `ApplyCapwap` + `CapwapApplied` → `ValidateCapwap`
  - `ValidateCapwap` + `CapwapValidated` → `Done`; `CapwapFailed` → `Stopped`
  - `port_walk_available`: false when `options.family == AccessPoint` (or when ports not chosen — keep existing phase list; runner already gates on `port_testing`)

- [ ] **Step 1: Write failing tests** — AP firmware then CAPWAP order; AP CAPWAP-only skips TFTP and config; NX-OS/IOS-XE path unchanged for config-only

- [ ] **Step 2: Run — expect FAIL**

Run: `cargo test -p bench-core phase -- --nocapture`

- [ ] **Step 3: Implement new phases and `next_phase` branches**

- [ ] **Step 4: Run — expect PASS**

- [ ] **Step 5: Commit**

```bash
git add crates/bench-core/src/phase.rs
git commit -m "feat: CAPWAP phases for access-point runs"
```

---

### Task 5: Family strategy trait + IOS-XE and NX-OS

**Files:**
- Create: `crates/bench-core/src/strategy.rs`
- Create: `crates/bench-core/src/strategy_iosxe.rs`
- Create: `crates/bench-core/src/strategy_nxos.rs`
- Modify: `crates/bench-core/src/lib.rs`

**Interfaces:**
- Consumes: `DeviceFamily`, `LinkKind`, existing port/uptime helpers as needed
- Produces: `pub trait FamilyStrategy` with methods the runner needs (exact names):

```rust
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
pub fn strategy_for(family: DeviceFamily) -> &'static dyn FamilyStrategy;
```

- IOS-XE strategy wraps today’s `transfer` / `ports` / session command strings (`install add … activate … commit` or whatever runner already sends — move those string literals here without behavior change).
- NX-OS strategy (pin in tests):
  - copy: `copy tftp://192.0.2.1/{file} bootflash:{file} vrf management` is wrong for our TFTP link — use `copy tftp://192.0.2.1/{file} bootflash:{file}` (same server address helpers as today)
  - assign address: `configure terminal`, `interface {iface}`, `no switchport` omitted on NX-OS Ethernet (L3 capable) — use `ip address 192.0.2.2 255.255.255.252`, `no shutdown`, `end` (no `ip tftp source-interface` if unsupported; include if NX-OS accepts — prefer matching IOS-XE shape where valid)
  - install: `install all nxos bootflash:{file} non-interactive` then rely on reload in that flow; if single-command reload needed, second command `reload` only when install does not reboot — **document chosen pair in the NX-OS module header**
  - interfaces: `show interface brief`; parse Ethernet/mgmt names; skip loopback/vlan/port-channel
  - port: `show interface {port}`; pass when line contains `link-state: up` or `(connected)` / admin up oper up — pick one rule and pin with sample NX-OS output in tests
  - uptime: `show version | include uptime` or `show system uptime` — parse with existing `parse_uptime_lines` if format matches; else small NX-OS parser used only by strategy
  - running-config: `show running-config`
  - save: `copy running-config startup-config`

- [ ] **Step 1: Write failing tests** for `strategy_for(IosXe)` copy command equals today’s `copy_tftp_command`; `strategy_for(Nxos)` port_passed true on sample NX-OS up output and false when down

- [ ] **Step 2: Run — expect FAIL**

Run: `cargo test -p bench-core strategy -- --nocapture`

- [ ] **Step 3: Implement trait + iosxe + nxos modules**

- [ ] **Step 4: Run — expect PASS**

- [ ] **Step 5: Commit**

```bash
git add crates/bench-core/src/strategy*.rs crates/bench-core/src/lib.rs
git commit -m "feat: family strategy for IOS-XE and NX-OS commands"
```

---

### Task 6: Access-point strategy

**Files:**
- Create: `crates/bench-core/src/strategy_ap.rs`
- Modify: `crates/bench-core/src/strategy.rs` (`strategy_for(AccessPoint)`)

**Interfaces:**
- Produces: AP `FamilyStrategy` — TFTP assign/clear/copy suitable for AP ethernet; install/reload commands for AP image; `show_version_command`; `list_interfaces_command` / `parse_interface_names` for first physical (usually `GigabitEthernet0` or similar — pin with sample); `port_passed` / `port_status_command` unused by runner on AP but may return false / empty; `show_running_config_command` unused on AP path

- [ ] **Step 1: Failing test** — `strategy_for(AccessPoint).copy_tftp_command("ap.img")` is a non-empty TFTP copy string containing `192.0.2.1` and `ap.img`

- [ ] **Step 2: Run — expect FAIL**

- [ ] **Step 3: Implement AP strategy** (use current CAPWAP AP CLI: `archive download-sw` / `ap-type` as needed — choose the install sequence that activates the flash image and reloads; document in module header; rejection handling stays in runner)

- [ ] **Step 4: Run — expect PASS**

- [ ] **Step 5: Commit**

```bash
git add crates/bench-core/src/strategy_ap.rs crates/bench-core/src/strategy.rs
git commit -m "feat: access-point family strategy for TFTP install"
```

---

### Task 7: Export CAPWAP and AP without configuration section

**Files:**
- Modify: `crates/bench-core/src/export.rs`

**Interfaces:**
- Consumes: `CheckStatus`, `CapwapChoice` fields as strings on `ExportInput`
- Produces: `ExportInput` gains `pub capwap: CheckStatus`, `pub capwap_ip: Option<String>`, `pub capwap_mask: Option<String>`, `pub capwap_gateway: Option<String>`; when `configuration == NotChosen`, omit the Configuration section; when CAPWAP chosen (`capwap != NotChosen` or IP is `Some`), emit CAPWAP section with IP/mask/gateway lines and status via `push_status`

- [ ] **Step 1: Failing tests** — AP export omits Configuration; includes CAPWAP Passed with three addresses; failed CAPWAP indents detail

- [ ] **Step 2: Run — expect FAIL**

Run: `cargo test -p bench-core export -- --nocapture`

- [ ] **Step 3: Implement**

- [ ] **Step 4: Run — expect PASS**

- [ ] **Step 5: Commit**

```bash
git add crates/bench-core/src/export.rs
git commit -m "feat: export CAPWAP and skip config section when not chosen"
```

---

### Task 8: Runner uses strategy and CAPWAP phases

**Files:**
- Modify: `crates/bench-core/src/runner.rs`

**Interfaces:**
- Consumes: `strategy_for`, `capwap_apply_command`, `capwap_passed`, `capwap_failure_detail`, updated phases
- Produces: `Runner::new` initializes `configuration`/`burn_in`/`ports` as `NotChosen` for `AccessPoint`; `capwap` from options; holds `family` via options; all former hard-coded IOS-XE transfer/port/version/show commands go through `strategy_for(self.options.family)`; `advance` handles `ApplyCapwap` / `ValidateCapwap`; `export_input` fills CAPWAP fields; `export` / ready gates: AP may export when every chosen check passed (configuration NotChosen does not block) — keep offering export when configuration passed **or** (family AP and chosen checks complete enough per existing “config passed first” rule): for AP, offer once any chosen check has reached Passed and none Failed unfinished… Spec: “Export is offered once every chosen AP check has passed” for final ready; interim: mirror switch behavior — for AP allow export when `ready_to_release` OR when at least one chosen check passed and none failed (simplest: **AP export enabled when `ready_to_release(&record)`** only, or when capwap/firmware not NotFinished if chosen). Prefer: enable Export button when `ready_to_release` for AP; for switch keep “configuration passed” early export. Implement `pub fn export_available(family: DeviceFamily, record: &RunRecord) -> bool`.

- [ ] **Step 1: Failing scripted tests** — AP CAPWAP-only run reaches Done with `capwap: Passed`; AP wrong CAPWAP → Stopped Failed; NX-OS config-only still validates via strategy show running-config (scripted); existing IOS-XE tests still pass

- [ ] **Step 2: Run — expect FAIL**

Run: `cargo test -p bench-core runner -- --nocapture`

- [ ] **Step 3: Wire strategy + CAPWAP into `Runner`**

- [ ] **Step 4: Run full `cargo test -p bench-core` — expect PASS**

- [ ] **Step 5: Commit**

```bash
git add crates/bench-core/src/runner.rs crates/bench-core/src/export.rs crates/bench-core/src/run.rs
git commit -m "feat: runner drives NX-OS and AP through family strategy"
```

---

### Task 9: Desktop start form and record UI

**Files:**
- Modify: `crates/bench-app/src/main.rs`
- Modify: `crates/bench-app/src/view.rs`
- Modify: `crates/bench-app/src/worker.rs` (construct `RunFiles` / options; AP may have empty config body)

**Interfaces:**
- Consumes: `DeviceFamily`, form CAPWAP fields, `export_available`, `RunRecord.capwap`
- Produces: Start form radio/select for IOS-XE / NX-OS / Access point calling `on_family_changed`; CAPWAP fields when AP + `capwap_on`; hide config/burn-in/ports for AP; hide CAPWAP for non-AP; Record shows CAPWAP row when not `NotChosen`; Export uses `export_available`

- [ ] **Step 1: Update UI + any view unit tests for screen selection (AP never PortWalk)**

- [ ] **Step 2: `cargo test -p bench-app` and `cargo build -p bench-app` — expect PASS**

- [ ] **Step 3: Commit**

```bash
git add crates/bench-app/src/main.rs crates/bench-app/src/view.rs crates/bench-app/src/worker.rs
git commit -m "feat: start form and record UI for NX-OS and access points"
```

---

### Task 10: README

**Files:**
- Modify: `README.md`

**Interfaces:** none

- [ ] **Step 1: Update overview, features, start-form, run sections for three families and AP CAPWAP; remove “IOS-XE only” non-goals that no longer apply; keep IOS-XR / Meraki out**

- [ ] **Step 2: Commit**

```bash
git add README.md
git commit -m "docs: describe NX-OS and access-point bench flows"
```

---

## Self-review notes

- Spec coverage: family selector, NX-OS full checklist, AP firmware±CAPWAP, CAPWAP command + `show capwap client config`, factory login unchanged, strategy approach, export, UI — Tasks 1–10.
- NX-OS/AP exact install CLI documented as strategy module headers + pinned tests (Calls left to builder in the spec).
- `ready_to_release` and form gates cover Review Focus rows.
