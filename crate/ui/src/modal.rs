//! Modal dialog types and their UI functions.
//!
//! Each modal captures its own state and exposes a `CloseAction` that the
//! caller (`AutosshApp`) interprets to apply or discard changes.

use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

use autossh_core::{
    Config, ConnectionConfig, ForwardConfig, ForwardMode, KeepaliveConfig, test_connection,
};
use eframe::egui::{self, Color32, RichText};

pub use crate::ssh_config::SshHostEntry;
use crate::ssh_config::parse_ssh_config;

use crate::log::{FG_DIM, FG_ERROR, FG_MUTED, FG_PRIMARY, FG_SUCCESS, FG_WARNING};

// ─── Pickable trait (shared by both import dialogs) ───────────────────────

trait Pickable {
    fn picked(&self) -> bool;
    fn set_picked(&mut self, v: bool);
    fn is_dup(&self) -> bool;
}

/// Render a scrollable checklist with select-all, used by both import dialogs.
fn pick_list<T: Pickable>(ui: &mut egui::Ui, items: &mut [T], render: impl Fn(&T, &mut egui::Ui)) {
    let mut all = items.iter().filter(|c| !c.is_dup()).all(|c| c.picked());
    ui.horizontal(|ui| {
        if ui.checkbox(&mut all, "select all").changed() {
            for c in items.iter_mut().filter(|c| !c.is_dup()) {
                c.set_picked(all);
            }
        }
    });
    egui::ScrollArea::vertical()
        .max_height(220.0)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for c in items.iter_mut() {
                let mut sel = c.picked();
                ui.horizontal(|ui| {
                    ui.add_enabled(!c.is_dup(), egui::Checkbox::new(&mut sel, ""));
                    if sel != c.picked() {
                        c.set_picked(sel);
                    }
                    render(c, ui);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if c.is_dup() {
                            ui.colored_label(FG_WARNING, "(duplicate)");
                        } else if c.picked() {
                            ui.colored_label(FG_PRIMARY, "✓");
                        }
                    });
                });
            }
        });
}

impl Pickable for CandidateConnection {
    fn picked(&self) -> bool {
        self.selected
    }
    fn set_picked(&mut self, v: bool) {
        self.selected = v;
    }
    fn is_dup(&self) -> bool {
        self.duplicate
    }
}

impl Pickable for SshHostEntry {
    fn picked(&self) -> bool {
        self.selected
    }
    fn set_picked(&mut self, v: bool) {
        self.selected = v;
    }
    fn is_dup(&self) -> bool {
        self.duplicate
    }
}

// ─── enums ─────────────────────────────────────────────────────────────────────

/// Which global field is being edited in the `EditGlobal` modal.
#[derive(Copy, Clone, PartialEq, Eq)]
pub enum GlobalGroup {
    KeepaliveInterval,
    KeepaliveCount,
    KeepaliveTimeout,
    RetryInitial,
    RetryMaximum,
    RetryStable,
}

impl GlobalGroup {
    pub fn label(self) -> &'static str {
        match self {
            Self::KeepaliveInterval => "keepalive.interval (s)",
            Self::KeepaliveCount => "keepalive.count_max",
            Self::KeepaliveTimeout => "keepalive.connect_timeout (s)",
            Self::RetryInitial => "retry.initial_seconds (s)",
            Self::RetryMaximum => "retry.maximum_seconds (s)",
            Self::RetryStable => "retry.stable_seconds (s)",
        }
    }
}

