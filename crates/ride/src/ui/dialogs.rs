//! I dialoghi sovrapposti: prompt di percorso, conferme, scorciatoie, LSP,
//! "vai al file".
//!
//! Tutto è disegnato con `egui::Window`. La funzione [`windows`] restituisce,
//! se c'è, il [`Prompt`] confermato: il compito di applicarlo (scrivere su
//! disco, aprire…) resta a [`crate::app::RideApp`], che è l'unico a toccare il
//! modello.

use std::path::PathBuf;

use egui::{Align, Align2, Color32, Context, Grid, Key, Layout, RichText, ScrollArea, TextEdit, Window};

use crate::action::{Action, CloseChoice, SHORTCUTS};
use crate::app::RideApp;
use crate::lsp::{server_for, LspManager, Severity};
use crate::state::Prompt;

/// Disegna tutte le finestre di dialogo aperte.
///
/// Restituisce il [`Prompt`] confermato dall'utente, che il chiamante deve
/// applicare con `RideApp::apply_prompt`.
pub fn windows(ctx: &Context, app: &mut RideApp, actions: &mut Vec<Action>) -> Option<Prompt> {
    let prompt = prompt_window(ctx, app);
    confirm_close_window(ctx, app, actions);
    confirm_delete_window(ctx, app, actions);
    goto_line_window(ctx, app, actions);
    shortcuts_window(ctx, app);
    about_window(ctx, app);
    lsp_window(ctx, app);
    problems_window(ctx, app, actions);
    build_window(ctx, app, actions);
    quick_open_window(ctx, app, actions);
    prompt
}

/// Dialogo che chiede un percorso o un nome.
fn prompt_window(ctx: &Context, app: &mut RideApp) -> Option<Prompt> {
    enum Outcome {
        Confirm(Prompt),
        Cancel,
    }
    let mut outcome: Option<Outcome> = None;

    if let Some(prompt) = app.dialogs.prompt.as_mut() {
        let title = prompt.title.clone();
        let hint = prompt.hint.clone();
        let placeholder = prompt.placeholder.clone();
        Window::new(title)
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(hint);
                ui.add_space(4.0);
                let response = ui.add(
                    TextEdit::singleline(&mut prompt.value)
                        .desired_width(460.0)
                        .hint_text(placeholder),
                );
                if prompt.focus {
                    response.request_focus();
                    prompt.focus = false;
                }
                let confirmed = response.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button("Annulla").clicked() {
                        outcome = Some(Outcome::Cancel);
                    }
                    if ui.button("Conferma").clicked() || confirmed {
                        outcome = Some(Outcome::Confirm(prompt.clone()));
                    }
                });
            });
    }

    match outcome {
        Some(Outcome::Confirm(prompt)) => {
            app.dialogs.prompt = None;
            Some(prompt)
        }
        Some(Outcome::Cancel) => {
            app.dialogs.prompt = None;
            None
        }
        None => None,
    }
}

/// Conferma di chiusura per i documenti con modifiche non salvate.
fn confirm_close_window(ctx: &Context, app: &mut RideApp, actions: &mut Vec<Action>) {
    let Some(request) = app.dialogs.confirm_close.clone() else {
        return;
    };
    let names: Vec<String> = request
        .docs
        .iter()
        .filter_map(|id| app.workspace.get(*id))
        .map(|doc| doc.display_name().to_string())
        .collect();
    let title = if request.quit {
        "Uscire da Ride?"
    } else if request.close_project.is_some() {
        "Chiudere il progetto?"
    } else if names.len() == 1 {
        "Chiudere il file?"
    } else {
        "Chiudere i file?"
    };

    let mut choice: Option<bool> = None; // true = salva, false = scarta
    let mut cancel = false;
    Window::new(title)
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.label(format!(
                "Ci sono {} documenti con modifiche non salvate:",
                names.len()
            ));
            ui.add_space(4.0);
            for name in &names {
                ui.label(RichText::new(name).monospace());
            }
            if request.close_project.is_some() {
                ui.add_space(4.0);
                ui.label(RichText::new("Con il progetto si chiudono tutte le sue tab.").weak());
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Salva e chiudi").clicked() {
                    choice = Some(true);
                }
                if ui.button("Non salvare").clicked() {
                    choice = Some(false);
                }
                if ui.button("Annulla").clicked() {
                    cancel = true;
                }
            });
        });

    if let Some(save) = choice {
        actions.push(Action::ResolveClose(CloseChoice {
            docs: request.docs.clone(),
            quit: request.quit,
            save,
            close_project: request.close_project.clone(),
        }));
        app.dialogs.confirm_close = None;
    } else if cancel {
        app.dialogs.confirm_close = None;
    }
}

