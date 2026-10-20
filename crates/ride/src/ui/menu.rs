//! La barra dei menu.
//!
//! Ogni voce spinge un'[`Action`] nella lista: il menu non modifica nulla da sé,
//! quindi resta valido anche se i pannelli cambiano ordine di disegno.

use egui::Ui;

use crate::action::Action;
use crate::app::RideApp;

/// Disegna la barra dei menu e raccoglie le azioni richieste.
pub fn bar(ui: &mut Ui, app: &RideApp) -> Vec<Action> {
    let mut actions = Vec::new();
    egui::MenuBar::new().ui(ui, |ui| {
        file_menu(ui, app, &mut actions);
        edit_menu(ui, app, &mut actions);
        view_menu(ui, app, &mut actions);
        run_menu(ui, app, &mut actions);
        go_menu(ui, app, &mut actions);
        help_menu(ui, app, &mut actions);
    });
    actions
}

/// Una voce sempre attiva.
fn item(ui: &mut Ui, actions: &mut Vec<Action>, label: &str, action: Action) {
    if ui.button(label).clicked() {
        actions.push(action);
        ui.close();
    }
}

/// Una voce che può essere disattivata (es. "Salva" senza documenti aperti).
fn item_enabled(
    ui: &mut Ui,
    actions: &mut Vec<Action>,
    label: &str,
    action: Action,
    enabled: bool,
) {
    if ui.add_enabled(enabled, egui::Button::new(label)).clicked() {
        actions.push(action);
        ui.close();
    }
}

/// Una voce che riflette un interruttore (vista, temi, …).
fn toggle(ui: &mut Ui, actions: &mut Vec<Action>, label: &str, on: bool, action: Action) {
    let text = if on {
        format!("✔ {label}")
    } else {
        format!("   {label}")
    };
    if ui.button(text).clicked() {
        actions.push(action);
        ui.close();
    }
}

fn file_menu(ui: &mut Ui, app: &RideApp, actions: &mut Vec<Action>) {
    let has_doc = app.workspace.active_id().is_some();
    ui.menu_button("File", |ui| {
        item(ui, actions, "Nuovo file", Action::NewFile);
        item(ui, actions, "Apri file…", Action::OpenFileDialog);
        item(ui, actions, "Apri cartella…", Action::OpenFolderDialog);
        ui.separator();
        item_enabled(ui, actions, "Salva", Action::Save, has_doc);
        item_enabled(ui, actions, "Salva con nome…", Action::SaveAs, has_doc);
        item_enabled(
            ui,
            actions,
            "Salva tutto",
            Action::SaveAll,
            app.workspace.dirty_count() > 0,
        );
        item_enabled(ui, actions, "Ricarica dal disco", Action::ReloadFile, has_doc);
        ui.separator();
        item_enabled(ui, actions, "Chiudi la tab", Action::CloseTab, has_doc);
        item_enabled(
            ui,
            actions,
            "Chiudi tutte le tab",
            Action::CloseAllTabs,
            !app.workspace.is_empty(),
        );
        ui.separator();
        if app.workspace.project().is_some() {
            item(ui, actions, "Chiudi il progetto attivo", Action::CloseProject);
        }
        if app.workspace.project_count() > 1 {
            ui.menu_button("Passa al progetto", |ui| {
                for (index, project) in app.workspace.projects().enumerate() {
                    let active = app.workspace.active_project_index() == Some(index);
                    let response = ui
                        .selectable_label(active, project.name())
                        .on_hover_text(project.root().display().to_string());
                    if response.clicked() && !active {
                        actions.push(Action::SwitchProject(index));
                        ui.close();
                    }
                }
            });
        }
        item(ui, actions, "Esci", Action::Quit);
    });
}