/// What the user did inside the dialog — used by the caller to decide whether
/// and how to commit the state.
#[derive(Clone, Debug, Default)]
pub enum CloseAction {
    #[default]
    None,
    Commit,
    Cancel(&'static str),
}

// ─── modal enum ────────────────────────────────────────────────────────────────

/// Modal dialogs for adding a connection, editing a single global value,
/// or importing hosts from another TOML config file.
#[derive(Default)]
pub enum Modal {
    #[default]
    None,
    Add(AddDialogState),
    /// Editing an existing connection; `idx` is the row in `config.connections`.
    EditConnection {
        idx: usize,
        state: AddDialogState,
    },
    EditGlobal {
        group: GlobalGroup,
        value: String,
    },
    Import(ImportDialogState),
    ImportSsh(SshImportState),
}

// ─── dialog state types ────────────────────────────────────────────────────────

/// Asynchronous state of the “test connection” probe shown in the add/edit dialog.
#[derive(Clone, Default)]
pub enum TestStatus {
    #[default]
    Idle,
    Running,
    Done {
        ok: bool,
        message: String,
    },
}

pub struct AddDialogState {
    pub name: String,
    pub description: String,
    pub host: String,
    /// Optional SSH user. When set, the effective destination becomes
    /// `user@host` (unless `host` itself already looks like `user@host`).
    pub user: String,
    /// Optional password for non-interactive password auth via `sshpass -e`.
    /// Empty when only key/agent auth is desired (the default).
    pub password: String,
    /// Optional SSH server port; empty delegates to OpenSSH's default (22).
    pub port: String,
    /// Each entry is editable in-place inside the dialog (mode dropdown, free
    /// text, delete). Converted to `Vec<ForwardConfig>` only on commit.
    pub forwards: Vec<EditableForward>,
    pub checked: Vec<bool>,
    pub draft_mode: ForwardMode,
    pub draft_forward: String,
    pub draft_enabled: bool,
    pub draft_checked: bool,
    /// Optional note captured for the next forward the user is about to add;
    /// moved into the freshly-pushed `EditableForward` on commit.
    pub draft_desc: String,
    pub close: CloseAction,
    /// Live state of the “test connection” probe.
    pub test_status: TestStatus,
    /// Channel written by the background ssh probe and polled each frame.
    /// `Some((ok, msg))` indicates the result is ready to be drained.
    pub test_channel: Arc<Mutex<Option<(bool, String)>>>,
}

impl Default for AddDialogState {
    fn default() -> Self {
        Self {
            name: String::new(),
            description: String::new(),
            host: String::new(),
            user: String::new(),
            password: String::new(),
            port: String::new(),
            forwards: Vec::new(),
            checked: Vec::new(),
            // New connections default to remote forwards (the canonical autossh
            // pattern of opening an inbound tunnel to a client).
            draft_mode: ForwardMode::Remote,
            draft_forward: String::new(),
            draft_enabled: true,
            draft_checked: false,
            draft_desc: String::new(),
            close: CloseAction::None,
            test_status: TestStatus::default(),
            test_channel: Arc::new(Mutex::new(None)),
        }
    }
}

#[derive(Clone)]
pub struct EditableForward {
    pub enabled: bool,
    pub mode: ForwardMode,
    pub forward: String,
    /// Free-form note displayed alongside the forward in the GUI; empty
    /// string falls back to `None` on commit so the TOML stays clean.
    pub description: String,
}

impl EditableForward {
    pub fn into_forward(self) -> ForwardConfig {
        ForwardConfig {
            enabled: self.enabled,
            mode: self.mode,
            forward: self.forward,
            description: field_into_option(&self.description),
        }
    }
}

pub struct EditDialogState {
    pub group: GlobalGroup,
    pub value: String,
    pub close: CloseAction,
}

#[derive(Default)]
pub struct ImportDialogState {
    pub path_input: String,
    pub status: ImportStatus,
    pub candidates: Vec<CandidateConnection>,
    pub close: CloseAction,
}

#[derive(Debug, Default)]
pub enum ImportStatus {
    #[default]
    Idle,
    Loaded(PathBuf),
    Failed(String),
}

#[derive(Default)]
pub struct SshImportState {
    pub source_path: PathBuf,
    pub candidates: Vec<SshHostEntry>,
    pub status: SshImportStatus,
    pub close: CloseAction,
}

#[derive(Debug, Default)]
pub enum SshImportStatus {
    #[default]
    Idle,
    Loaded,
    Failed(String),
}

pub struct CandidateConnection {
    pub name: String,
    pub host: String,
    pub forwards: Vec<ForwardConfig>,
    pub selected: bool,
    pub duplicate: bool,
}

/// Map an editable dialog field to its `Option<String>` storage form: an empty
/// input writes `None` (fallback to default behaviour), a non-empty value is
/// trimmed and wrapped.
pub(crate) fn field_into_option(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Parse an optional SSH port, rejecting zero and non-numeric values.
pub(crate) fn field_into_port(value: &str) -> Result<Option<u16>, &'static str> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    match value.parse::<u16>() {
        Ok(port @ 1..=u16::MAX) => Ok(Some(port)),
        _ => Err("port must be an integer from 1 to 65535"),
    }
}

// ─── test connection helpers ─────────────────────────────────────────────────────

/// Drain the background ssh probe channel; once a result arrives, flip
/// `test_status` from `Running` to `Done`. While still running, request a
/// repaint so the dialog keeps polling without user interaction.
fn poll_test(state: &mut AddDialogState, ctx: &egui::Context) {
    if !matches!(state.test_status, TestStatus::Running) {
        return;
    }
    let result = state
        .test_channel
        .try_lock()
        .ok()
        .and_then(|mut g| g.take());
    if let Some((ok, message)) = result {
        state.test_status = TestStatus::Done { ok, message };
    } else {
        ctx.request_repaint_after(Duration::from_millis(100));
    }
}

/// Build a throwaway `ConnectionConfig` for the probe so the dialog can test
/// before the row has been committed. Forwards are irrelevant for an auth-only
/// probe, so they are left empty.
fn connection_for_test(state: &AddDialogState) -> ConnectionConfig {
    ConnectionConfig {
        name: state.name.trim().to_string(),
        description: field_into_option(&state.description),
        host: Some(state.host.trim().to_string()),
        user: field_into_option(&state.user),
        password: field_into_option(&state.password),
        port: field_into_port(&state.port).unwrap_or(None),
        enabled: true,
        ssh_path: None,
        extra_args: Vec::new(),
        forwards: Vec::new(),
    }
}

/// Spawn a background thread that runs `ssh … true` and reports the outcome
/// through `test_channel`. The probe is capped at 20 s — well above the default
/// 15 s `ConnectTimeout` yet short enough not to stall the dialog.
fn spawn_test(state: &mut AddDialogState) {
    let connection = connection_for_test(state);
    let keepalive = KeepaliveConfig::default();
    state.test_status = TestStatus::Running;
    state.test_channel = Arc::new(Mutex::new(None));
    let channel = Arc::clone(&state.test_channel);
    let _ = std::thread::Builder::new()
        .name("ssh-test".into())
        .spawn(move || {
            let output = test_connection(&connection, &keepalive, Duration::from_secs(20));
            if let Ok(mut guard) = channel.lock() {
                *guard = Some((output.ok, output.message));
            }
        });
}

// ─── helpers ───────────────────────────────────────────────────────────────────

/// Compact label used in the per-row mode dropdown (existing forwards + draft).
fn forward_mode_label(mode: ForwardMode) -> &'static str {
    match mode {
        ForwardMode::Local => "L local",
        ForwardMode::Remote => "R remote",
        ForwardMode::Dynamic => "D dynamic",
    }
}

/// Mode-aware placeholder: hint states which side listens vs targets.
fn forward_hint(mode: ForwardMode) -> &'static str {
    match mode {
        // Hints are intentionally short so they fit inside the ~225 px column
        // reserved for the spec input alongside the per-forward description.
        ForwardMode::Local => "5173  or  5173:127.0.0.1:5173  (L)",
        ForwardMode::Remote => "5173  or  5173:127.0.0.1:5173  (R)",
        ForwardMode::Dynamic => "1080  (D SOCKS)",
    }
}