/// Conferma di eliminazione di un file o di una cartella.
fn confirm_delete_window(ctx: &Context, app: &mut RideApp, actions: &mut Vec<Action>) {
    let Some(path) = app.dialogs.confirm_delete.clone() else {
        return;
    };
    let is_dir = path.is_dir();
    let mut delete = false;
    let mut cancel = false;

    Window::new("Conferma eliminazione")
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.label(format!(
                "Eliminare {}?",
                if is_dir {
                    format!("la cartella «{}»", path.display())
                } else {
                    format!("il file «{}»", path.display())
                }
            ));
            if is_dir {
                ui.colored_label(
                    Color32::from_rgb(230, 120, 120),
                    "Tutto il contenuto verrà rimosso.",
                );
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Elimina").clicked() {
                    delete = true;
                }
                if ui.button("Annulla").clicked() {
                    cancel = true;
                }
            });
        });

    if delete {
        actions.push(Action::DeletePath(path));
        app.dialogs.confirm_delete = None;
    } else if cancel {
        app.dialogs.confirm_delete = None;
    }
}

/// Dialogo "vai alla riga".
fn goto_line_window(ctx: &Context, app: &mut RideApp, actions: &mut Vec<Action>) {
    if !app.dialogs.goto_line {
        return;
    }
    let mut confirm = false;
    let mut cancel = false;
    {
        let value = &mut app.dialogs.goto_line_value;
        Window::new("Vai alla riga")
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label("Numero di riga (1 = prima riga):");
                let response = ui.add(TextEdit::singleline(value).desired_width(120.0));
                response.request_focus();
                let enter = ui.input(|i| i.key_pressed(Key::Enter));
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button("Vai").clicked() || enter {
                        confirm = true;
                    }
                    if ui.button("Annulla").clicked() {
                        cancel = true;
                    }
                });
            });
    }

    if confirm {
        let line = app
            .dialogs
            .goto_line_value
            .trim()
            .parse::<usize>()
            .unwrap_or(1)
            .max(1);
        actions.push(Action::GotoLineAt(line));
        app.dialogs.goto_line = false;
    } else if cancel {
        app.dialogs.goto_line = false;
    }
}

/// Elenco delle scorciatoie (F1).
fn shortcuts_window(ctx: &Context, app: &mut RideApp) {
    if !app.dialogs.shortcuts {
        return;
    }
    let mut open = true;
    Window::new("Scorciatoie da tastiera")
        .open(&mut open)
        .collapsible(false)
        .resizable(true)
        .default_size([520.0, 520.0])
        .show(ctx, |ui| {
            ScrollArea::vertical().show(ui, |ui| {
                Grid::new("ride-shortcuts")
                    .num_columns(2)
                    .striped(true)
                    .spacing([18.0, 6.0])
                    .show(ui, |ui| {
                        for shortcut in SHORTCUTS {
                            ui.label(RichText::new(shortcut.keys).monospace().strong());
                            ui.label(shortcut.description);
                            ui.end_row();
                        }
                    });
            });
        });
    if !open {
        app.dialogs.shortcuts = false;
    }
}

/// Informazioni su Ride.
fn about_window(ctx: &Context, app: &mut RideApp) {
    if !app.dialogs.about {
        return;
    }
    let mut open = true;
    Window::new("Informazioni su Ride")
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.label(RichText::new("Ride").heading());
            ui.label(format!("versione {}", env!("CARGO_PKG_VERSION")));
            ui.add_space(6.0);
            ui.label("Editor di codice multi-file con gestione di progetti.");
            ui.label("Interfaccia: egui / eframe. Evidenziazione: syntect.");
            ui.add_space(6.0);
            ui.label(RichText::new("Premi F1 per l'elenco delle scorciatoie.").weak());
        });
    if !open {
        app.dialogs.about = false;
    }
}

