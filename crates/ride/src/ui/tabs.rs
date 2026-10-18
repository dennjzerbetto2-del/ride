//! La barra delle tab dei file aperti.
//!
//! Ogni tab ricorda il documento tramite il suo [`DocumentId`]: l'identità
//! sopravvive al riordino e alla chiusura di altre tab. Le tab si possono
//! trascinare per riordinarle e chiudere con il tasto centrale del mouse.

use egui::{Button, RichText, ScrollArea, Sense, Stroke, Ui};
use ride_core::DocumentId;

use crate::action::Action;
use crate::app::RideApp;

/// Disegna la barra delle tab.
pub fn bar(ui: &mut Ui, app: &RideApp) -> Vec<Action> {
    let mut actions = Vec::new();
    ui.horizontal(|ui| {
        if let Some(project) = app.workspace.project() {
            ui.label(RichText::new(format!("📁 {}", project.name())).strong());
            ui.separator();
        }
        if app.workspace.is_empty() {
            ui.label(RichText::new("Nessun file aperto").weak());
            return;
        }
        ScrollArea::horizontal()
            .id_salt("ride-tabs-scroll")
            .auto_shrink([false, true])
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let ids: Vec<DocumentId> = app.workspace.docs().map(|(id, _)| id).collect();
                    let active = app.workspace.active_id();
                    for (index, id) in ids.iter().enumerate() {
                        tab(ui, app, &mut actions, index, *id, active);
                    }
                });
            });
    });
    actions
}

/// Una singola tab.
fn tab(
    ui: &mut Ui,
    app: &RideApp,
    actions: &mut Vec<Action>,
    index: usize,
    id: DocumentId,
    active: Option<DocumentId>,
) {
    let Some(doc) = app.workspace.get(id) else {
        return;
    };
    let selected = active == Some(id);
    let mut label = doc.display_name().to_string();
    if doc.is_dirty() {
        label = format!("● {label}");
    } else {
        label = format!("  {label}");
    }

    let response = ui.add(
        Button::selectable(selected, label)
            .sense(Sense::click_and_drag())
            .truncate(),
    );
    if response.clicked() {
        actions.push(Action::ActivateTab(id));
    }
    if response.middle_clicked() {
        actions.push(Action::CloseTabAt(index));
    }

    response.context_menu(|ui| {
        if ui.button("Chiudi").clicked() {
            actions.push(Action::CloseTabAt(index));
            ui.close();
        }
        if ui.button("Chiudi le altre").clicked() {
            actions.push(Action::CloseOtherTabs);
            ui.close();
        }
    });

    // Riordino: il payload è l'id della tab trascinata.
    response.dnd_set_drag_payload(id);
    if let Some(source) = response.dnd_hover_payload::<DocumentId>() {
        let x = response.rect.left();
        ui.painter().vline(
            x,
            response.rect.top()..=response.rect.bottom(),
            Stroke::new(2.0, ui.visuals().selection.bg_fill),
        );
        if response.dnd_release_payload::<DocumentId>().is_some() {
            actions.push(Action::MoveTab(*source, index));
        }
    }

    let tooltip = doc.path().map(|p| p.display().to_string());
    let response = response.on_hover_text(match tooltip {
        Some(path) => format!("{path}\n(trascina per riordinare, tasto centrale per chiudere)"),
        None => "Documento non ancora salvato".to_string(),
    });
    let _ = response;

    if ui
        .add(Button::new("✕").small().frame(false))
        .on_hover_text("Chiudi la tab")
        .clicked()
    {
        actions.push(Action::CloseTabAt(index));
    }
    ui.add_space(2.0);
}
