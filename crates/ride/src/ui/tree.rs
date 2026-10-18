//! L'albero del progetto.
//!
//! I figli di ogni cartella vengono letti **la prima volta che la si espande**
//! (vedi [`ride_core::project::FileNode`]): aprire un progetto con decine di
//! migliaia di file resta immediato.
//!
//! Il pannello non tocca il modello: apre file, rinomina, elimina… tutto
//! passando per [`Action`] o per i dialoghi, esattamente come il menu.

use std::path::{Path, PathBuf};

use egui::{Align, Color32, Layout, Response, RichText, ScrollArea, Ui};
use ride_core::project::{FileNode, TreeOptions};

use crate::action::Action;
use crate::app::RideApp;
use crate::state::{DialogState, Prompt, PromptAction};

/// Contesto immutabile di disegno, condiviso da tutta la ricorsione.
struct Ctx<'a> {
    options: &'a TreeOptions,
    open: &'a [PathBuf],
    root: Option<&'a Path>,
    selected: Option<&'a Path>,
    scroll_to: Option<&'a Path>,
}

/// Disegna il pannello del progetto.
pub fn panel(ui: &mut Ui, app: &mut RideApp) -> Vec<Action> {
    let mut actions = Vec::new();
    header(ui, app, &mut actions);
    ui.separator();

    let RideApp {
        tree,
        workspace,
        dialogs,
        ..
    } = app;

    if tree.root.is_none() {
        ui.vertical_centered(|ui| {
            ui.add_space(24.0);
            ui.label(RichText::new("Nessun progetto aperto").strong());
            ui.label(RichText::new("Apri una cartella per elencare i file.").weak());
            ui.add_space(8.0);
            if ui.button("Apri una cartella…").clicked() {
                actions.push(Action::OpenFolderDialog);
            }
        });
        return actions;
    }

    let options = tree.options.clone();
    let open: Vec<PathBuf> = workspace
        .docs()
        .filter_map(|(_, doc)| doc.path().map(|p| p.to_path_buf()))
        .collect();
    let root = workspace.root().map(|p| p.to_path_buf());
    let selected = tree.selected.clone();
    let scroll_to = tree.scroll_to.take();

    let ctx = Ctx {
        options: &options,
        open: &open,
        root: root.as_deref(),
        selected: selected.as_deref(),
        scroll_to: scroll_to.as_deref(),
    };

    let mut scroll_target: Option<PathBuf> = None;
    ScrollArea::vertical()
        .id_salt("ride-tree-scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            if let Some(root_node) = tree.root.as_mut() {
                node_ui(ui, root_node, &ctx, &mut actions, dialogs, &mut scroll_target);
            }
        });
    tree.scroll_to = scroll_target;
    actions
}

/// Intestazione del pannello: le linguette dei progetti aperti e i comandi rapidi.
///
/// Un clic su una linguetta cambia progetto: l'albero sotto diventa quello di
/// quella cartella. È il modo per tenere aperti più progetti insieme.
fn header(ui: &mut Ui, app: &RideApp, actions: &mut Vec<Action>) {
    let count = app.workspace.project_count();
    ui.horizontal_wrapped(|ui| {
        if count == 0 {
            ui.label(RichText::new("Progetto").strong());
            return;
        }
        for (index, project) in app.workspace.projects().enumerate() {
            let active = app.workspace.active_project_index() == Some(index);
            let response = ui
                .selectable_label(active, project.name())
                .on_hover_text(project.root().display().to_string());
            if response.clicked() && !active {
                actions.push(Action::SwitchProject(index));
            }
        }
    });
    ui.horizontal(|ui| {
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if count > 0
                && ui
                    .small_button("✕")
                    .on_hover_text("Chiudi il progetto attivo")
                    .clicked()
            {
                actions.push(Action::CloseProject);
            }
            if ui
                .small_button("📂")
                .on_hover_text("Apri un'altra cartella…")
                .clicked()
            {
                actions.push(Action::OpenFolderDialog);
            }
            if ui
                .small_button("⟳")
                .on_hover_text("Ricarica l'albero (F5)")
                .clicked()
            {
                actions.push(Action::RefreshTree);
            }
        });
    });
}

