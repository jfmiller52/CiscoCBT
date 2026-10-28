# IOS-XE bench configurator

## Intent

A technician at a bench configures a Cisco device over its console port and leaves with proof of what happened. The device is fresh out of the box or has been reset to factory defaults. The first version speaks IOS-XE on switches and routers. The program is a desktop window on the technician’s computer, drawn with web layout, around a Rust core that owns the console session.

The window should feel like Cursor: dark, dense, a narrow activity bar, the device and the run in a sidebar, the record in the main area, a console transcript, and a status bar that shows the serial connection. That look is chrome and layout. The session engine does not depend on it.

## Goals

- Pick a configuration file on the technician’s computer, apply it over the console, and pass only when every command in that file is present in the running config. Extra lines on the device are allowed.
- Optionally install a firmware image chosen on the technician’s computer. The image is served by TFTP from a chosen ethernet port on that computer. The device pulls it. The console starts the install and the reboot.
- Optionally run a burn-in for a whole number of hours the technician enters, at least 1. After a clean reboot, wait that many hours, then pass only when `show version | include uptime` reports at least that much uptime.
- Optionally walk every physical interface, one at a time. The technician plugs a secondary switch in by hand. A switch port passes when `show interface status` says `connected`. A router interface passes when it is up and the line protocol is up.
- Export a validation file that includes the chassis serial number, the configuration result, the validated burn-in time when burn-in was chosen, and the port results when port testing was chosen.
- Show a reminder for the whole run of the factory username, password, and enable secret the device actually accepted.

## Non-goals

- NX-OS, IOS-XR, small-business CLI, and access points. The same bench flow can gain those command sets later. They are not in this version.
- Devices that are not factory fresh or factory reset.
- Sending a firmware image down the console cable.
- Talking to the secondary switch. It is plugged in by hand.
- A byte-for-byte match between the uploaded file and the running config.
- Guessing a third password if `Passw0rd%` is also rejected.
- Driving more than one console cable.

## Anti-goals

- A release record that passes when a chosen check failed, was skipped, or never ran.
- A reboot that comes back without the configuration that was just applied.
- Treating uptime from before this run as the burn-in. The burn-in clock starts at the clean-slate reboot, and the program waits the entered hours before it reads uptime.
- A firmware copy that uses a temporary address as if it were part of the configuration under test.
- A reboot into a hashed password the program cannot type.

## Constraints

- The technician’s computer runs macOS or Windows. This version ships for both. Serial-port discovery, binding the TFTP ethernet port, permissions, and packaging are per system. The session core is shared.
- One serial console session. Cisco console defaults are 9600 8N1. The technician picks the local serial device, because the computer may have more than one.
- A current IOS-XE image is too large to copy at console speed. TFTP on a second cable is the path for the image. The configuration file still goes over the console.
- The program only starts against a factory or factory-reset device.
- Factory login has two shapes. If the console is open, decline the initial configuration dialog and enter `enable` with no secret. If first boot asks for an account, the username is `cisco`, the password is `cisco`, the confirmation is `cisco`, and the enable secret is `cisco`.
- If the device rejects that password as too short, resend the password and the enable secret as `Passw0rd%`, and confirm the password again. The username stays `cisco`. If `Passw0rd%` is rejected, stop and show the device’s reason.
- The reminder shows the factory username, password, and enable secret that were actually accepted. It stays visible for the run. It does not swap in secrets parsed from the configuration file.
- After the configuration is applied, a later login uses a password the program can actually type. Cleartext in the uploaded file counts (`username … password …`, `username … secret 0 …`, `enable password …`, `enable secret 0 …`, or `enable secret` / `username … secret` whose value is not already a hash). A type 7 secret in the file counts, because that file already contains a recoverable password. Hashed forms (`secret 5`, `secret 8`, `secret 9`, and values that begin with `$1$`, `$5$`, `$8$`, or `$9$`) do not.
- Use the factory credentials when the file does not change console login.
- When the file does change it, use the login only if it is unambiguous: exactly one privilege-15 local user with a typable password, or, if there is no local user, a single typable password on `line con 0`. If several privilege-15 users are typable, or the console line is set to AAA, stop before the reboot, name the lines, and leave the factory reminder up. Do not pick one. This stop is the same rule as refusing to reboot into a hash: the program does not guess which password the console will want.
- The device boots from startup-config. Save the applied configuration before any reboot that must come back configured. Do not save the temporary TFTP address.
- Uptime is time since boot. The clean-slate reboot is what makes a long previous uptime irrelevant. A reload during the wait drops uptime under the entered hours and the burn-in stays failed until a new clean wait completes.
- `show interface status` and the word `connected` are the switch check. Router interfaces are checked with `show interfaces <name>` and pass on `up` / line protocol `up`.
- Virtual interfaces (loopback, tunnel, VLAN interfaces, virtual templates, and the like) are not ports in the walk and are not the TFTP port. The TFTP port is the first physical interface the device reports, which may be a management port if the device lists that first.