/// Stato dell'integrazione LSP.
fn lsp_window(ctx: &Context, app: &mut RideApp) {
    if !app.dialogs.lsp_info {
        return;
    }
    let language = app.workspace.active().map(|doc| doc.language());
    let server = language.and_then(server_for);
    let installed = language.is_some_and(LspManager::server_available);
    let mut open = true;
    Window::new("Language server (LSP)")
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .default_size([500.0, 260.0])
        .show(ctx, |ui| {
            ui.label(format!("Stato: {}", app.lsp.status().label()));
            if let Some(error) = app.lsp.error() {
                ui.colored_label(Color32::from_rgb(230, 120, 120), error);
            }
            ui.separator();
            match (server, language) {
                (Some(config), Some(language)) => {
                    ui.label(format!("Server per {}: {}", language.name(), config.name));
                    ui.label(
                        RichText::new(format!(
                            "Comando: {} {}",
                            config.program,
                            config.args.join(" ")
                        ))
                        .monospace(),
                    );
                    ui.label(if installed {
                        "Installato ✓"
                    } else {
                        "Non installato ✗"
                    });
                    if let Some(root) = app.lsp.project_root() {
                        ui.label(
                            RichText::new(format!("Cartella: {}", root.display())).weak(),
                        );
                    }
                }
                _ => {
                    ui.label("Nessun language server noto per il linguaggio attivo.");
                }
            }
            ui.separator();
            ui.label(RichText::new("Diagnostica del documento attivo").strong());
            match app.workspace.active_id() {
                Some(id) if !app.lsp.diagnostics(id).is_empty() => {
                    ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                        for diagnostic in app.lsp.diagnostics(id) {
                            let (r, g, b) = diagnostic.severity.rgb();
                            ui.colored_label(
                                Color32::from_rgb(r, g, b),
                                format!("riga {}: {}", diagnostic.line + 1, diagnostic.message),
                            );
                        }
                    });
                }
                _ => {
                    ui.label(RichText::new("Nessun problema segnalato.").weak());
                }
            }
        });
    if !open {
        app.dialogs.lsp_info = false;
    }
}

/// L'icona che accompagna una diagnostica nell'elenco.
fn severity_icon(severity: Severity) -> &'static str {
    match severity {
        Severity::Error => "✖",
        Severity::Warning => "⚠",
        Severity::Information => "ℹ",
        Severity::Hint => "•",
    }
}

/// Popup con i problemi segnalati dal language server per il documento attivo.
///
/// Si apre da solo quando arriva un errore *nuovo* (vedi `RideApp::poll_lsp`) e
/// si chiude con la ✕, con *Chiudi* o con `Esc`. Da ogni voce si salta alla
/// riga interessata. Non è un dialogo modale: non ruba la tastiera (è fuori da
/// [`crate::state::DialogState::any_open`]).
fn problems_window(ctx: &Context, app: &mut RideApp, actions: &mut Vec<Action>) {
    if !app.dialogs.problems {
        return;
    }
    let Some(id) = app.workspace.active_id() else {
        app.dialogs.problems = false;
        return;
    };
    // Una copia: poche decine di elementi, e serve per non tenere in prestito
    // `app` mentre disegniamo.
    let diagnostics = app.lsp.diagnostics(id).to_vec();
    let signature = crate::lsp::signature(&diagnostics);
    let mut jump: Option<usize> = None;
    let mut open = true;
    let mut close = false;

    Window::new("Problemi (LSP)")
        .open(&mut open)
        .collapsible(false)
        .resizable(true)
        .default_size([520.0, 240.0])
        .anchor(Align2::RIGHT_BOTTOM, [-16.0, -52.0])
        .show(ctx, |ui| {
            if diagnostics.is_empty() {
                ui.label(RichText::new("Nessun problema segnalato.").weak());
                return;
            }
            ScrollArea::vertical()
                .max_height(280.0)
                .show(ui, |ui| {
                    for diagnostic in &diagnostics {
                        let (r, g, b) = diagnostic.severity.rgb();
                        let text = format!(
                            "{}  riga {}: {}",
                            severity_icon(diagnostic.severity),
                            diagnostic.line + 1,
                            diagnostic.message
                        );
                        let label = ui.add(
                            egui::Label::new(
                                RichText::new(text).color(Color32::from_rgb(r, g, b)),
                            )
                            .sense(egui::Sense::click()),
                        );
                        if label.clicked() {
                            jump = Some(diagnostic.start);
                        }
                    }
                });
            ui.separator();
            ui.horizontal(|ui| {
                if ui.button("Chiudi").clicked() {
                    close = true;
                }
                ui.label(
                    RichText::new(format!("{} segnalazioni", diagnostics.len())).weak(),
                );
            });
        });

    if let Some(start) = jump {
        actions.push(Action::GotoDiagnostic(start));
    }
    // Chiudendolo (o saltando a una voce) l'utente "prende atto": finché i
    // problemi restano gli stessi il popup non si riapre da solo.
    if jump.is_some() || close || !open {
        app.dialogs.problems = false;
        app.dialogs.problems_ack = signature;
    }
}

/// Quante righe di build disegnare (una console non deve disegnare migliaia di
/// righe a ogni frame: si mostra solo la coda).
const BUILD_TAIL_LINES: usize = 600;

