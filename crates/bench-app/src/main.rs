mod view;

use std::path::Path;
use std::sync::mpsc::{self, Sender};
use std::time::Duration;

use bench_core::{release_heading, reminder_line, status_label, CheckStatus};
use dioxus::desktop::{Config, LogicalSize, WindowBuilder};
use dioxus::prelude::*;

use view::{screen_for, Screen, UiCommand, ViewState};

const STYLE: &str = include_str!("../assets/app.css");

fn main() {
    let (commands, receiver) = mpsc::channel();
    drop(receiver);
    open_window(commands, ViewState::new());
}

/// Opens the bench window on `initial` and sends the technician's commands on `commands`.
fn open_window(commands: Sender<UiCommand>, initial: ViewState) {
    LaunchBuilder::desktop()
        .with_cfg(
            Config::new()
                .with_window(
                    WindowBuilder::new()
                        .with_title("IOS-XE Bench")
                        .with_inner_size(LogicalSize::new(1180.0, 780.0))
                        .with_min_inner_size(LogicalSize::new(900.0, 600.0)),
                )
                .with_background_color((24, 24, 24, 255)),
        )
        .with_context(commands)
        .with_context(initial)
        .launch(App);
}

#[component]
fn App() -> Element {
    let state = use_signal(consume_context::<ViewState>);
    let mut console_open = use_signal(|| false);
    let mut export_pending = use_signal(|| false);
    let mut export_note = use_signal(|| None::<String>);

    use_effect(move || {
        let Some(text) = state.read().export_text.clone() else {
            return;
        };
        if *export_pending.peek() {
            export_pending.set(false);
            spawn(async move {
                export_note.set(Some(save_export(text).await));
            });
        }
    });

    let view = state.read();
    let screen = screen_for(&view);
    rsx! {
        style { {STYLE} }
        div { class: "window",
            div { class: "activity-bar", aria_hidden: "true",
                div { class: "app-mark", "XE" }
            }
            aside { class: "sidebar", aria_label: "On the console",
                Sidebar { state }
            }
            div { class: "workspace",
                if let Some(secrets) = &view.reminder {
                    div { class: "reminder", role: "status",
                        p { class: "reminder-title", "Password used on this device" }
                        p { class: "reminder-line mono", {reminder_line(secrets)} }
                    }
                }
                if let Some(reason) = &view.stop_reason {
                    div { class: "stop-banner", role: "alert",
                        p { class: "stop-title", "The run stopped" }
                        p { class: "stop-reason", "{reason}" }
                    }
                }
                main { class: "main",
                    {
                        match screen {
                            Screen::StartForm => rsx! {
                                StartFormScreen { state }
                            },
                            Screen::CablePrompt => rsx! {
                                CablePrompt { state }
                            },
                            Screen::PortWalk => rsx! {
                                PortWalk { state }
                            },
                            Screen::Record => rsx! {
                                RecordScreen { state, export_pending, export_note }
                            },
                        }
                    }
                }
                if console_open() {
                    section { class: "console-panel", aria_label: "Console transcript",
                        h2 { class: "console-title", "Console" }
                        div { class: "console-scroll",
                            if view.transcript.is_empty() {
                                p { class: "muted", "Nothing from the device yet." }
                            } else {
                                pre { class: "mono", "{view.transcript}" }
                            }
                        }
                    }
                }
            }
            footer { class: "status-bar",
                button {
                    class: "status-button",
                    aria_pressed: "{console_open}",
                    onclick: move |_| console_open.toggle(),
                    "Console"
                }
                span { "9600 8N1" }
                if !view.form.serial_device.is_empty() {
                    span { class: "mono", "{view.form.serial_device}" }
                }
            }
        }
    }
}

#[component]
fn Sidebar(state: Signal<ViewState>) -> Element {
    let view = state.read();
    let form = &view.form;
    rsx! {
        h2 { class: "sidebar-title", "On the console" }
        div { class: "device-card",
            match &view.hostname {
                Some(name) => rsx! {
                    p { class: "device-name", "{name}" }
                },
                None => rsx! {
                    p { class: "device-name unknown", "Host name not read yet" }
                },
            }
            dl { class: "device-facts",
                Fact {
                    label: "Serial device",
                    value: form.serial_device.clone(),
                    full: form.serial_device.clone(),
                }
                Fact {
                    label: "Configuration file",
                    value: file_name(&form.config_path),
                    full: form.config_path.clone(),
                }
                if form.firmware_on {
                    Fact {
                        label: "Firmware image",
                        value: file_name(&form.image_path),
                        full: form.image_path.clone(),
                    }
                }
            }
        }
    }
}