/// Build an `AddDialogState` from an existing connection (e.g. for editing).
pub fn state_from_connection(c: &ConnectionConfig) -> AddDialogState {
    let n = c.forwards.len();
    AddDialogState {
        name: c.name.clone(),
        description: c.description.clone().unwrap_or_default(),
        host: c.host.clone().unwrap_or_else(|| c.name.clone()),
        user: c.user.clone().unwrap_or_default(),
        password: c.password.clone().unwrap_or_default(),
        port: c.port.map(|port| port.to_string()).unwrap_or_default(),
        forwards: c
            .forwards
            .iter()
            .map(|f| EditableForward {
                enabled: f.enabled,
                mode: f.mode,
                forward: f.forward.clone(),
                description: f.description.clone().unwrap_or_default(),
            })
            .collect(),
        checked: vec![false; n],
        // Carry the same default forward type as the add dialog for new lines.
        draft_mode: ForwardMode::Remote,
        draft_forward: String::new(),
        draft_enabled: true,
        draft_checked: false,
        draft_desc: String::new(),
        close: CloseAction::None,
        test_status: TestStatus::default(),
        test_channel: Arc::new(Mutex::new(None)),
    }
}

// ─── dialog UI: add / edit connection ─────────────────────────────────────────

pub(crate) fn selection_checkbox(ui: &mut egui::Ui, checked: &mut bool) -> egui::Response {
    ui.scope(|ui| {
        let color = if ui.visuals().dark_mode {
            egui::Color32::from_gray(160)
        } else {
            egui::Color32::from_gray(125)
        };
        ui.visuals_mut().widgets.inactive.bg_stroke = egui::Stroke::new(1.0, color);
        ui.checkbox(checked, "")
    })
    .inner
}

pub(crate) fn enabled_dot(ui: &mut egui::Ui, enabled: &mut bool) {
    let color = if *enabled { FG_SUCCESS } else { FG_MUTED };
    let status = if *enabled { "enabled" } else { "disabled" };
    let (rect, response) = ui.allocate_exact_size(egui::vec2(12.0, 18.0), egui::Sense::click());
    ui.painter().circle_filled(rect.center(), 4.0, color);
    if response
        .on_hover_text(format!("{status}; click to toggle"))
        .clicked()
    {
        *enabled = !*enabled;
    }
}

fn forward_number(ui: &mut egui::Ui, number: usize) {
    ui.add_sized(
        [24.0, 18.0],
        egui::Label::new(RichText::new(format!("{number}.")).small().color(FG_MUTED)),
    );
}

fn delete_forward_button(ui: &mut egui::Ui) -> bool {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(18.0, 22.0), egui::Sense::click());
    let stroke = egui::Stroke::new(
        1.5,
        if response.hovered() {
            FG_ERROR
        } else {
            FG_MUTED
        },
    );
    let center = rect.center();
    ui.painter().line_segment(
        [
            center + egui::vec2(-4.0, -4.0),
            center + egui::vec2(4.0, 4.0),
        ],
        stroke,
    );
    ui.painter().line_segment(
        [
            center + egui::vec2(-4.0, 4.0),
            center + egui::vec2(4.0, -4.0),
        ],
        stroke,
    );
    response.on_hover_text("Delete port").clicked()
}

fn push_draft_forward(state: &mut AddDialogState) {
    let forward = state.draft_forward.trim().to_string();
    if forward.is_empty() {
        return;
    }
    state.forwards.push(EditableForward {
        enabled: state.draft_enabled,
        mode: state.draft_mode,
        forward,
        description: std::mem::take(&mut state.draft_desc),
    });
    state.checked.push(state.draft_checked);
    state.draft_forward.clear();
    state.draft_enabled = true;
    state.draft_checked = false;
}