## Approaches

Three ways to build the window were considered. All three keep the same Rust core for the serial session, TFTP, and the run.

- **Desktop window drawn with web layout.** The core owns the cable, the TFTP server, and the multi-hour run. The window is a web view, so the Cursor-like chrome is ordinary layout. This is the chosen approach. The look lands in CSS, and the session can be tested without the window.
- **Native editor toolkit.** The same core, with a fully native window in the family of UI Zed uses. It can feel more like an editor. The toolkit is younger than this program’s need to stay up through a firmware reboot and a multi-hour burn-in.
- **Simpler native toolkit.** The same core, with a smaller Rust UI toolkit. It is the straightest window to build, and it would look like a utility rather than Cursor.

A browser tab was set aside. It would not reliably own a serial port and a chosen ethernet interface for the length of a burn-in.

## Decisions

The technician chose the desktop window drawn with web layout.

Chosen at the start of each run, before anything is sent:

- The local serial device for the console cable.
- The configuration file. Required.
- Firmware upgrade, off unless chosen. When on, an image file and the ethernet port on this computer that will serve TFTP.
- Burn-in, off unless chosen. When on, a whole number of hours, minimum 1. The field can be typed. Plus and minus change it by 1.
- Port testing, off unless chosen.

The release record includes a check only when it was chosen, plus the configuration check, which always runs. Firmware, when chosen, passes when `show version` after the install reboot reports the version token in the image filename. If the filename has no version token, stop and say so before the install.

Port testing is not a step in the reboot list. If it was chosen, the walk is available once configuration validation has passed, including while burn-in is waiting. The burn-in clock does not wait for the walk. The record is not ready to release until every chosen check has passed.

The export is a text file the technician saves on this computer. It is offered once configuration validation has passed. If burn-in or port testing was chosen and has not finished, those sections say they are not finished yet, and a later export replaces the file with the finished results. The technician can export again.

## The run

### With firmware

1. Log in with the factory rules above.
2. Tell the technician to plug the ethernet cable from the chosen TFTP port into the first physical interface. Wait until they confirm the cable is in.
3. Give that interface a temporary IPv4 address on a dedicated link, no shutdown. On a switch, make that port routed for the transfer (`no switchport` and an address on the port). Do not write this address to startup-config.
4. Serve the chosen image by TFTP bound to the chosen local ethernet port. On the console, have the device copy that file to flash.
5. Install that image and reboot. This is the first reboot.
6. Log in again with the factory credentials. Confirm the running version. This is the firmware pass.
7. Apply the configuration file over the console. Save it to startup-config.
8. Reboot. Log in with the rules in Constraints. Validate the configuration.
9. Reboot again. This boot is the clean slate. Uptime starts here.
10. If burn-in was chosen, wait the entered hours, then run `show version | include uptime`. Pass when the reported uptime is at least the entered hours. Quote that line on the export.
11. The port walk, if chosen, can proceed any time after step 8.

### Without firmware

Start at step 7. The clean-slate reboot is still the reboot after validation. Burn-in and the port walk follow the same rules.

Applying the configuration means sending the file’s commands on the console (`configure terminal` and the file body, or an equivalent IOS-XE replace that leaves those commands in the running config). The pass is the comparison below, not the absence of errors alone. Errors while applying fail the configuration check and stop the run before a reboot.

## Configuration check

After the reboot that follows the apply, read the running config and compare it to the uploaded file.

- Ignore blank lines, `!` comments, and the running-config header (`Building configuration…`, `Current configuration : N bytes`, `Last configuration change…`).
- A command from the file is present when the running config shows that the command took effect. The line does not have to reappear character for character. IOS-XE rewrites some commands as it stores them, and those stored forms count:
  - An abbreviated interface name matches the expanded name.
  - `interface range` matches the member interfaces carrying those sub-commands.
  - A cleartext password or secret in the file matches the hash the device stored for that user or enable secret.
- Interface sub-commands count under the interface they apply to, including members expanded from a range.
- Order between sections does not have to match.
- Lines that exist only on the device do not fail the check.
- A command with no corresponding stored form fails the check. The record names the missing commands. Do not reboot into the clean slate after a failed comparison.

## Burn-in

The technician enters whole hours. The smallest value while burn-in is on is 1. There is no maximum.

The program waits that long in wall-clock time after the clean-slate reboot, then reads uptime. A 4 hour entry passes on an uptime of 4 hours and 6 minutes. It fails on 3 hours and 58 minutes. If the device prints more than one uptime line, every line must clear the entered hours.