#[component]
fn Fact(label: &'static str, value: String, full: String) -> Element {
    rsx! {
        dt { "{label}" }
        if value.is_empty() {
            dd { class: "muted", "Not chosen" }
        } else {
            dd { class: "mono", title: "{full}", "{value}" }
        }
    }
}

#[component]
fn StartFormScreen(mut state: Signal<ViewState>) -> Element {
    let commands = use_context::<Sender<UiCommand>>();
    let mut error = use_signal(|| None::<&'static str>);
    let view = state.read();
    let form = &view.form;
    let hours = if form.hours == 0 {
        String::new()
    } else {
        form.hours.to_string()
    };
    rsx! {
        h1 { class: "screen-title", "Start a run" }
        div { class: "form",
            section { class: "card",
                label { class: "field-label", r#for: "serial-device", "Serial device" }
                DeviceSelect {
                    id: "serial-device",
                    placeholder: "Choose a serial device",
                    options: view.serial_devices.clone(),
                    current: form.serial_device.clone(),
                    onpick: move |name| state.write().form.serial_device = name,
                }
            }
            section { class: "card",
                FileField {
                    id: "config-file",
                    label: "Configuration file",
                    path: form.config_path.clone(),
                    onpick: move |path| state.write().form.config_path = path,
                }
            }
            section { class: "card",
                Toggle {
                    id: "firmware-on",
                    label: "Upgrade firmware",
                    checked: form.firmware_on,
                    ontoggle: move |on| state.write().form.firmware_on = on,
                }
                if form.firmware_on {
                    div { class: "card-body",
                        FileField {
                            id: "image-file",
                            label: "Firmware image",
                            path: form.image_path.clone(),
                            onpick: move |path| state.write().form.image_path = path,
                        }
                        div { class: "field",
                            label { class: "field-label", r#for: "local-ethernet", "Ethernet port on this computer" }
                            DeviceSelect {
                                id: "local-ethernet",
                                placeholder: "Choose an ethernet port",
                                options: view.ethernet_ports.clone(),
                                current: form.local_ethernet.clone(),
                                onpick: move |name| state.write().form.local_ethernet = name,
                            }
                        }
                    }
                }
            }
            section { class: "card",
                Toggle {
                    id: "burn-in-on",
                    label: "Burn-in",
                    checked: form.burn_in_on,
                    ontoggle: move |on| state.write().form.burn_in_on = on,
                }
                if form.burn_in_on {
                    div { class: "card-body",
                        label { class: "hours-label", r#for: "burn-in-hours", "Length, in whole hours" }
                        div { class: "stepper",
                            button {
                                class: "button step",
                                aria_label: "One hour less",
                                onclick: move |_| state.write().form.step_hours(-1),
                                "−"
                            }
                            input {
                                id: "burn-in-hours",
                                class: "hours-input mono",
                                r#type: "number",
                                min: "1",
                                step: "1",
                                inputmode: "numeric",
                                value: "{hours}",
                                oninput: move |event: FormEvent| {
                                    state.write().form.hours = event.value().trim().parse().unwrap_or(0);
                                },
                            }
                            button {
                                class: "button step",
                                aria_label: "One hour more",
                                onclick: move |_| state.write().form.step_hours(1),
                                "+"
                            }
                            span { "hours" }
                        }
                    }
                }
            }
            section { class: "card",
                Toggle {
                    id: "port-testing",
                    label: "Port testing",
                    checked: form.port_testing,
                    ontoggle: move |on| state.write().form.port_testing = on,
                }
            }
            div { class: "form-actions",
                button {
                    class: "button primary",
                    onclick: move |_| {
                        let options = state.read().form.to_options();
                        match options {
                            Ok(options) => {
                                error.set(None);
                                let _ = commands.send(UiCommand::Start(options));
                            }
                            Err(message) => error.set(Some(message)),
                        }
                    },
                    "Start"
                }
                if let Some(message) = error() {
                    p { class: "form-error", role: "alert", "{message}" }
                }
            }
        }
    }
}

#[component]
fn DeviceSelect(
    id: &'static str,
    placeholder: &'static str,
    options: Vec<String>,
    current: String,
    onpick: EventHandler<String>,
) -> Element {
    rsx! {
        select {
            id,
            class: "select mono",
            onchange: move |event: FormEvent| onpick.call(event.value()),
            option { value: "", disabled: true, selected: current.is_empty(),
                if options.is_empty() {
                    "None found"
                } else {
                    "{placeholder}"
                }
            }
            for name in options {
                option { value: "{name}", selected: name == current, "{name}" }
            }
        }
    }
}

#[component]
fn FileField(
    id: &'static str,
    label: &'static str,
    path: String,
    onpick: EventHandler<String>,
) -> Element {
    let label_id = format!("{id}-label");
    rsx! {
        div { class: "field",
            p { class: "field-label", id: "{label_id}", "{label}" }
            div { class: "file-row",
                if path.is_empty() {
                    p { class: "path empty", "No file chosen" }
                } else {
                    p { class: "path mono", title: "{path}", "{path}" }
                }
                button {
                    class: "button",
                    aria_describedby: "{label_id}",
                    onclick: move |_| {
                        spawn(async move {
                            let picked = rfd::AsyncFileDialog::new().set_title(label).pick_file().await;
                            if let Some(file) = picked {
                                onpick.call(file.path().display().to_string());
                            }
                        });
                    },
                    "Choose file"
                }
            }
        }
    }
}

#[component]
fn Toggle(
    id: &'static str,
    label: &'static str,
    checked: bool,
    ontoggle: EventHandler<bool>,
) -> Element {
    rsx! {
        div { class: "toggle-row",
            input {
                id,
                class: "switch",
                r#type: "checkbox",
                role: "switch",
                checked,
                onchange: move |event: FormEvent| ontoggle.call(event.checked()),
            }
            label { r#for: id, "{label}" }
        }
    }
}

#[component]
fn CablePrompt(state: Signal<ViewState>) -> Element {
    let commands = use_context::<Sender<UiCommand>>();
    let view = state.read();
    rsx! {
        p { class: "prompt-lead", "Plug the ethernet cable from this computer’s TFTP port into" }
        match &view.tftp_port {
            Some(port) => rsx! {
                p { class: "port-name mono", "{port}" }
                p { class: "prompt-note", "That is the first physical port the device reported." }
            },
            None => rsx! {
                p { class: "port-name waiting", "Waiting for the device to name its first physical port." }
            },
        }
        div { class: "prompt-action",
            button {
                class: "button primary",
                disabled: view.tftp_port.is_none(),
                onclick: move |_| {
                    let _ = commands.send(UiCommand::ConfirmCable);
                },
                "Cable is in"
            }
        }
    }
}

#[component]
fn PortWalk(state: Signal<ViewState>) -> Element {
    let commands = use_context::<Sender<UiCommand>>();
    let view = state.read();
    let current = view.current_port.clone().unwrap_or_default();
    let passed: Vec<String> = view
        .ports
        .iter()
        .filter(|(_, status)| *status == CheckStatus::Passed)
        .map(|(name, _)| name.clone())
        .collect();
    let failure = view.ports.iter().find_map(|(name, status)| match status {
        CheckStatus::Failed { detail } if *name == current => Some(detail.clone()),
        _ => None,
    });
    let summary = format!(
        "Ports already passed · {} of {}",
        passed.len(),
        view.ports.len()
    );
    let port = current.clone();
    rsx! {
        p { class: "prompt-lead", "Connect the secondary switch" }
        p { class: "port-name mono", "{current}" }
        p { class: "prompt-note",
            "Plug the secondary switch into this port. The earlier ports can be unplugged. This check looks at this port only."
        }
        if let Some(detail) = failure {
            div { class: "check-failed", role: "alert",
                p { "The last check of this port failed." }
                pre { class: "detail mono", "{detail}" }
            }
        }
        div { class: "prompt-action",
            button {
                class: "button primary",
                onclick: move |_| {
                    let _ = commands.send(UiCommand::CheckPort(port.clone()));
                },
                "Check this port"
            }
        }
        h2 { class: "section-title", "{summary}" }
        if passed.is_empty() {
            p { class: "muted", "No port has passed yet." }
        } else {
            ul { class: "port-grid",
                for name in passed {
                    li { class: "port-chip",
                        span { class: "mono", "{name}" }
                        " "
                        span { class: "status-pass", "passed" }
                    }
                }
            }
        }
    }
}

#[component]
fn RecordScreen(
    mut state: Signal<ViewState>,
    mut export_pending: Signal<bool>,
    mut export_note: Signal<Option<String>>,
) -> Element {
    let commands = use_context::<Sender<UiCommand>>();
    let view = state.read();
    let record = &view.record;
    let form = &view.form;
    let checks = [
        &record.configuration,
        &record.firmware,
        &record.burn_in,
        &record.ports,
    ];
    let chosen = checks
        .iter()
        .filter(|status| ***status != CheckStatus::NotChosen)
        .count();
    let passed = checks
        .iter()
        .filter(|status| ***status == CheckStatus::Passed)
        .count();
    let ports_passed = view
        .ports
        .iter()
        .filter(|(_, status)| *status == CheckStatus::Passed)
        .count();
    let can_export = record.configuration == CheckStatus::Passed;
    let burn_in_note = form.burn_in_on.then(|| match view.burn_in_remaining {
        Some(left) => format!("{} · {} left", hours_text(form.hours), remaining_text(left)),
        None => hours_text(form.hours),
    });
    let ports_note = form
        .port_testing
        .then(|| format!("{ports_passed} of {} ports passed", view.ports.len()));
    rsx! {
        div { class: "record-header",
            h1 { class: "screen-title", {release_heading(record)} }
            p { class: "muted", "{passed} of {chosen} passed" }
        }
        ul { class: "checks",
            CheckRow {
                name: "Configuration",
                status: record.configuration.clone(),
                file: Some(file_name(&form.config_path)),
            }
            CheckRow {
                name: "Firmware",
                status: record.firmware.clone(),
                file: form.firmware_on.then(|| file_name(&form.image_path)),
            }
            CheckRow { name: "Burn-in", status: record.burn_in.clone(), note: burn_in_note }
            CheckRow { name: "Ports", status: record.ports.clone(), note: ports_note }
        }
        if form.port_testing {
            h2 { class: "section-title", "Ports" }
            if view.ports.is_empty() {
                p { class: "muted", "The device has not listed its ports yet." }
            } else {
                ul { class: "port-grid",
                    for (name, status) in view.ports.iter() {
                        li { class: "port-chip",
                            span { class: "mono", "{name}" }
                            " "
                            span { class: status_class(status), {status_label(status)} }
                        }
                    }
                }
            }
        }
        section { class: "card export",
            div { class: "export-row",
                if can_export {
                    p { "Verification is done, so the export can be saved." }
                } else {
                    p { class: "muted", "Export is available once the configuration check passes." }
                }
                button {
                    class: "button primary",
                    disabled: !can_export,
                    onclick: move |_| {
                        export_pending.set(true);
                        export_note.set(None);
                        state.write().export_text = None;
                        let _ = commands.send(UiCommand::Export);
                    },
                    "Export validation"
                }
            }
            if let Some(note) = export_note() {
                p { class: "export-note", role: "status", "{note}" }
            }
            if let Some(text) = &view.export_text {
                div { class: "export-preview",
                    h2 { "Exported validation" }
                    pre { class: "mono", "{text}" }
                }
            }
        }
    }
}

#[component]
fn CheckRow(
    name: &'static str,
    status: CheckStatus,
    file: Option<String>,
    note: Option<String>,
) -> Element {
    let status_classes = format!("check-status {}", status_class(&status));
    rsx! {
        li { class: "check",
            div { class: "check-head",
                div {
                    p { class: "check-name", "{name}" }
                    if let Some(file) = file {
                        p { class: "check-sub mono", "{file}" }
                    }
                    if let Some(note) = note {
                        p { class: "check-sub", "{note}" }
                    }
                }
                p { class: status_classes, {status_label(&status)} }
            }
            if let CheckStatus::Failed { detail } = &status {
                pre { class: "detail mono", "{detail}" }
            }
        }
    }
}

async fn save_export(text: String) -> String {
    let Some(file) = rfd::AsyncFileDialog::new()
        .set_file_name("validation.txt")
        .save_file()
        .await
    else {
        return "The export was not saved.".to_string();
    };
    match std::fs::write(file.path(), text) {
        Ok(()) => format!("Saved to {}", file.path().display()),
        Err(err) => format!("Could not save to {}: {err}", file.path().display()),
    }
}

fn status_class(status: &CheckStatus) -> &'static str {
    match status {
        CheckStatus::Passed => "status-pass",
        CheckStatus::Failed { .. } => "status-fail",
        CheckStatus::NotFinished => "status-pending",
        CheckStatus::NotChosen => "status-muted",
    }
}

fn file_name(path: &str) -> String {
    Path::new(path).file_name().map_or_else(
        || path.to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

fn hours_text(hours: u32) -> String {
    if hours == 1 {
        "1 hour".to_string()
    } else {
        format!("{hours} hours")
    }
}

fn remaining_text(left: Duration) -> String {
    let minutes = left.as_secs().div_ceil(60);
    format!("{}h {:02}m", minutes / 60, minutes % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remaining_time_rounds_up_to_the_minute() {
        assert_eq!(
            remaining_text(Duration::from_secs(3 * 3600 + 12 * 60)),
            "3h 12m"
        );
        assert_eq!(remaining_text(Duration::from_secs(30)), "0h 01m");
        assert_eq!(remaining_text(Duration::ZERO), "0h 00m");
    }
}
