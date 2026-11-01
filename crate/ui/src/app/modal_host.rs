//! Modal dispatch: route the current `Modal` variant into the correct
//! egui window, then fold its `CloseAction` back into either a kept-open
//! state or a commit/cancel against the config.

use std::collections::HashSet;

use autossh_core::ConnectionConfig;
use eframe::egui;

use crate::modal::{
    AddDialogState, CloseAction, EditDialogState, EditableForward, GlobalGroup, Modal,
    field_into_option, field_into_port, run_add_dialog_ui, run_edit_dialog_ui,
    run_import_dialog_ui, run_ssh_import_dialog_ui,
};

use super::AutosshApp;

impl AutosshApp {
    pub fn render_modal(&mut self, ctx: &egui::Context) {
        let modal = std::mem::replace(&mut self.modal, Modal::None);
        match modal {
            Modal::None => {}
            Modal::Add(state) => {
                let mut state = state;
                let name_taken = self
                    .config
                    .connections
                    .iter()
                    .any(|connection| connection.name.trim() == state.name.trim());
                egui::Window::new("Add connection")
                    .collapsible(false)
                    .resizable(false)
                    .default_size([620.0, 430.0])
                    .show(ctx, |ui| {
                        run_add_dialog_ui(ui, &mut state, name_taken);
                    });
                self.apply_add_dialog_state(state);
            }
            Modal::EditConnection { idx, state } => {
                let mut state = state;
                let name = self
                    .config
                    .connections
                    .get(idx)
                    .map(|c| c.name.clone())
                    .unwrap_or_default();
                let name_taken =
                    self.config
                        .connections
                        .iter()
                        .enumerate()
                        .any(|(other_idx, connection)| {
                            other_idx != idx && connection.name.trim() == state.name.trim()
                        });
                egui::Window::new(format!("Edit connection ({} - {})", idx + 1, name))
                    .collapsible(false)
                    .resizable(false)
                    .default_size([620.0, 430.0])
                    .show(ctx, |ui| {
                        run_add_dialog_ui(ui, &mut state, name_taken);
                    });
                self.apply_edit_connection_state(idx, state);
            }
            Modal::EditGlobal { group, value } => {
                let mut state = EditDialogState {
                    group,
                    value,
                    close: CloseAction::None,
                };
                egui::Window::new(format!("Edit {}", group.label()))
                    .collapsible(false)
                    .resizable(false)
                    .default_size([380.0, 160.0])
                    .show(ctx, |ui| {
                        run_edit_dialog_ui(ui, &mut state);
                    });
                self.apply_edit_dialog_state(state);
            }
            Modal::Import(mut state) => {
                let existing: Vec<String> = self
                    .config
                    .connections
                    .iter()
                    .map(|c| c.name.clone())
                    .collect();
                egui::Window::new("Import hosts from a TOML config")
                    .collapsible(false)
                    .resizable(true)
                    .default_size([520.0, 440.0])
                    .show(ctx, |ui| {
                        run_import_dialog_ui(ui, &mut state, &existing);
                    });
                self.apply_import_dialog_state(state);
            }
            Modal::ImportSsh(mut state) => {
                let existing: Vec<String> = self
                    .config
                    .connections
                    .iter()
                    .map(|c| c.name.clone())
                    .collect();
                egui::Window::new("Import SSH hosts from ~/.ssh/config")
                    .collapsible(false)
                    .resizable(true)
                    .default_size([520.0, 440.0])
                    .show(ctx, |ui| {
                        run_ssh_import_dialog_ui(ui, &mut state, &existing);
                    });
                self.apply_ssh_import_dialog_state(state);
            }
        }
    }

    fn apply_add_dialog_state(&mut self, state: AddDialogState) {
        match state.close {
            CloseAction::None => {
                self.modal = Modal::Add(state);
            }
            CloseAction::Commit => {
                let name = state.name.trim().to_string();
                if self
                    .config
                    .connections
                    .iter()
                    .any(|connection| connection.name.trim() == name)
                {
                    self.flash(format!("connection name {name:?} already exists"));
                    self.modal = Modal::Add(state);
                    return;
                }
                let port = match field_into_port(&state.port) {
                    Ok(port) => port,
                    Err(message) => {
                        self.flash(message);
                        self.modal = Modal::Add(state);
                        return;
                    }
                };
                let forwards: Vec<_> = state
                    .forwards
                    .iter()
                    .cloned()
                    .map(EditableForward::into_forward)
                    .collect();
                self.config.connections.push(ConnectionConfig {
                    name: name.clone(),
                    description: field_into_option(&state.description),
                    host: Some(state.host.trim().to_string()),
                    user: field_into_option(&state.user),
                    password: field_into_option(&state.password),
                    port,
                    enabled: true,
                    ssh_path: None,
                    extra_args: Vec::new(),
                    forwards,
                });
                self.dirty = true;
                self.selected_connection = self.config.connections.len() - 1;
                self.flash(format!("added {name}"));
            }
            CloseAction::Cancel(message) => {
                self.flash(message);
            }
        }
    }

