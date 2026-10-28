# NX-OS and access-point families

## Intent

Extend Cisco CBT App so a technician can run the same bench product against **IOS-XE**, **NX-OS**, or an **access point**, chosen on the start form. IOS-XE behavior stays as in the v1.0 design. NX-OS keeps that full release checklist with NX-OS commands. Access points get a thinner run: optional firmware (same TFTP path) and/or CAPWAP addressing from a prompt, with validation via `show capwap client config`.

Who it’s for: the same bench technician who already uses the app on factory or factory-reset gear.

Why: v1.0 deliberately deferred NX-OS and APs; those devices are now on the same bench and need a proof file without a second tool.

## Goals

- Start-form **device family**: IOS-XE, NX-OS, or Access point. The form shows only the options that family supports.
- **NX-OS**: same full checklist as IOS-XE — configuration file required; optional firmware, burn-in, port testing; same export and ready-to-release rules.
- **Access point**: no configuration file apply, no burn-in, no port walk. Optional **firmware upgrade** (same TFTP path as today) and/or **CAPWAP addressing** (IP, subnet mask, default gateway from a prompt). Firmware-only, CAPWAP-only, or both are allowed.
- CAPWAP apply command: `capwap ap ip <static-ip-addr> <static-netmask> <ip-addr-default-gateway>`.
- CAPWAP validation: `show capwap client config` must show the IP, mask, and gateway the technician entered.
- **Factory login** for all three families: same rule as today (open console or `cisco`/`cisco`, `Passw0rd%` if too short; reminder of what was accepted).
- One shared session core with a **family strategy** (not separate apps). Desktop shell stays Dioxus; only the start form and record checks change by family.

## Non-goals

- IOS-XR, small-business CLI, Meraki, or controller-side CAPWAP provisioning.
- Changing the IOS-XE command set or pass rules except where shared helpers need to be parameterized by family.
- Talking to a wireless controller; the AP is configured only over its console.
- Auto-detecting family from the console (technician chooses on the form).
- Byte-for-byte config match on NX-OS (same presence rules as IOS-XE, with NX-OS rewrite allowances the builder documents).

## Anti-goals

- A release record that passes when a chosen check failed, was skipped, or never ran.
- Showing IOS-XE-only options (config / burn-in / ports) on an AP run, or AP-only CAPWAP fields on IOS-XE / NX-OS.
- Claiming CAPWAP passed without `show capwap client config` confirming the entered IP, mask, and gateway.
- Rebooting into a hashed console password the program cannot type (same rule as v1.0, including after NX-OS or AP config/firmware where those apply).

## Constraints

- Existing crate split (`bench-core` + `bench-app`), macOS and Windows, one console at 9600 8N1, TFTP from a chosen laptop ethernet port for firmware.
- Devices are factory fresh or factory reset.
- Family is chosen before Start and does not change mid-run.
- For an AP run, Start is allowed when at least one of firmware or CAPWAP addressing is on. Both may be on. Neither is not allowed.
- AP export is offered once every chosen AP check has passed (firmware and/or CAPWAP). There is no configuration check on APs.
- NX-OS and IOS-XE export remain gated on configuration pass first, as today; unfinished optional checks can still appear as not finished.

## Approaches

Three ways to structure multi-family support were considered:

1. **Family strategy in `bench-core` (chosen).** A device-family enum on the start form; family-specific CLI and checks behind a strategy (trait or module set). One runner / phase machine asks the strategy for commands and pass rules. IOS-XE stays the default implementation; NX-OS supplies alternate commands for the same checks; AP supplies a shorter strategy (firmware + CAPWAP only).
2. **Match on family inside the existing runner.** Faster to ship; becomes hard to read as NX-OS and AP rules grow.
3. **Separate runners per family.** Strong isolation; duplicates export, waits, and UI wiring.

The technician chose approach 1.

## Decisions

### Start form

- **Device family** control: IOS-XE | NX-OS | Access point.
- **IOS-XE and NX-OS:** serial device; configuration file (required); optional firmware (image + ethernet port); optional burn-in (whole hours ≥ 1); optional port testing. Same layout as today plus the family control.
- **Access point:** serial device; optional firmware (image + ethernet port); optional CAPWAP block with IP, subnet mask, and default gateway. No config file, burn-in, or port testing controls. Start requires firmware and/or CAPWAP enabled (and fields filled when CAPWAP is on; image + ethernet when firmware is on).