pub fn run_add_dialog_ui(ui: &mut egui::Ui, state: &mut AddDialogState, name_taken: bool) {
    // Esc closes the dialog whether the user has filled fields or not.
    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        state.close = CloseAction::Cancel("cancelled");
    }

    // Drain the background ssh probe if it has produced a result.
    poll_test(state, ui.ctx());

    // ── host identity ─────────────────────────────────────────
    // Row 1 is (Name, Description); row 2 is (Host, User, Password, Port).
    // Each cell is allocated with an exact `vec2(width, height)` via
    // `allocate_ui`, and the inner TextEdit asks for the same width via
    // `desired_width`, so the outer box and the inner field line up pixel
    // for pixel. `allocate_ui` consumes ~8 px of internal padding on each
    // side, so the visible inter-field gap is `COL_GAP + 8`. The two rows
    // are computed to share identical geometry so Description's left edge
    // sits flush with User's and its right edge sits flush with Port's.
    const NAME_HOST_WIDTH: f32 = 180.0;
    const USER_WIDTH: f32 = 120.0;
    const PASSWORD_WIDTH: f32 = 170.0;
    const PORT_WIDTH: f32 = 70.0;
    const COL_GAP: f32 = 8.0;
    const FIELD_PADDING: f32 = 8.0; // egui's allocate_ui internal padding
    const GAP_TOTAL: f32 = COL_GAP + FIELD_PADDING;
    // Description start = NAME_HOST_WIDTH + GAP_TOTAL (== User's left edge).
    // Description end must equal Port's right edge:
    //   NAME_HOST_WIDTH + GAP_TOTAL + DESCRIPTION_WIDTH
    //   = NAME_HOST_WIDTH + GAP_TOTAL + USER_WIDTH + GAP_TOTAL + PASSWORD_WIDTH + GAP_TOTAL + PORT_WIDTH
    // so DESCRIPTION_WIDTH = USER + PASSWORD + PORT + 2*GAP_TOTAL.
    const DESCRIPTION_WIDTH: f32 = USER_WIDTH + GAP_TOTAL + PASSWORD_WIDTH + GAP_TOTAL + PORT_WIDTH;
    const FIELD_HEIGHT: f32 = 40.0;

    // Forwards-row column widths (pixels). Each row is four columns:
    //  N.☐  |  mode combo  |  forward spec  |  description
    // Three inter-column gaps (COL_GAP = 8 px each) separate them.
    // The total row width equals FORWARDS_AREA_WIDTH so the description's
    // right edge aligns with the connection row's Port field right edge.
    const FORWARDS_AREA_WIDTH: f32 = NAME_HOST_WIDTH
        + GAP_TOTAL
        + USER_WIDTH
        + GAP_TOTAL
        + PASSWORD_WIDTH
        + GAP_TOTAL
        + PORT_WIDTH;
    const FORWARD_NUM_W: f32 = 72.0;
    const FORWARD_MODE_W: f32 = 110.0;
    const FORWARD_SPEC_W: f32 = 150.0;
    // `allocate_ui` shrinks the compact number/status and mode cells to their
    // contents. Compensate that reclaimed width so the last input and group
    // border end at the same x-coordinate as the Port field above.
    const FORWARD_END_COMPENSATION: f32 = 32.0;
    const FORWARD_DESC_W: f32 = FORWARDS_AREA_WIDTH
        - FORWARD_NUM_W
        - GAP_TOTAL
        - FORWARD_MODE_W
        - GAP_TOTAL
        - FORWARD_SPEC_W
        - GAP_TOTAL
        + FORWARD_END_COMPENSATION
        - 18.0;
    const FORWARD_ROW_H: f32 = 28.0;

    ui.add_space(2.0);
    ui.horizontal(|ui| {
        ui.allocate_ui(egui::vec2(NAME_HOST_WIDTH, FIELD_HEIGHT), |ui| {
            ui.vertical(|ui| {
                ui.label(RichText::new("Name").strong().color(FG_PRIMARY));
                ui.add(
                    egui::TextEdit::singleline(&mut state.name)
                        .hint_text("home-server")
                        .desired_width(NAME_HOST_WIDTH),
                );
            });
        });
        ui.add_space(COL_GAP);
        ui.allocate_ui(egui::vec2(DESCRIPTION_WIDTH, FIELD_HEIGHT), |ui| {
            ui.vertical(|ui| {
                ui.label(RichText::new("Description").strong().color(FG_PRIMARY));
                ui.add(
                    egui::TextEdit::singleline(&mut state.description)
                        .hint_text("optional note")
                        .desired_width(DESCRIPTION_WIDTH),
                );
            });
        });
    });
    if name_taken && !state.name.trim().is_empty() {
        ui.label(
            RichText::new("Name already exists; choose a unique name.")
                .small()
                .color(FG_ERROR),
        );
    }
    ui.add_space(3.0);
    ui.horizontal(|ui| {
        ui.allocate_ui(egui::vec2(NAME_HOST_WIDTH, FIELD_HEIGHT), |ui| {
            ui.vertical(|ui| {
                ui.label(RichText::new("Host").strong().color(FG_PRIMARY))
                    .on_hover_text("SSH alias or user@host");
                ui.add(
                    egui::TextEdit::singleline(&mut state.host)
                        .hint_text("example.com")
                        .desired_width(NAME_HOST_WIDTH),
                );
            });
        });
        ui.add_space(COL_GAP);
        ui.allocate_ui(egui::vec2(USER_WIDTH, FIELD_HEIGHT), |ui| {
            ui.vertical(|ui| {
                ui.label(RichText::new("User").strong().color(FG_PRIMARY))
                    .on_hover_text("Optional; overrides the user in Host");
                ui.add(
                    egui::TextEdit::singleline(&mut state.user)
                        .hint_text("optional")
                        .desired_width(USER_WIDTH),
                );
            });
        });
        ui.add_space(COL_GAP);
        ui.allocate_ui(egui::vec2(PASSWORD_WIDTH, FIELD_HEIGHT), |ui| {
            ui.vertical(|ui| {
                ui.label(RichText::new("Password").strong().color(FG_PRIMARY))
                    .on_hover_text("Optional; stored as plaintext; requires sshpass beside autossh-ui or on PATH");
                ui.add(
                    egui::TextEdit::singleline(&mut state.password)
                        .password(true)
                        .hint_text("optional")
                        .desired_width(PASSWORD_WIDTH),
                );
            });
        });
        ui.add_space(COL_GAP);
        ui.allocate_ui(egui::vec2(PORT_WIDTH, FIELD_HEIGHT), |ui| {
            ui.vertical(|ui| {
                ui.label(RichText::new("Port").strong().color(FG_PRIMARY))
                    .on_hover_text("Optional SSH server port; defaults to 22");
                ui.add(
                    egui::TextEdit::singleline(&mut state.port)
                        .hint_text("22")
                        .desired_width(PORT_WIDTH),
                );
            });
        });
    });
    if field_into_port(&state.port).is_err() {
        ui.label(
            RichText::new("Port must be an integer from 1 to 65535.")
                .small()
                .color(FG_ERROR),
        );
    }

    // ── test connection ───────────────────────────────────────────────────
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        let can_test = !state.host.trim().is_empty();
        let running = matches!(state.test_status, TestStatus::Running);
        let pressed = if running {
            ui.add_enabled(false, egui::Button::new("🔌  testing…"))
                .clicked()
        } else {
            ui.add_enabled(can_test, egui::Button::new("🔌  test connection"))
                .clicked()
        };
        if pressed {
            spawn_test(state);
        }
        match &state.test_status {
            TestStatus::Idle => {}
            TestStatus::Running => {
                ui.label(RichText::new("testing…").small().color(FG_WARNING));
            }
            TestStatus::Done { ok, message } => {
                let (icon, color) = if *ok {
                    ("✓", FG_SUCCESS)
                } else {
                    ("✗", FG_ERROR)
                };
                ui.label(
                    RichText::new(format!("{icon} {message}"))
                        .small()
                        .color(color),
                );
            }
        }
        ui.label(
            RichText::new("tests the configured authentication method")
                .small()
                .color(FG_DIM),
        );
    });

    // ── forwarded ports ───────────────────────────────────────────────────
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new("Forwarded ports").strong().color(FG_PRIMARY));
        ui.label(
            RichText::new(format!("{} configured", state.forwards.len()))
                .small()
                .color(FG_MUTED),
        );
    });
    let list_height =
        (ui.ctx().content_rect().bottom() - ui.cursor().top() - 70.0).clamp(100.0, 350.0);
    egui::ScrollArea::vertical()
        .max_height(list_height)
        .show(ui, |ui| {
            ui.group(|ui| {
                if state.forwards.is_empty() {
                    ui.label(
                RichText::new(
                    "no ports yet — fill the row below; it will be included when you press Apply",
                )
                .small()
                .color(FG_DIM),
            );
                }
                state.checked.resize(state.forwards.len(), false);

                // ── existing forwards ───────────────────────────────────
                let mut delete_forward = None;
                for i in 0..state.forwards.len() {
                    ui.horizontal(|ui| {
                        // col 1: N. + delete checkbox + enabled status
                        ui.allocate_ui(egui::vec2(FORWARD_NUM_W, FORWARD_ROW_H), |ui| {
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 3.0;
                                forward_number(ui, i + 1);
                                let mut checked = *state.checked.get(i).unwrap_or(&false);
                                if selection_checkbox(ui, &mut checked).changed() {
                                    if let Some(slot) = state.checked.get_mut(i) {
                                        *slot = checked;
                                    }
                                }
                                enabled_dot(ui, &mut state.forwards[i].enabled);
                            });
                        });
                        // col 2: mode combo
                        ui.allocate_ui(egui::vec2(FORWARD_MODE_W, FORWARD_ROW_H), |ui| {
                            egui::ComboBox::from_id_salt(("add_mode", i))
                                .selected_text(forward_mode_label(state.forwards[i].mode))
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(
                                        &mut state.forwards[i].mode,
                                        ForwardMode::Local,
                                        forward_mode_label(ForwardMode::Local),
                                    );
                                    ui.selectable_value(
                                        &mut state.forwards[i].mode,
                                        ForwardMode::Remote,
                                        forward_mode_label(ForwardMode::Remote),
                                    );
                                    ui.selectable_value(
                                        &mut state.forwards[i].mode,
                                        ForwardMode::Dynamic,
                                        forward_mode_label(ForwardMode::Dynamic),
                                    );
                                });
                        });
                        ui.add_space(COL_GAP);
                        // col 3: forward spec
                        let spec_hint = forward_hint(state.forwards[i].mode);
                        ui.allocate_ui(egui::vec2(FORWARD_SPEC_W, FORWARD_ROW_H), |ui| {
                            ui.add(
                                egui::TextEdit::singleline(&mut state.forwards[i].forward)
                                    .hint_text(spec_hint),
                            );
                        });
                        ui.add_space(COL_GAP);
                        // col 4: description
                        ui.allocate_ui(egui::vec2(FORWARD_DESC_W, FORWARD_ROW_H), |ui| {
                            ui.add(
                                egui::TextEdit::singleline(&mut state.forwards[i].description)
                                    .hint_text("optional note"),
                            );
                        });
                        if delete_forward_button(ui) {
                            delete_forward = Some(i);
                        }
                    });
                }
                if let Some(i) = delete_forward {
                    state.forwards.remove(i);
                    state.checked.remove(i);
                }

                // ── delete checked ───────────────────────────────────────
                let n_checked = state.checked.iter().filter(|&&c| c).count();
                if n_checked > 0 {
                    if ui
                        .button(
                            RichText::new(format!("🗑  delete checked ({n_checked})"))
                                .color(FG_ERROR),
                        )
                        .clicked()
                    {
                        let mut i = state.forwards.len();
                        while i > 0 {
                            i -= 1;
                            if i < state.checked.len() && state.checked[i] {
                                state.forwards.remove(i);
                                state.checked.remove(i);
                            }
                        }
                    }
                }

                // ── draft row (same columns and spacing as existing rows) ──
                let mut submit_draft = false;
                ui.horizontal(|ui| {
                    // col 1: N+1 + delete checkbox + enabled status
                    ui.allocate_ui(egui::vec2(FORWARD_NUM_W, FORWARD_ROW_H), |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 3.0;
                            forward_number(ui, state.forwards.len() + 1);
                            selection_checkbox(ui, &mut state.draft_checked);
                            enabled_dot(ui, &mut state.draft_enabled);
                        });
                    });
                    // col 2: mode combo
                    ui.allocate_ui(egui::vec2(FORWARD_MODE_W, FORWARD_ROW_H), |ui| {
                        egui::ComboBox::from_id_salt("draft_mode")
                            .selected_text(forward_mode_label(state.draft_mode))
                            .show_ui(ui, |ui| {
                                ui.selectable_value(
                                    &mut state.draft_mode,
                                    ForwardMode::Local,
                                    forward_mode_label(ForwardMode::Local),
                                );
                                ui.selectable_value(
                                    &mut state.draft_mode,
                                    ForwardMode::Remote,
                                    forward_mode_label(ForwardMode::Remote),
                                );
                                ui.selectable_value(
                                    &mut state.draft_mode,
                                    ForwardMode::Dynamic,
                                    forward_mode_label(ForwardMode::Dynamic),
                                );
                            });
                    });
                    ui.add_space(COL_GAP);
                    // col 3: forward spec
                    ui.allocate_ui(egui::vec2(FORWARD_SPEC_W, FORWARD_ROW_H), |ui| {
                        let response = ui.add(
                            egui::TextEdit::singleline(&mut state.draft_forward)
                                .hint_text(forward_hint(state.draft_mode)),
                        );
                        submit_draft |= (response.has_focus() || response.lost_focus())
                            && ui.input(|input| input.key_pressed(egui::Key::Enter));
                    });
                    ui.add_space(COL_GAP);
                    // col 4: description; full width keeps the draft row aligned
                    // with every configured-forward row above it.
                    ui.allocate_ui(egui::vec2(FORWARD_DESC_W, FORWARD_ROW_H), |ui| {
                        let response = ui.add(
                            egui::TextEdit::singleline(&mut state.draft_desc)
                                .hint_text("optional note"),
                        );
                        submit_draft |= (response.has_focus() || response.lost_focus())
                            && ui.input(|input| input.key_pressed(egui::Key::Enter));
                    });
                });
                if submit_draft {
                    push_draft_forward(state);
                }
            });
        });

    // ── footer ─────────────────────────────────────────────────────────────
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        let can_commit = !name_taken
            && !state.name.trim().is_empty()
            && !state.host.trim().is_empty()
            && field_into_port(&state.port).is_ok()
            && (!state.forwards.is_empty() || !state.draft_forward.trim().is_empty());
        if ui
            .add_enabled(
                can_commit,
                egui::Button::new(RichText::new("✔  Apply").strong()),
            )
            .clicked()
        {
            push_draft_forward(state);
            state.close = CloseAction::Commit;
        }
        if ui.button("cancel").clicked() {
            state.close = CloseAction::Cancel("cancelled");
        }
    });
}

