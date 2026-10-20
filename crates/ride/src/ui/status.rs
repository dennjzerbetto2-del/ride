//! La barra di stato: messaggi a sinistra, informazioni sul documento a destra.

use std::time::Instant;

use egui::{Align, Color32, Layout, RichText, Ui};

use crate::action::Action;
use crate::app::RideApp;
use crate::lsp::{server_for, LspStatus, Severity};
use crate::state::Level;

/// Disegna la barra di stato.
pub fn bar(ui: &mut Ui, app: &RideApp, now: Instant, actions: &mut Vec<Action>) {
    ui.horizontal(|ui| {
        message(ui, app, now);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            info(ui, app, actions);
        });
    });
}

/// Il messaggio corrente (o il percorso del file attivo).
fn message(ui: &mut Ui, app: &RideApp, now: Instant) {
    if let Some((level, text)) = app.status.current(now) {
        let (icon, color) = match level {
            Level::Info => ("ℹ", Color32::from_rgb(0x8a, 0xb4, 0xe0)),
            Level::Warn => ("⚠", Color32::from_rgb(0xe0, 0xb8, 0x6a)),
            Level::Error => ("✖", Color32::from_rgb(0xe0, 0x7a, 0x7a)),
        };
        ui.colored_label(color, format!("{icon} {text}"));
        return;
    }
    match app.workspace.active() {
        Some(doc) => {
            let text = if doc.is_dirty() {
                format!("{} (modificato)", doc.display_path())
            } else {
                doc.display_path()
            };
            ui.label(RichText::new(text).weak());
        }
        None => {
            ui.label(RichText::new("Pronto").weak());
        }
    }
}