### NX-OS run

Same phase order and pass meanings as the IOS-XE design (`docs/superpowers/specs/2026-10-28-ios-xe-bench-design.md`): factory login → optional firmware (cable prompt, temporary address, TFTP copy, install, reboot, version token) → apply config → save → reboot → presence check → clean-slate reboot → optional burn-in wait/uptime → optional port walk.

Family strategy supplies NX-OS spellings where they differ (for example interface listing, copy/install, `show running-config`, uptime, port connectedness, chassis serial). Presence comparison keeps the same spirit: every meaningful line from the file must be present; extras on the device are allowed; documented NX-OS rewrites count.

Port walk: still one physical port at a time with a secondary switch. Pass when the strategy’s port check says the named port is up/connected. Virtual interfaces excluded as today.

### Access-point run

1. Factory login (same rules).
2. If firmware chosen: same TFTP path as IOS-XE (cable into first physical port, temporary address as the AP strategy defines, copy, install, reboot, version token from filename vs `show version` or AP-equivalent).
3. If CAPWAP chosen: send `capwap ap ip <ip> <mask> <gateway>` (values from the form). Then run `show capwap client config`. Pass only when that output shows the entered IP, mask, and gateway. Fail names what was missing.
4. No config apply, no burn-in, no port walk.
5. Ready to release when every chosen check (firmware and/or CAPWAP) has passed. Export includes chassis serial when available, host name when available, and each chosen check.

Order when both firmware and CAPWAP are chosen: firmware first (including its reboot and version check), then CAPWAP apply and validation. Builder may adjust if the AP must be in a particular mode after image install; document the rule in code.

### CAPWAP validation

Parse `show capwap client config` for the static IP, netmask, and default gateway. All three must match the form values (normal IPv4 string comparison after trimming). Wrong or missing values fail CAPWAP.

### Shared

- Password reminder unchanged.
- Console transcript and status bar unchanged.
- Firmware version token still taken from the image filename; stop before install if none.
- Temporary TFTP addressing must not remain as a requirement on any family’s validation that is not about that temporary link.

## Screen

- Start form: family selector first (or prominently above the options). Switching family clears or hides inapplicable fields so the technician cannot submit an AP run with a switch config file, or an NX-OS run with CAPWAP fields.
- Record for IOS-XE / NX-OS: configuration, firmware, burn-in, ports as today.
- Record for AP: firmware (if chosen) and CAPWAP (if chosen) only. CAPWAP row shows the entered IP (and optionally mask/gateway in the subline).
- No port-walk or burn-in UI on AP. Cable-is-in prompt still appears when AP firmware is on.

## Export

### IOS-XE / NX-OS

Unchanged sections from the v1.0 design, labeled by family only if useful for the technician.

### Access point

- Chassis serial when readable; host name when readable.
- Firmware image name and running version when firmware was chosen.
- CAPWAP: entered IP, mask, gateway, and Passed / Failed / Not finished when CAPWAP was chosen. On failure, include enough of `show capwap client config` (or the missing fields) to see why.

## Calls (builder)

These were not fixed in conversation; settle them in code and keep them consistent with the goals above.

- Exact NX-OS commands for: listing physical interfaces, temporary TFTP address (including whether `no switchport` applies), copy from TFTP, install/activate + reload, reading running-config, uptime, port connectedness, chassis serial. Prefer current NX-OS CLI; stop and show device output on rejection.
- Exact AP commands for: listing the first physical port, temporary address for TFTP, copy, install/activate + reload, and reading version for the firmware token check. Prefer current CAPWAP AP CLI used on the bench.
- How strictly CAPWAP output matching works (field labels in `show capwap client config`); match the three values the technician entered.
- Shape of the family strategy API in `bench-core` (trait vs modules), as long as the runner does not hard-code NX-OS/AP strings outside that boundary.
- Whether NX-OS presence comparison needs extra rewrite rules beyond IOS-XE’s; document them next to the comparator.
- UI copy for the family selector and CAPWAP field labels.

## Left to the builder

- Implementation plan tasks, tests per family strategy, and wiring the Dioxus form to `DeviceFamily`.
- Packaging/version bump only if the human partner asks for a release after this lands.
- README updates to describe the three families and AP CAPWAP flow.

## Out of scope / later

- Auto-detect family from `show version`.
- Controller join verification beyond CAPWAP client config IP/mask/gateway.
- IOS-XR and other families.