// ─── dialog UI: edit global value ─────────────────────────────────────────────

pub fn run_edit_dialog_ui(ui: &mut egui::Ui, state: &mut EditDialogState) {
    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        state.close = CloseAction::Cancel("cancelled");
    }

    ui.add_space(4.0);
    ui.label(
        RichText::new(state.group.label())
            .strong()
            .color(FG_PRIMARY),
    );
    let resp = ui.add(egui::TextEdit::singleline(&mut state.value).desired_width(f32::INFINITY));
    // egui 0.29 singleline surrenders focus on Enter but consumes the event.
    let submitted = resp.lost_focus();
    ui.add_space(8.0);
    ui.label(
        RichText::new("Applied value is broadcast to every connection on save.")
            .small()
            .color(FG_MUTED),
    );
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        if ui.button("✔  Apply").clicked() || submitted {
            if state.value.trim().parse::<u64>().is_ok() {
                state.close = CloseAction::Commit;
            } else {
                state.close = CloseAction::Cancel("not a non-negative integer");
            }
        }
        if ui.button("cancel").clicked() {
            state.close = CloseAction::Cancel("cancelled");
        }
    });
}

// ─── dialog UI: import from TOML ──────────────────────────────────────────────

pub fn run_import_dialog_ui(ui: &mut egui::Ui, state: &mut ImportDialogState, existing: &[String]) {
    // Esc closes the dialog regardless of its current state (loading, failed, populated).
    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        state.close = CloseAction::Cancel("cancelled");
    }

    // ── header: path + load
    ui.add_space(4.0);
    ui.label(RichText::new("Source TOML file").strong().color(FG_PRIMARY));
    let resp = ui.add(
        egui::TextEdit::singleline(&mut state.path_input)
            .hint_text("~/path/to/another-autossh-config.toml")
            .desired_width(f32::INFINITY),
    );
    // egui 0.29 singleline surrenders focus on Enter but consumes the event.
    let submitted = resp.lost_focus();
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        if ui.button("📂  Load").clicked() || submitted {
            state.try_load(existing);
        }
    });
    ui.add_space(4.0);
    match &state.status {
        ImportStatus::Idle => {
            ui.label(RichText::new("(no source loaded)").small().color(FG_DIM));
        }
        ImportStatus::Loaded(path) => {
            ui.label(
                RichText::new(format!("✓ loaded {}", path.display()))
                    .small()
                    .color(FG_SUCCESS),
            );
        }
        ImportStatus::Failed(message) => {
            ui.colored_label(FG_ERROR, message);
        }
    }
    ui.add_space(8.0);
    ui.label(
        RichText::new(format!(
            "{} candidate connection(s)",
            state.candidates.len()
        ))
        .small()
        .color(FG_MUTED),
    );
    ui.separator();

    // ── body: candidates or empty/loading/error placeholder
    if state.candidates.is_empty() {
        let placeholder_text = match &state.status {
            ImportStatus::Failed(_) => "load failed — fix the path or press Esc / cancel to close",
            ImportStatus::Idle => "load a TOML file above to list candidates",
            ImportStatus::Loaded(_) => "the file had no [[connections]] blocks",
        };
        ui.add_space(40.0);
        ui.vertical_centered(|ui| {
            ui.label(RichText::new(placeholder_text).small().color(FG_DIM));
        });
    } else {
        pick_list(ui, &mut state.candidates, |c, ui| {
            ui.vertical(|ui| {
                ui.label(RichText::new(&c.name).strong());
                ui.label(
                    RichText::new(format!("{}  ·  {} forwards", c.host, c.forwards.len()))
                        .small()
                        .color(FG_MUTED),
                );
            });
        });
    }

    // ── footer: import + cancel buttons (always visible, even on load failure)
    ui.add_space(8.0);
    ui.separator();
    let any_selectable = state.candidates.iter().any(|c| c.selected && !c.duplicate);
    ui.horizontal(|ui| {
        if ui
            .add_enabled(
                any_selectable,
                egui::Button::new(RichText::new("✔  Import selected").strong()),
            )
            .clicked()
        {
            state.close = CloseAction::Commit;
        }
        if ui.button("cancel").clicked() {
            state.close = CloseAction::Cancel("cancelled");
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(RichText::new("Esc to close").small().color(FG_DIM));
        });
    });
}