fn edit_menu(ui: &mut Ui, app: &RideApp, actions: &mut Vec<Action>) {
    let has_doc = app.workspace.active_id().is_some();
    ui.menu_button("Modifica", |ui| {
        item_enabled(
            ui,
            actions,
            "Commenta / scommenta",
            Action::ToggleComment,
            has_doc,
        );
        item_enabled(ui, actions, "Indenta", Action::Indent, has_doc);
        item_enabled(ui, actions, "Disindenta", Action::Dedent, has_doc);
        ui.separator();
        item_enabled(ui, actions, "Duplica la riga", Action::DuplicateLines, has_doc);
        item_enabled(ui, actions, "Sposta la riga su", Action::MoveLinesUp, has_doc);
        item_enabled(
            ui,
            actions,
            "Sposta la riga giù",
            Action::MoveLinesDown,
            has_doc,
        );
        item_enabled(ui, actions, "Cancella la riga", Action::DeleteLines, has_doc);
        ui.separator();
        item(ui, actions, "Trova…", Action::Find);
        item(ui, actions, "Trova e sostituisci…", Action::Replace);
        item(ui, actions, "Vai alla riga…", Action::GotoLine);
        ui.separator();
        // Si formatta con lo strumento esterno, o — se non c'è — con il
        // language server: la voce dice quale dei due verrà usato.
        let language = app.workspace.active().map(|d| d.language());
        let external = language.and_then(ride_core::format::formatter_name);
        let external_ok = language.is_some_and(ride_core::format::can_format);
        let server_ok = language.is_some_and(|l| {
            crate::lsp::server_can_format(l) && crate::lsp::LspManager::server_available(l)
        });
        let label = if external_ok {
            format!("Formatta con {}", external.unwrap_or("il formatter"))
        } else if server_ok {
            let name = language
                .and_then(crate::lsp::server_for)
                .map(|c| c.program)
                .unwrap_or("il language server");
            format!("Formatta con {name} (LSP)")
        } else if let Some(name) = external {
            format!("Formatta con {name}")
        } else {
            "Formatta il documento".to_string()
        };
        item_enabled(
            ui,
            actions,
            &label,
            Action::FormatDocument,
            external_ok || server_ok,
        );
    });
}

fn view_menu(ui: &mut Ui, app: &RideApp, actions: &mut Vec<Action>) {
    ui.menu_button("Visualizza", |ui| {
        toggle(
            ui,
            actions,
            "Pannello del progetto",
            app.prefs.show_tree,
            Action::ToggleTree,
        );
        toggle(
            ui,
            actions,
            "Cerca nel progetto",
            app.prefs.show_search_panel,
            Action::ToggleSearchPanel,
        );
        toggle(
            ui,
            actions,
            "Terminale (Ctrl+`)",
            app.terminal.open,
            Action::ToggleTerminal,
        );
        toggle(
            ui,
            actions,
            "Autoscomparsa del terminale",
            app.terminal.auto_hide,
            Action::ToggleTerminalAutoHide,
        );
        ui.separator();
        toggle(
            ui,
            actions,
            "Numeri di riga",
            app.prefs.line_numbers,
            Action::ToggleLineNumbers,
        );
        toggle(
            ui,
            actions,
            "Evidenziazione della sintassi",
            app.prefs.highlight,
            Action::ToggleHighlight,
        );
        toggle(
            ui,
            actions,
            "Chiudi le parentesi automaticamente",
            app.prefs.auto_brackets,
            Action::ToggleAutoBrackets,
        );
        toggle(
            ui,
            actions,
            "Evidenzia la coppia di parentesi",
            app.prefs.match_brackets,
            Action::ToggleBracketMatch,
        );
        toggle(
            ui,
            actions,
            "Piega i blocchi dalle graffe",
            app.prefs.folding,
            Action::ToggleFolding,
        );
        toggle(
            ui,
            actions,
            "Language server (LSP)",
            app.prefs.lsp,
            Action::ToggleLsp,
        );
        ui.separator();
        toggle(
            ui,
            actions,
            "Tema scuro",
            app.prefs.dark,
            Action::NextTheme,
        );
        item(
            ui,
            actions,
            &format!("Tema sintassi: {}", app.highlighter.theme_name()),
            Action::NextSyntaxTheme,
        );
        ui.separator();
        item(ui, actions, "Ingrandisci", Action::ZoomIn);
        item(ui, actions, "Rimpicciolisci", Action::ZoomOut);
        item(ui, actions, "Dimensione predefinita", Action::ZoomReset);
        ui.separator();
        item_enabled(
            ui,
            actions,
            "Ricarica l'albero",
            Action::RefreshTree,
            app.workspace.project().is_some(),
        );
    });
}