/// Le informazioni sul documento attivo, da destra verso sinistra.
fn info(ui: &mut Ui, app: &RideApp, actions: &mut Vec<Action>) {
    let id = app.workspace.active_id();
    let doc = app.workspace.active();
    let snapshot = app.editor.filter(|s| Some(s.doc) == id);

    // Posizione del cursore.
    let (line, column) = snapshot.map(|s| (s.line, s.column)).unwrap_or((app.cursor_line, 0));
    ui.label(RichText::new(format!("Riga {}, Col {}", line + 1, column + 1)).weak());

    if let Some(snapshot) = snapshot {
        let selected = snapshot.selection.1.saturating_sub(snapshot.selection.0);
        if selected > 0 {
            ui.separator();
            ui.label(RichText::new(format!("{selected} car.")).weak());
        }
    }

    // Terminale integrato: anche da nascosto resta raggiungibile da qui.
    if app.terminal.open || app.terminal.has_session() {
        ui.separator();
        let (text, color) = if app.terminal.open {
            ("⌨ terminale".to_string(), ui.visuals().weak_text_color())
        } else if app.terminal.is_running() {
            (
                "⌨ terminale (nascosto)".to_string(),
                Color32::from_rgb(0x8a, 0xb4, 0xe0),
            )
        } else {
            ("⌨ terminale (uscito)".to_string(), ui.visuals().weak_text_color())
        };
        let response = ui.add(
            egui::Label::new(RichText::new(text).monospace().color(color))
                .sense(egui::Sense::click())
                .selectable(false),
        );
        if response.clicked() {
            actions.push(Action::ToggleTerminal);
        }
        response
            .on_hover_text("Terminale integrato — clicca per mostrarlo o nasconderlo")
            .on_hover_cursor(egui::CursorIcon::PointingHand);
    }

    let Some(doc) = doc else {
        return;
    };

    // Terminatore di riga.
    ui.separator();
    ui.label(RichText::new(doc.eol().label()).weak().monospace())
        .on_hover_text("Terminatore di riga usato al salvataggio");

    // Linguaggio.
    ui.separator();
    ui.label(RichText::new(doc.language().name()).weak());

    // Formattazione: strumento esterno oppure, se manca, il language server.
    let language = doc.language();
    let external = ride_core::format::formatter_for(language);
    let external_ok = external.is_some_and(|_| ride_core::format::can_format(language));
    let warning = Color32::from_rgb(0xe0, 0xb8, 0x6a);

    if external_ok {
        let name = external.map(|spec| spec.program).unwrap_or("formatter");
        ui.separator();
        ui.label(RichText::new(format!("{name} ✓")).weak().monospace())
            .on_hover_text("Il formattatore esterno è nel PATH");
    } else if crate::lsp::server_can_format(language) {
        // Il server sa formattare: la mancanza del tool esterno non è un problema.
        let server = server_for(language).map(|c| c.program).unwrap_or("LSP");
        let installed = crate::lsp::LspManager::server_available(language);
        let ready = app.lsp.status() == LspStatus::Ready;
        ui.separator();
        let (text, color, hint) = if ready {
            (
                format!("{server} ✓ (LSP)"),
                ui.visuals().weak_text_color(),
                "La formattazione passerà dal language server".to_string(),
            )
        } else if installed {
            (
                format!("{server} (LSP)"),
                ui.visuals().weak_text_color(),
                "La formattazione passerà dal language server (non ancora pronto)".to_string(),
            )
        } else {
            (
                format!("{server} ✗"),
                warning,
                format!("{server} non è installato: serve per la formattazione"),
            )
        };
        ui.colored_label(color, RichText::new(text).monospace())
            .on_hover_text(hint);
    } else if let Some(spec) = external {
        ui.separator();
        ui.colored_label(
            warning,
            RichText::new(format!("{} ✗", spec.program)).monospace(),
        )
        .on_hover_text(format!(
            "{} non è installato ({})",
            spec.program, spec.hint
        ));
    }

    // Diagnostica del language server sul documento attivo.
    if let Some(id) = id {
        let diagnostics = app.lsp.diagnostics(id);
        if !diagnostics.is_empty() {
            let errors = diagnostics
                .iter()
                .filter(|d| d.severity == Severity::Error)
                .count();
            let warnings = diagnostics
                .iter()
                .filter(|d| d.severity == Severity::Warning)
                .count();
            let color = if errors > 0 {
                Color32::from_rgb(0xe0, 0x7a, 0x7a)
            } else if warnings > 0 {
                Color32::from_rgb(0xe0, 0xb8, 0x6a)
            } else {
                ui.visuals().weak_text_color()
            };
            ui.separator();
            let response = ui.add(
                egui::Label::new(
                    RichText::new(format!("✖ {errors}  ⚠ {warnings}"))
                        .monospace()
                        .color(color),
                )
                    .sense(egui::Sense::click())
                    .selectable(false),
            );
            if response.clicked() {
                actions.push(Action::ShowProblems);
            }
            response
                .on_hover_text("Errori e avvisi segnalati dal language server — clicca per vederli")
                .on_hover_cursor(egui::CursorIcon::PointingHand);
        }
    }

    // Language server.
    ui.separator();
    let lsp_label = match server_for(language) {
        Some(config) => {
            let installed = crate::lsp::LspManager::server_available(language);
            match app.lsp.status() {
                LspStatus::Starting => format!("LSP: {} avvio…", config.name),
                LspStatus::Ready => format!("LSP: {} pronto", config.name),
                LspStatus::Failed => "LSP: errore".to_string(),
                LspStatus::Off if installed => format!("LSP: {} pronto", config.name),
                LspStatus::Off => format!("LSP: {} mancante", config.name),
            }
        }
        None => "LSP: non disponibile".to_string(),
    };
    ui.label(RichText::new(lsp_label).weak().monospace())
        .on_hover_text(app.lsp.error().unwrap_or("Nessun errore del language server"));

    // Build in corso o conclusa: cliccabile per aprire la console.
    if app.build.running || !app.build.lines.is_empty() {
        ui.separator();
        let (text, color) = if app.build.running {
            ("⚙ build in corso".to_string(), Color32::from_rgb(0x8a, 0xb4, 0xe0))
        } else if app.build.last == Some(false) {
            ("⚙ build fallita".to_string(), Color32::from_rgb(0xe0, 0x7a, 0x7a))
        } else {
            ("⚙ build".to_string(), ui.visuals().weak_text_color())
        };
        let response = ui.add(
            egui::Label::new(RichText::new(text).monospace().color(color))
                .sense(egui::Sense::click())
                .selectable(false),
        );
        if response.clicked() {
            actions.push(Action::ShowBuildConsole);
        }
        response
            .on_hover_text("Console dei comandi di build — clicca per aprirla")
            .on_hover_cursor(egui::CursorIcon::PointingHand);
    }

    // Stato di modifica / sola lettura.
    if doc.is_read_only() {
        ui.separator();
        ui.colored_label(
            Color32::from_rgb(0xe0, 0xb8, 0x6a),
            RichText::new("sola lettura").monospace(),
        );
    } else if doc.is_dirty() {
        ui.separator();
        ui.colored_label(
            Color32::from_rgb(0xd8, 0xd0, 0x60),
            RichText::new("● non salvato").monospace(),
        );
    }

    // Progetto.
    if let Some(project) = app.workspace.project() {
        ui.separator();
        ui.label(RichText::new(format!("📁 {}", project.name())).weak());
    }
}