/// La console dei comandi di build (Maven/Gradle).
fn build_window(ctx: &Context, app: &mut RideApp, actions: &mut Vec<Action>) {
    if !app.build.open {
        return;
    }
    let mut open = true;
    let title = if app.build.label.is_empty() {
        "Console".to_string()
    } else {
        format!("Console — {}", app.build.label)
    };
    let running = app.build.running;
    let last = app.build.last;

    Window::new(title)
        .open(&mut open)
        .collapsible(false)
        .resizable(true)
        .default_size([760.0, 360.0])
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                if running {
                    ui.spinner();
                    ui.label(RichText::new("In esecuzione…").weak());
                    if ui.button("Interrompi").clicked() {
                        actions.push(Action::StopBuild);
                    }
                } else if let Some(success) = last {
                    let (icon, color) = if success {
                        ("✔", Color32::from_rgb(0x8a, 0xc0, 0x8a))
                    } else {
                        ("✖", Color32::from_rgb(0xe0, 0x7a, 0x7a))
                    };
                    let verb = if success { "Completato" } else { "Non riuscito" };
                    ui.colored_label(color, format!("{icon} {verb}"));
                } else {
                    ui.label(RichText::new("Nessun comando in esecuzione").weak());
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui
                        .add_enabled(
                            !app.build.lines.is_empty(),
                            egui::Button::new("Pulisci"),
                        )
                        .clicked()
                    {
                        app.build.lines.clear();
                    }
                });
            });
            ui.separator();

            // Solo la coda dell'output, in sola lettura (ma si può copiare).
            let start = app.build.lines.len().saturating_sub(BUILD_TAIL_LINES);
            let tail = app.build.lines[start..].join("\n");
            let mut buffer: &str = &tail;
            let rows = app.build.lines.len() - start;
            ScrollArea::vertical()
                .auto_shrink([false, false])
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    ui.add(
                        TextEdit::multiline(&mut buffer)
                            .code_editor()
                            .desired_width(f32::INFINITY)
                            .desired_rows(rows.max(1)),
                    );
                });
        });

    if !open {
        app.build.open = false;
    }
}

/// "Vai al file" (Ctrl+P): filtro fuzzy sull'elenco dei file del progetto.
fn quick_open_window(ctx: &Context, app: &mut RideApp, actions: &mut Vec<Action>) {
    if !app.dialogs.quick_open.open {
        return;
    }
    let root = app.workspace.root().map(|p| p.to_path_buf());
    let mut chosen: Option<PathBuf> = None;

    let quick = &mut app.dialogs.quick_open;
    Window::new("Vai al file")
        .collapsible(false)
        .resizable(true)
        .default_size([620.0, 420.0])
        .anchor(Align2::CENTER_TOP, [0.0, 80.0])
        .show(ctx, |ui| {
            let response = ui.add(
                TextEdit::singleline(&mut quick.query)
                    .hint_text("Nome del file…")
                    .desired_width(f32::INFINITY),
            );
            if quick.focus {
                response.request_focus();
                quick.focus = false;
            }
            ui.add_space(4.0);

            if quick.loading {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Elenco dei file…");
                });
                return;
            }

            let matches: Vec<(i32, PathBuf)> = quick
                .matches(root.as_deref())
                .into_iter()
                .map(|(score, path)| (score, path.to_path_buf()))
                .collect();
            if matches.is_empty() {
                ui.label(RichText::new("Nessun file corrisponde").weak());
                return;
            }
            if quick.selected >= matches.len() {
                quick.selected = matches.len() - 1;
            }

            if ui.input(|i| i.key_pressed(Key::ArrowDown)) {
                quick.selected = (quick.selected + 1).min(matches.len() - 1);
            }
            if ui.input(|i| i.key_pressed(Key::ArrowUp)) {
                quick.selected = quick.selected.saturating_sub(1);
            }
            if ui.input(|i| i.key_pressed(Key::Enter)) {
                chosen = Some(matches[quick.selected].1.clone());
            }

            ScrollArea::vertical()
                .id_salt("ride-quick-open")
                .max_height(340.0)
                .show(ui, |ui| {
                    for (index, (_, path)) in matches.iter().enumerate() {
                        let relative = root
                            .as_ref()
                            .and_then(|root| path.strip_prefix(root).ok())
                            .unwrap_or(path.as_path())
                            .display()
                            .to_string();
                        if ui
                            .selectable_label(index == quick.selected, relative)
                            .clicked()
                        {
                            chosen = Some(path.clone());
                        }
                    }
                });
        });

    if let Some(path) = chosen {
        actions.push(Action::OpenPath(path));
        app.dialogs.quick_open.open = false;
    }
}