/// Disegna un nodo (file o cartella) e, se serve, i suoi figli.
fn node_ui(
    ui: &mut Ui,
    node: &mut FileNode,
    ctx: &Ctx<'_>,
    actions: &mut Vec<Action>,
    dialogs: &mut DialogState,
    scroll_target: &mut Option<PathBuf>,
) {
    let is_selected = ctx.selected == Some(node.path.as_path());
    let is_open = !node.is_dir && ctx.open.iter().any(|p| p == &node.path);
    let relative = ctx
        .root
        .and_then(|root| node.path.strip_prefix(root).ok())
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| node.name.clone());

    let marker = if node.is_dir {
        if node.expanded {
            "▾"
        } else {
            "▸"
        }
    } else if is_open {
        "●"
    } else {
        " "
    };
    let text = format!("{marker} {}", node.name);

    let response = ui
        .selectable_label(is_selected, text)
        .on_hover_text(relative);
    if ctx.scroll_to == Some(node.path.as_path()) {
        response.scroll_to_me(Some(Align::Center));
        *scroll_target = Some(node.path.clone());
    }
    if response.clicked() {
        if node.is_dir {
            node.toggle(ctx.options);
        } else {
            actions.push(Action::OpenPath(node.path.clone()));
        }
    }
    context_menu(&response, node, actions, dialogs);

    if node.is_dir && node.expanded {
        let indent_id = node.path.clone();
        let empty = node.children.as_ref().is_some_and(|c| c.is_empty());
        if empty {
            ui.indent(indent_id, |ui| {
                ui.label(RichText::new("(vuota)").weak());
            });
        } else if let Some(children) = node.children.as_mut() {
            ui.indent(indent_id, |ui| {
                for child in children.iter_mut() {
                    node_ui(ui, child, ctx, actions, dialogs, scroll_target);
                }
            });
        }
    }

    if let Some(error) = &node.error {
        ui.indent(node.path.clone(), |ui| {
            ui.colored_label(Color32::from_rgb(220, 120, 120), error);
        });
    }
}

/// Menu contestuale di un nodo: apri, crea, rinomina, elimina.
fn context_menu(
    response: &Response,
    node: &FileNode,
    actions: &mut Vec<Action>,
    dialogs: &mut DialogState,
) {
    let path = node.path.clone();
    let is_dir = node.is_dir;
    let name = node.name.clone();
    response.context_menu(|ui| {
        if is_dir {
            if ui.button("Nuovo file…").clicked() {
                dialogs.prompt = Some(Prompt::new(
                    "Nuovo file",
                    "Nome del file da creare (le cartelle col `/` vengono create).",
                    "",
                    PromptAction::NewFile(path.clone()),
                ));
                ui.close();
            }
            if ui.button("Nuova cartella…").clicked() {
                dialogs.prompt = Some(Prompt::new(
                    "Nuova cartella",
                    "Nome della cartella da creare.",
                    "",
                    PromptAction::NewFolder(path.clone()),
                ));
                ui.close();
            }
            ui.separator();
        } else if ui.button("Apri").clicked() {
            actions.push(Action::OpenPath(path.clone()));
            ui.close();
        }
        if ui.button("Rinomina…").clicked() {
            dialogs.prompt = Some(Prompt::new(
                "Rinomina",
                "Nuovo nome (senza cartelle).",
                name.clone(),
                PromptAction::Rename(path.clone()),
            ));
            ui.close();
        }
        if ui.button("Elimina").on_hover_text("Chiede conferma").clicked() {
            dialogs.confirm_delete = Some(path.clone());
            ui.close();
        }
    });
}