/// Il menu dei comandi di build: compare solo per i progetti Maven o Gradle.
fn run_menu(ui: &mut Ui, app: &RideApp, actions: &mut Vec<Action>) {
    // Niente progetto Java e nessun output da rileggere: il menu non serve.
    if app.toolchains.is_empty() && app.build.lines.is_empty() {
        return;
    }
    ui.menu_button("Esegui", |ui| {
        for toolchain in &app.toolchains {
            let tool = toolchain.tool;
            let running = app.build.running;
            let available = toolchain.available();
            ui.menu_button(tool.name(), |ui| {
                for (index, command) in ride_core::build::commands(tool).iter().enumerate() {
                    let hint = format!(
                        "{} {}",
                        toolchain.label(),
                        toolchain.command_line(command).join(" ")
                    );
                    let response = ui.add_enabled(
                        available && !running,
                        egui::Button::new(command.label),
                    );
                    if response.clicked() {
                        actions.push(Action::RunBuild(tool, index));
                        ui.close();
                    }
                    response.on_hover_text(hint);
                }
                // Goal dei plugin Maven: i nomi li dà il catalogo, ma a
                // comparire sono solo i plugin del progetto (dal `pom.xml`),
                // più i pochi che Maven risolve da sé.
                if tool == ride_core::build::BuildTool::Maven && !app.maven_plugins.is_empty() {
                    ui.separator();
                    ui.menu_button("Plugin", |ui| {
                        for plugin in &app.maven_plugins {
                            let submenu = ui.menu_button(plugin.prefix, |ui| {
                                for goal in plugin.goals {
                                    let goal_args = plugin.goal(goal);
                                    let hint = format!(
                                        "{} {}",
                                        toolchain.label(),
                                        toolchain.arguments(&[goal_args.as_str()]).join(" ")
                                    );
                                    let response = ui
                                        .add_enabled(available && !running, egui::Button::new(*goal));
                                    if response.clicked() {
                                        actions.push(Action::RunBuildArgs(tool, goal_args));
                                        ui.close();
                                    }
                                    response.on_hover_text(hint);
                                }
                            });
                            submenu.response.on_hover_text(plugin.artifact_id);
                        }
                    });
                }
                // Qualunque altro goal, con le sue opzioni.
                ui.separator();
                let libero =
                    ui.add_enabled(available && !running, egui::Button::new("Comando…"));
                if libero.clicked() {
                    actions.push(Action::AskBuildArgs(tool));
                    ui.close();
                }
                libero.on_hover_text("Scrivi tu il goal e le sue opzioni");
                if !available {
                    ui.separator();
                    ui.label(
                        egui::RichText::new(format!("«{}» non è installato", toolchain.label()))
                            .weak(),
                    );
                }
            });
        }
        if !app.toolchains.is_empty() {
            ui.separator();
        }
        if ui
            .add_enabled(
                !app.build.lines.is_empty(),
                egui::Button::new("Mostra la console"),
            )
            .clicked()
        {
            actions.push(Action::ShowBuildConsole);
            ui.close();
        }
        if ui
            .add_enabled(app.build.running, egui::Button::new("Interrompi"))
            .clicked()
        {
            actions.push(Action::StopBuild);
            ui.close();
        }
    });
}

fn go_menu(ui: &mut Ui, app: &RideApp, actions: &mut Vec<Action>) {
    let has_doc = app.workspace.active_id().is_some();
    ui.menu_button("Vai", |ui| {
        item_enabled(
            ui,
            actions,
            "Vai al file… (Ctrl+P)",
            Action::QuickOpen,
            app.workspace.project().is_some(),
        );
        item_enabled(ui, actions, "Vai alla riga…", Action::GotoLine, has_doc);
        item_enabled(
            ui,
            actions,
            "Vai alla definizione (F12)",
            Action::GotoDefinition,
            has_doc,
        );
        item_enabled(
            ui,
            actions,
            "Mostra nell'albero",
            Action::RevealActiveFile,
            has_doc,
        );
        ui.separator();
        item_enabled(ui, actions, "Tab successiva", Action::NextTab, app.workspace.len() > 1);
        item_enabled(
            ui,
            actions,
            "Tab precedente",
            Action::PreviousTab,
            app.workspace.len() > 1,
        );
    });
}

fn help_menu(ui: &mut Ui, app: &RideApp, actions: &mut Vec<Action>) {
    ui.menu_button("Aiuto", |ui| {
        item(ui, actions, "Scorciatoie da tastiera (F1)", Action::ShowShortcuts);
        item(ui, actions, "Language server", Action::ShowLspInfo);
        if app.workspace.active_id().is_some() {
            item(ui, actions, "Problemi segnalati (LSP)", Action::ShowProblems);
        }
        ui.separator();
        item(ui, actions, "Informazioni su Ride", Action::ShowAbout);
        if app.workspace.active_id().is_some() {
            ui.separator();
            item(ui, actions, "Riavvia il language server", Action::RestartLsp);
        }
    });
}