// ─── dialog UI: import from SSH config ────────────────────────────────────────

pub fn run_ssh_import_dialog_ui(
    ui: &mut egui::Ui,
    state: &mut SshImportState,
    existing: &[String],
) {
    // Esc closes the dialog whether the load succeeded, failed, or hasn't run.
    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        state.close = CloseAction::Cancel("cancelled");
    }

    ui.add_space(4.0);
    ui.label(
        RichText::new("Source OpenSSH config")
            .strong()
            .color(FG_PRIMARY),
    );
    ui.label(
        RichText::new(state.source_path.display().to_string())
            .monospace()
            .color(Color32::from_rgb(180, 200, 220)),
    );
    ui.add_space(6.0);
    if ui.button("📂  Load").clicked() {
        state.try_load(existing);
    }
    ui.add_space(4.0);
    match &state.status {
        SshImportStatus::Idle => {
            ui.label(RichText::new("(no source loaded)").small().color(FG_DIM));
        }
        SshImportStatus::Loaded => {
            ui.label(
                RichText::new(format!("✓ loaded {} host(s)", state.candidates.len()))
                    .small()
                    .color(FG_SUCCESS),
            );
        }
        SshImportStatus::Failed(message) => {
            ui.colored_label(FG_ERROR, message);
        }
    }

    // hint about the autossh-required placeholder
    ui.add_space(4.0);
    ui.label(
        RichText::new(
            "autossh requires at least one port per connection; imported hosts will get a placeholder R 10022:127.0.0.1:22 that you can edit later.",
        )
        .small()
        .color(FG_MUTED),
    );
    ui.separator();

    // body
    if state.candidates.is_empty() {
        let placeholder_text = match &state.status {
            SshImportStatus::Failed(_) => "load failed — press Esc / cancel to close",
            SshImportStatus::Idle => "press Load to read SSH host aliases",
            SshImportStatus::Loaded => "the file declared no usable Host blocks",
        };
        ui.add_space(20.0);
        ui.vertical_centered(|ui| {
            ui.label(RichText::new(placeholder_text).small().color(FG_DIM));
        });
    } else {
        pick_list(ui, &mut state.candidates, |c, ui| {
            ui.vertical(|ui| {
                ui.label(RichText::new(&c.alias).strong());
                ui.label(
                    RichText::new(format!("{}  ·  port {}", c.destination, c.port))
                        .small()
                        .color(FG_MUTED),
                );
            });
        });
    }

    // footer (always visible)
    ui.add_space(8.0);
    ui.separator();
    let any_selectable = state.candidates.iter().any(|c| c.selected && !c.duplicate);
    ui.horizontal(|ui| {
        if ui
            .add_enabled(
                any_selectable,
                egui::Button::new(RichText::new("✔  Import selected").strong()),
            )
            .clicked()
        {
            state.close = CloseAction::Commit;
        }
        if ui.button("cancel").clicked() {
            state.close = CloseAction::Cancel("cancelled");
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(RichText::new("Esc to close").small().color(FG_DIM));
        });
    });
}

