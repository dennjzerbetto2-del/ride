//! Il pannello "cerca nel progetto".
//!
//! La ricerca vera gira su un thread separato ([`crate::state::Jobs`]): qui si
//! disegna solo la query e i risultati già pronti. Se la query cambia, il
//! pannello la segna come "da rifare" e l'aggiornamento parte da solo.

use egui::{Align, Color32, CollapsingHeader, Layout, RichText, ScrollArea, TextEdit, Ui};

use crate::action::Action;
use crate::app::RideApp;

/// Disegna il pannello di ricerca e raccoglie le azioni.
pub fn panel(ui: &mut Ui, app: &mut RideApp) -> Vec<Action> {
    let mut actions = Vec::new();

    let RideApp {
        search,
        workspace,
        prefs,
        ..
    } = app;

    ui.horizontal(|ui| {
        ui.label(RichText::new("Cerca nel progetto").strong());
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui
                .small_button("✕")
                .on_hover_text("Chiudi il pannello")
                .clicked()
            {
                prefs.show_search_panel = false;
                prefs.mark_dirty();
            }
        });
    });
    ui.separator();

    let typed = ui.add(
        TextEdit::singleline(&mut search.query.text)
            .hint_text("Testo da cercare…")
            .desired_width(f32::INFINITY),
    );
    if search.focus_query {
        typed.request_focus();
        search.focus_query = false;
    }
    if typed.changed() {
        search.mark_dirty();
    }
    if typed.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
        search.mark_dirty();
    }

    ui.horizontal(|ui| {
        ui.label("File:");
        let include = ui.add(
            TextEdit::singleline(&mut search.include)
                .hint_text("*.rs")
                .desired_width(120.0),
        );
        if include.changed() {
            search.mark_dirty();
        }
        if ui
            .button("Cerca")
            .on_hover_text("Rilancia la ricerca")
            .clicked()
        {
            search.mark_dirty();
        }
    });

    ui.horizontal(|ui| {
        if ui.checkbox(&mut search.query.case_sensitive, "Aa").changed() {
            search.mark_dirty();
        }
        if ui.checkbox(&mut search.query.whole_word, "Parola").changed() {
            search.mark_dirty();
        }
        if ui.checkbox(&mut search.query.use_regex, "Regex").changed() {
            search.mark_dirty();
        }
    });

    // Per impostazione predefinita la ricerca segue le regole dell'albero
    // (`.gitignore`, file nascosti). Questi interruttori servono proprio per
    // cercare in *tutti* i file, anche quelli esclusi.
    ui.horizontal(|ui| {
        if ui
            .checkbox(&mut prefs.show_hidden, "Nascosti")
            .on_hover_text("Cerca anche nei file e nelle cartelle nascoste")
            .changed()
        {
            prefs.mark_dirty();
            search.mark_dirty();
        }
        if ui
            .checkbox(&mut prefs.respect_gitignore, "Rispetta .gitignore")
            .on_hover_text("Togli la spunta per cercare anche nei file ignorati")
            .changed()
        {
            prefs.mark_dirty();
            search.mark_dirty();
        }
    });

    ui.separator();

    if let Some(error) = &search.error {
        ui.colored_label(Color32::from_rgb(230, 120, 120), error);
    }
    if search.running {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label("Ricerca in corso…");
        });
    }

    if workspace.root().is_none() {
        ui.label(RichText::new("Apri un progetto per cercare in tutti i file.").weak());
        return actions;
    }

    let Some(result) = &search.result else {
        if search.query.text.trim().is_empty() {
            ui.label(RichText::new("Scrivi qualcosa per cercare in tutti i file.").weak());
        }
        return actions;
    };

    ui.horizontal(|ui| {
        let summary = format!(
            "{} risultati in {} file",
            result.hits.len(),
            result.files_scanned
        );
        ui.label(RichText::new(summary).strong());
        if result.truncated {
            ui.label(RichText::new("(elenco troncato)").weak());
        }
        if result.files_skipped > 0 {
            ui.label(
                RichText::new(format!("{} file saltati", result.files_skipped)).weak(),
            );
        }
    });

    let root = workspace.root().map(|p| p.to_path_buf());
    ScrollArea::vertical()
        .id_salt("ride-search-results")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for (path, hits) in result.by_file() {
                let relative = root
                    .as_ref()
                    .and_then(|root| path.strip_prefix(root).ok())
                    .unwrap_or(path.as_path())
                    .display()
                    .to_string();
                let title = format!("{relative}  ({})", hits.len());
                CollapsingHeader::new(title)
                    .id_salt(&path)
                    .default_open(true)
                    .show(ui, |ui| {
                        for hit in hits {
                            let line = hit.line_text.trim();
                            let label = format!("{}: {}", hit.line + 1, line);
                            if ui
                                .selectable_label(false, label)
                                .on_hover_text(path.display().to_string())
                                .clicked()
                            {
                                actions.push(Action::OpenPath(path.clone()));
                                actions.push(Action::GotoLineAt(hit.line + 1));
                            }
                        }
                    });
            }
        });

    actions
}