The export records the hours entered and the uptime line the device printed.

## Port testing

The program lists physical interfaces and walks them one at a time. The screen names the port. The technician connects the secondary switch to that port and chooses Check this port.

- Switch: run `show interface status`. That port passes when its status is `connected`.
- Router: run `show interfaces <name>`. That port passes when the interface is up and the line protocol is up.
- Only the port just named has to be connected at the moment of the check. Earlier ports may be unplugged.
- A port the applied configuration has shut stays down with a cable in it and fails.
- The export lists every physical port and whether it passed.

## Firmware transfer

The technician picks the image file and the local ethernet port. The program tells them the Cisco interface by name and waits for confirmation. Example copy, when the device’s first physical interface is Gi1/0/1: plug the cable into Gi1/0/1.

The temporary address exists only for the copy. It is removed from the running config before the configuration file is applied, or it is simply never saved and the apply replaces the running config. It must not be required for the configuration check to pass, and it must not remain as an extra requirement the uploaded file has to contain.

The install-and-reboot command is the one current IOS-XE uses to activate an image that is already on flash and then reload. If the device rejects that command, stop and show the output. Do not claim the firmware passed.

## Export

A text file. Sections, when they apply:

- Chassis serial number, read from the device (`show inventory`, and `show version` if inventory has no chassis serial).
- Host name.
- Configuration file name, and that the running config contains the file’s commands. Include the missing commands when it does not.
- Firmware image file name and the running version, when firmware was chosen.
- Burn-in hours entered, the uptime line, and pass or fail, when burn-in was chosen.
- Each physical port and its result, when port testing was chosen.

## Screen

One window.

- Sidebar: the device on the console, the serial device, and the files chosen for this run.
- Main area: the start-of-run choices, then the live record (configuration, firmware, burn-in, ports) with pass, fail, or not finished.
- Password reminder across the top of the session whenever factory credentials were set or typed.
- Port walk replaces the main area while it is the active pass: the port name, the instruction to connect the secondary switch, Check this port, and the ports already passed.
- Firmware wait replaces the main area until the cable is confirmed: the interface name and Cable is in.
- Console transcript available from the window so the technician can see what the device printed.
- Export validation writes the text file and shows the same contents.

Ready to release means every check that belongs to this run has passed. The count is configuration, plus firmware if chosen, plus burn-in if chosen, plus ports if chosen.

## Calls

These were not stated by the technician. They are recorded so they can be checked.

- A command counts as present when the stored config shows it took effect. Expanded interface names, an `interface range` expanded into members, and a cleartext secret stored as a hash all count.
- After the file is applied, the next login is used only when it is unambiguous: one privilege-15 user with a typable password, or a single console-line password. A type 7 secret in the file is typable. Several such users, or AAA on the console, stops the run before that reboot.
- The port walk is available once configuration validation has passed, including during the burn-in wait. It is not inserted into the reboot list. Release still waits for it when it was chosen.
- The export is a text file the technician saves, and they can export again when a later check finishes.
- The console is 9600 8N1, and the technician picks which local serial device is the cable.
- The temporary TFTP address is placed on the named port itself. On a switch that port is made routed for the transfer. The address is never saved.
- Firmware passes when `show version` matches the version token in the image filename.
- The on-screen reminder keeps the factory username, password, and enable secret. It does not switch to secrets parsed out of the configuration file.
- Dioxus desktop is the expected window. Another web-layout desktop shell is acceptable when the screen in this document stays the same and the session core stays free of the UI.

## Left to the builder

- Crate split: a library for the session, and a desktop shell drawn with web layout. Dioxus desktop is the expected shell. Another web-layout desktop shell is acceptable if it keeps the same screen and the core stays UI-free.
- Two packages, macOS and Windows, from that shared core. Platform code is limited to the serial device, the chosen ethernet port, TFTP bind permissions, and the file dialogs.
- The exact temporary IPv4 pair on the TFTP link, as long as it is a single direct link, is not saved, and is gone before the configuration check.
- The exact IOS-XE install command, as long as it activates the image that was copied and then reboots, and a rejection stops the run.
- Serial-port discovery and the TFTP bind to the chosen NIC.
- How interface names are ordered when deciding the first physical port. Document the rule in code: skip virtual interfaces; keep physical ones, including a management port; take the first in the device’s own listing.
- Parsing details for uptime, interface status, and version tokens, within the pass rules above.
- Visual tokens that get the window close to Cursor without changing the screen structure in this document.

## Later

Other device families keep this flow and replace the commands. They are separate work after this version. An access point has no row of switch ports, so its port check cannot be this walk.