    /// Same dialog as Add, but commits overwrite `connections[idx]` instead
    /// of appending. `idx` is captured at modal-open time; if the user
    /// cancels or merely moves entries around, the row is left intact.
    fn apply_edit_connection_state(&mut self, idx: usize, state: AddDialogState) {
        match state.close {
            CloseAction::None => {
                // still editing
                self.modal = Modal::EditConnection { idx, state };
            }
            CloseAction::Commit => {
                if idx >= self.config.connections.len() {
                    // the connection was deleted while the dialog was open.
                    self.flash("connection vanished; edit discarded");
                    return;
                }
                let name = state.name.trim().to_string();
                if self
                    .config
                    .connections
                    .iter()
                    .enumerate()
                    .any(|(other_idx, connection)| {
                        other_idx != idx && connection.name.trim() == name
                    })
                {
                    self.flash(format!("connection name {name:?} already exists"));
                    self.modal = Modal::EditConnection { idx, state };
                    return;
                }
                let port = match field_into_port(&state.port) {
                    Ok(port) => port,
                    Err(message) => {
                        self.flash(message);
                        self.modal = Modal::EditConnection { idx, state };
                        return;
                    }
                };
                let forwards: Vec<_> = state
                    .forwards
                    .iter()
                    .cloned()
                    .map(EditableForward::into_forward)
                    .collect();
                let conn = &mut self.config.connections[idx];
                conn.name = name.clone();
                conn.description = field_into_option(&state.description);
                conn.host = Some(state.host.trim().to_string());
                conn.user = field_into_option(&state.user);
                conn.password = field_into_option(&state.password);
                conn.port = port;
                conn.forwards = forwards;
                self.dirty = true;
                self.flash(format!("updated {name}"));
            }
            CloseAction::Cancel(message) => {
                self.flash(message);
            }
        }
    }

    fn apply_edit_dialog_state(&mut self, state: EditDialogState) {
        match state.close {
            CloseAction::None => {
                self.modal = Modal::EditGlobal {
                    group: state.group,
                    value: state.value,
                };
            }
            CloseAction::Commit => {
                let Ok(n) = state.value.trim().parse::<u64>() else {
                    self.flash("not a non-negative integer; cancelled");
                    return;
                };
                let mut ka = self.keepalive();
                let mut r = self.retry();
                match state.group {
                    GlobalGroup::KeepaliveInterval => ka.interval = n,
                    GlobalGroup::KeepaliveCount => ka.count_max = n as u32,
                    GlobalGroup::KeepaliveTimeout => ka.connect_timeout = n,
                    GlobalGroup::RetryInitial => r.initial_seconds = n,
                    GlobalGroup::RetryMaximum => r.maximum_seconds = n,
                    GlobalGroup::RetryStable => r.stable_seconds = n,
                }
                self.apply_globals(&ka, &r);
                self.dirty = true;
                self.flash(format!("{} → {n}", state.group.label()));
            }
            CloseAction::Cancel(message) => {
                self.flash(message);
            }
        }
    }

    fn apply_import_dialog_state(&mut self, state: crate::modal::ImportDialogState) {
        match state.close {
            CloseAction::None => {
                self.modal = Modal::Import(state);
            }
            CloseAction::Commit => {
                let existing: HashSet<String> = self
                    .config
                    .connections
                    .iter()
                    .map(|c| c.name.clone())
                    .collect();
                let mut imported = 0usize;
                let mut skipped = 0usize;
                let mut next_selected = self.selected_connection;
                for cand in state.candidates {
                    if !cand.selected {
                        continue;
                    }
                    if existing.contains(&cand.name) {
                        skipped += 1;
                        continue;
                    }
                    self.config.connections.push(ConnectionConfig {
                        name: cand.name.clone(),
                        description: None,
                        host: Some(cand.host),
                        user: None,
                        password: None,
                        port: None,
                        enabled: true,
                        ssh_path: None,
                        extra_args: Vec::new(),
                        forwards: cand.forwards,
                    });
                    imported += 1;
                    next_selected = self.config.connections.len() - 1;
                }
                if imported > 0 {
                    self.dirty = true;
                    self.selected_connection = next_selected;
                    self.flash(format!("imported {imported} connection(s)"));
                }
                if skipped > 0 {
                    self.flash(format!("skipped {skipped} duplicate name(s)"));
                }
                if imported == 0 && skipped == 0 {
                    self.flash("nothing selected to import");
                }
            }
            CloseAction::Cancel(_) => {}
        }
    }

    fn apply_ssh_import_dialog_state(&mut self, state: crate::modal::SshImportState) {
        match state.close {
            CloseAction::None => {
                self.modal = Modal::ImportSsh(state);
            }
            CloseAction::Commit => {
                let existing: HashSet<String> = self
                    .config
                    .connections
                    .iter()
                    .map(|c| c.name.clone())
                    .collect();
                let mut imported = 0usize;
                let mut skipped = 0usize;
                let mut next_selected = self.selected_connection;
                for cand in state.candidates {
                    if !cand.selected || cand.duplicate {
                        if cand.duplicate {
                            skipped += 1;
                        }
                        continue;
                    }
                    if existing.contains(&cand.alias) {
                        skipped += 1;
                        continue;
                    }
                    // SSH config has no forwards; seed a placeholder reverse
                    // tunnel so the config validates, then the user can edit.
                    let placeholder = autossh_core::ForwardConfig {
                        enabled: true,
                        mode: autossh_core::ForwardMode::Remote,
                        forward: "10022:127.0.0.1:22".to_string(),
                        description: None,
                    };
                    self.config.connections.push(ConnectionConfig {
                        name: cand.alias.clone(),
                        description: None,
                        host: Some(cand.destination.clone()),
                        user: None,
                        password: None,
                        port: None,
                        enabled: true,
                        ssh_path: None,
                        extra_args: Vec::new(),
                        forwards: vec![placeholder],
                    });
                    imported += 1;
                    next_selected = self.config.connections.len() - 1;
                }
                if imported > 0 {
                    self.dirty = true;
                    self.selected_connection = next_selected;
                    self.flash(format!(
                        "imported {imported} SSH host(s) (placeholder 10022 reverse tunnel added)",
                    ));
                }
                if skipped > 0 {
                    self.flash(format!("skipped {skipped} duplicate alias(es)"));
                }
                if imported == 0 && skipped == 0 {
                    self.flash("nothing selected to import");
                }
            }
            CloseAction::Cancel(_) => {}
        }
    }
}