// ─── import dialog logic ───────────────────────────────────────────────────────

impl ImportDialogState {
    /// Parse the user-supplied path, populate `candidates` and bump the status.
    /// Names that already exist in the current config are flagged as duplicates
    /// and unselected by default so the user can only import what's truly new.
    pub fn try_load(&mut self, existing: &[String]) {
        let trimmed = self.path_input.trim();
        if trimmed.is_empty() {
            self.status = ImportStatus::Failed("path is empty".into());
            self.candidates.clear();
            return;
        }
        let path = PathBuf::from(trimmed);
        match Config::load(&path) {
            Ok(config) => {
                self.candidates = config
                    .connections
                    .into_iter()
                    .map(|c| {
                        let duplicate = existing.iter().any(|n| n == &c.name);
                        let name = c.name.clone();
                        let host = c.destination().to_string();
                        CandidateConnection {
                            name,
                            host,
                            forwards: c.forwards,
                            selected: !duplicate,
                            duplicate,
                        }
                    })
                    .collect();
                self.status = ImportStatus::Loaded(path);
            }
            Err(error) => {
                self.candidates.clear();
                self.status = ImportStatus::Failed(format!("{error:#}"));
            }
        }
    }
}

impl SshImportState {
    pub fn try_load(&mut self, existing: &[String]) {
        match parse_ssh_config(&self.source_path) {
            Ok(candidates) => {
                let entries: Vec<SshHostEntry> = candidates
                    .into_iter()
                    .map(|mut entry| {
                        // 默认一个都不选，由用户通过 select all 或逐条勾选
                        entry.duplicate = existing.iter().any(|n| n == &entry.alias);
                        entry.selected = false;
                        entry
                    })
                    .collect();
                // Stable order: original parse order is preserved.
                if entries.is_empty() {
                    self.status = SshImportStatus::Loaded;
                } else {
                    self.candidates = entries;
                    self.status = SshImportStatus::Loaded;
                }
            }
            Err(error) => {
                self.candidates.clear();
                self.status = SshImportStatus::Failed(format!("{error:#}"));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use autossh_core::{ConnectionConfig, ForwardConfig, ForwardMode};

    fn make_connection(name: &str, host: &str, forward: &str) -> ConnectionConfig {
        ConnectionConfig {
            name: name.into(),
            description: None,
            host: Some(host.into()),
            user: None,
            password: None,
            port: None,
            enabled: true,
            ssh_path: None,
            extra_args: Vec::new(),
            forwards: vec![ForwardConfig {
                enabled: true,
                mode: ForwardMode::Local,
                forward: forward.into(),
                description: None,
            }],
        }
    }

    #[test]
    fn import_dialog_flags_duplicates_and_unselects_them() {
        let dir = std::env::temp_dir().join("autossh-ui-import-tests");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("source.toml");
        let _ = std::fs::remove_file(&path);
        let src = Config {
            log: Default::default(),
            keepalive: Default::default(),
            retry: Default::default(),
            connections: vec![
                make_connection("home", "c005", "8080:127.0.0.1:8080"),
                make_connection("backup", "c006", "10022:127.0.0.1:22"),
            ],
        };
        src.save(&path).unwrap();

        let mut dialog = ImportDialogState {
            path_input: path.display().to_string(),
            ..Default::default()
        };
        let existing = vec!["home".to_string()];
        dialog.try_load(&existing);

        assert_eq!(dialog.candidates.len(), 2);
        match &dialog.status {
            ImportStatus::Loaded(_) => {}
            _ => panic!("expected Loaded status, got {:?}", dialog.status),
        }
        let home = dialog
            .candidates
            .iter()
            .find(|c| c.name == "home")
            .expect("home should be present");
        assert!(home.duplicate, "home collides with existing");
        assert!(!home.selected, "duplicates default to unselected");
        let backup = dialog
            .candidates
            .iter()
            .find(|c| c.name == "backup")
            .expect("backup should be present");
        assert!(!backup.duplicate);
        assert!(backup.selected, "new candidates default to selected");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn import_dialog_surfaces_load_errors() {
        let mut dialog = ImportDialogState {
            path_input: "/nonexistent/path/__does_not_exist.toml".to_string(),
            ..Default::default()
        };
        dialog.try_load(&[]);
        match &dialog.status {
            ImportStatus::Failed(_) => {}
            other => panic!("expected Failed status, got {other:?}"),
        }
        assert!(dialog.candidates.is_empty());
    }

    #[test]
    fn parses_optional_ssh_port() {
        assert_eq!(field_into_port(""), Ok(None));
        assert_eq!(field_into_port("2202"), Ok(Some(2202)));
        assert!(field_into_port("0").is_err());
        assert!(field_into_port("not-a-port").is_err());
    }

    #[test]
    fn state_from_connection_round_trips_fields() {
        let mut conn = make_connection("home", "c005", "10022:127.0.0.1:22");
        conn.description = Some("home gateway".into());
        conn.port = Some(2202);
        conn.forwards[0].description = Some("home SSH".into());
        let state = state_from_connection(&conn);
        assert_eq!(state.name, "home");
        assert_eq!(state.description, "home gateway");
        assert_eq!(state.host, "c005");
        assert_eq!(state.port, "2202");
        assert_eq!(state.forwards.len(), 1);
        assert_eq!(state.forwards[0].forward, "10022:127.0.0.1:22");
        assert_eq!(state.forwards[0].mode, ForwardMode::Local);
        assert_eq!(
            state.forwards[0].description, "home SSH",
            "per-forward description must populate the editable row"
        );
        // fresh edit starts with no draft so the user is not surprised
        assert!(state.draft_forward.is_empty());
        assert!(state.draft_desc.is_empty());
    }

    #[test]
    fn editable_forward_into_forward_drops_empty_description() {
        // White-space-only and empty strings must round-trip to `None` so the
        // persisted TOML does not sprout a noisy `description = ""` line.
        let editable = EditableForward {
            enabled: false,
            mode: ForwardMode::Remote,
            forward: "10022:127.0.0.1:22".into(),
            description: "   ".into(),
        };
        let forward = editable.clone().into_forward();
        assert!(!forward.enabled);
        assert!(forward.description.is_none());
        let with_text = EditableForward {
            description: "home SSH".into(),
            ..editable
        };
        let forward = with_text.into_forward();
        assert_eq!(forward.description.as_deref(), Some("home SSH"));
    }

    #[test]
    fn push_draft_forward_preserves_status_and_prepares_next_row() {
        let mut state = AddDialogState {
            draft_forward: " 1080 ".into(),
            draft_desc: "SOCKS".into(),
            draft_enabled: false,
            draft_checked: true,
            ..Default::default()
        };
        push_draft_forward(&mut state);
        assert_eq!(state.forwards.len(), 1);
        assert_eq!(state.forwards[0].forward, "1080");
        assert_eq!(state.forwards[0].description, "SOCKS");
        assert!(!state.forwards[0].enabled);
        assert_eq!(state.checked, vec![true]);
        assert!(state.draft_forward.is_empty());
        assert!(state.draft_desc.is_empty());
        assert!(state.draft_enabled);
        assert!(!state.draft_checked);
    }

    #[test]
    fn add_dialog_state_defaults_to_idle_test_status() {
        let state = AddDialogState::default();
        assert!(matches!(state.test_status, TestStatus::Idle));
        assert!(
            state.test_channel.lock().unwrap().is_none(),
            "default channel must be empty"
        );
    }

    #[test]
    fn state_from_connection_round_trips_test_status() {
        let conn = make_connection("home", "c005", "10022:127.0.0.1:22");
        let state = state_from_connection(&conn);
        assert!(matches!(state.test_status, TestStatus::Idle));
        assert!(state.test_channel.lock().unwrap().is_none());
    }

    #[test]
    fn edit_connection_falls_back_to_name_when_host_missing() {
        // ssh-import style entries sometimes have host=None; falling back
        // to the connection name keeps the dialog usable.
        let conn = ConnectionConfig {
            name: "home".into(),
            description: None,
            host: None,
            user: None,
            password: None,
            port: None,
            enabled: true,
            ssh_path: None,
            extra_args: Vec::new(),
            forwards: vec![],
        };
        let state = state_from_connection(&conn);
        assert_eq!(state.host, "home");
    }
}
