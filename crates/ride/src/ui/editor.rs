//! L'area di editing: numeri di riga fissi, testo con evidenziazione, ricerca.
//!
//! ## Perché non è solo un `TextEdit`
//!
//! Tre dettagli fanno la differenza tra "una casella di testo" e un editor:
//!
//! 1. **i numeri di riga** devono restare fermi mentre si scorre in orizzontale
//!    e non devono costare nulla con 100.000 righe → vengono disegnati *dopo*
//!    lo `ScrollArea`, solo per le righe visibili, usando `galley_pos` di
//!    `TextEdit` per allinearli al pixel;
//! 2. **l'indentazione automatica** dopo `Invio` e `Tab` che rispetta il
//!    linguaggio: `TextEdit` non lo fa, quindi il testo viene corretto subito
//!    dopo l'evento (vedi `auto_indent`);
//! 3. **le righe lunghe si scorrono**, non si spezzano: il `layouter` costruisce
//!    una galley senza a capo e il widget prende la larghezza misurata al frame
//!    precedente.

use std::ops::Range;

use egui::text::{CCursor, CCursorRange};
use egui::{
    pos2, Align, Align2, Color32, Galley, Id, Key, Layout, Pos2, Rect, RichText, ScrollArea, Sense,
    Stroke, StrokeKind, TextEdit, TextStyle, Ui,
};
use ride_core::editor::{AutoPair, EnterEdit, Lines};
use ride_core::folds::FoldBlock;
use ride_core::symbols::FunctionSymbol;
use ride_core::{Cursor, Document, DocumentId, Language};

use crate::action::Action;
use crate::app::RideApp;
use crate::highlight::{HighlightKey, Highlighter};
use crate::lsp::Diagnostic;
use crate::prefs::Prefs;
use crate::state::{EditorSnapshot, FindState};

/// Spazio a sinistra e a destra dei numeri di riga.
const GUTTER_PAD: f32 = 8.0;
/// Distanza tra il canale dei numeri e il codice.
const GUTTER_GAP: f32 = 10.0;
/// Larghezza della colonna dei marcatori di piega (a destra dei numeri).
const FOLD_MARKER_WIDTH: f32 = 14.0;
/// Larghezza della colonna con la `M` dei metodi (a sinistra dei numeri).
const SYMBOL_MARKER_WIDTH: f32 = 16.0;
/// Oltre questo numero di righe i numeri non vengono disegnati.
const MAX_GUTTER_LINES: usize = 200_000;

/// Cosa ha combinato l'editor in questo frame.
pub struct EditorOutput {
    /// Indice di carattere del cursore.
    pub caret: usize,
    /// Intervallo selezionato (in caratteri, ordinato).
    pub selection: (usize, usize),
    /// L'utente ha davvero modificato il testo (non solo spostato il cursore).
    pub changed: bool,
    /// Comandi chiesti dal menu contestuale.
    pub actions: Vec<Action>,
}

/// Disegna l'area di editing del documento attivo e aggiorna lo stato dell'app.
///
/// È il punto in cui il modello (`Document`) e lo stato dell'interfaccia
/// (`EditorSnapshot`, posizione del cursore) restano allineati.
pub fn panel(ui: &mut Ui, app: &mut RideApp) -> Vec<Action> {
    let mut actions = Vec::new();

    let Some(id) = app.workspace.active_id() else {
        welcome(ui, app, &mut actions);
        return actions;
    };

    // Selezione richiesta da un'azione o da una ricerca: va impostata *prima*
    // che `TextEdit` legga il suo stato. Chi ha chiesto la selezione decide
    // anche se spostare il focus (vedi `RideApp::pending_selection`).
    if let Some((from, to)) = app.pending_selection.take() {
        select(ui.ctx(), id, from, to);
    }

    find_bar(ui, app, &mut actions);

    let wants_focus = app.focus_editor;
    let cursor_line = app.cursor_line;
    // La diagnostica del language server per questo documento (una copia
    // piccola: poche decine di elementi al massimo).
    let diagnostics = app.lsp.diagnostics(id).to_vec();

    let output = {
        let RideApp {
            workspace,
            prefs,
            highlighter,
            find,
            ..
        } = app;
        let Some(doc) = workspace.get_mut(id) else {
            return actions;
        };
        show(ui, id, doc, prefs, highlighter, find, cursor_line, &diagnostics)
    };

    if wants_focus {
        ui.ctx()
            .memory_mut(|memory| memory.request_focus(editor_id(id)));
        app.focus_editor = false;
    }

    let (line, column) = app.cursor_position(id, output.caret);
    app.cursor_line = line;
    if let Some(doc) = app.workspace.get_mut(id) {
        doc.set_cursor(Cursor::new(line, column));
    }
    app.editor = Some(EditorSnapshot {
        doc: id,
        selection: output.selection,
        line,
        column,
    });

    // L'utente ha modificato il testo: `TextEdit` scrive direttamente nel
    // buffer senza passare da `Document::text_mut`, quindi la revisione (e lo
    // stato "modificato") vanno segnati qui. Nello stesso giro il language
    // server riceve il testo nuovo (`textDocument/didChange`).
    if output.changed {
        let text = match app.workspace.get_mut(id) {
            Some(doc) => {
                doc.mark_edited();
                doc.text().to_string()
            }
            None => String::new(),
        };
        app.lsp.did_change(id, &text);
    }

    actions.extend(output.actions);
    actions
}

/// Schermata mostrata quando non c'è nessun documento aperto.
fn welcome(ui: &mut Ui, app: &RideApp, actions: &mut Vec<Action>) {
    ui.vertical_centered(|ui| {
        ui.add_space((ui.available_height() * 0.25).max(0.0));
        ui.label(RichText::new("Ride").heading());
        ui.label(RichText::new("Editor di codice multi-file").weak());
        ui.add_space(16.0);
        ui.horizontal(|ui| {
            if ui.button("Nuovo file (Ctrl+N)").clicked() {
                actions.push(Action::NewFile);
            }
            if ui.button("Apri file… (Ctrl+O)").clicked() {
                actions.push(Action::OpenFileDialog);
            }
            if ui.button("Apri cartella…").clicked() {
                actions.push(Action::OpenFolderDialog);
            }
        });
        ui.add_space(12.0);
        if app.workspace.project().is_some() {
            ui.label(RichText::new("Ctrl+P per aprire un file del progetto").weak());
        } else {
            ui.label(RichText::new("Nessun progetto aperto: puoi aprirne uno dal menu File").weak());
        }
    });
}

/// Identificativo stabile del widget di editing di un documento.
pub fn editor_id(id: DocumentId) -> Id {
    Id::unique(("ride-editor", id))
}

/// Id dell'area scorrevole (per ricordare la posizione di scorrimento).
fn scroll_id(id: DocumentId) -> Id {
    Id::unique(("ride-editor-scroll", id))
}

/// Imposta la selezione (in caratteri) del widget di editing.
pub fn select(ctx: &egui::Context, id: DocumentId, from: usize, to: usize) {
    let mut state = TextEdit::load_state(ctx, editor_id(id)).unwrap_or_default();
    state
        .cursor
        .set_char_range(Some(CCursorRange::two(CCursor::new(from), CCursor::new(to))));
    TextEdit::store_state(ctx, editor_id(id), state);
}

/// La barra "trova / sostituisci" sopra l'editor.
///
/// È una vista del solo [`FindState`]: le azioni (navigazione, sostituzione)
/// vengono restituite come [`Action`] e applicate da chi chiama, così la barra
/// non tocca mai il documento.
fn find_bar(ui: &mut Ui, app: &mut RideApp, actions: &mut Vec<Action>) {
    if !app.find.open {
        return;
    }
    let find = &mut app.find;
    let mut close = false;

    ui.horizontal(|ui| {
        ui.label("Trova:");
        let response = ui.add(
            TextEdit::singleline(&mut find.query)
                .desired_width(240.0)
                .hint_text("Testo da cercare…"),
        );
        if find.focus_query {
            response.request_focus();
            find.focus_query = false;
        }
        if response.changed() {
            find.stale = true;
        }
        if response.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
            actions.push(Action::FindNext);
        }

        // Quante corrispondenze ci sono e su quale siamo.
        if let Some(error) = &find.error {
            ui.colored_label(Color32::from_rgb(230, 120, 120), error);
        } else if !find.query.is_empty() {
            let total = find.matches.len();
            let current = if total == 0 { 0 } else { find.current + 1 };
            ui.label(RichText::new(format!("{current}/{total}")).weak());
        }

        if ui
            .small_button("◀")
            .on_hover_text("Corrispondenza precedente (Shift+F3)")
            .clicked()
        {
            actions.push(Action::FindPrevious);
        }
        if ui
            .small_button("▶")
            .on_hover_text("Corrispondenza successiva (F3)")
            .clicked()
        {
            actions.push(Action::FindNext);
        }

        if ui
            .selectable_label(find.case_sensitive, "Aa")
            .on_hover_text("Distingui maiuscole e minuscole")
            .clicked()
        {
            find.case_sensitive = !find.case_sensitive;
            find.stale = true;
        }
        if ui
            .selectable_label(find.whole_word, "Parola")
            .on_hover_text("Solo parole intere")
            .clicked()
        {
            find.whole_word = !find.whole_word;
            find.stale = true;
        }
        if ui
            .selectable_label(find.use_regex, "Regex")
            .on_hover_text("Espressione regolare")
            .clicked()
        {
            find.use_regex = !find.use_regex;
            find.stale = true;
        }
        if !find.show_replace
            && ui
                .small_button("⇄")
                .on_hover_text("Mostra la sostituzione (Ctrl+H)")
                .clicked()
        {
            find.show_replace = true;
        }

        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui
                .small_button("✕")
                .on_hover_text("Chiudi (Esc)")
                .clicked()
            {
                close = true;
            }
        });
    });

    if find.show_replace {
        ui.horizontal(|ui| {
            ui.label("Con:");
            let response = ui.add(
                TextEdit::singleline(&mut find.replace)
                    .desired_width(240.0)
                    .hint_text("Sostituzione…"),
            );
            if response.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                actions.push(Action::ReplaceCurrent);
            }
            if ui
                .button("Sostituisci")
                .on_hover_text("Sostituisce la corrispondenza attiva")
                .clicked()
            {
                actions.push(Action::ReplaceCurrent);
            }
            if ui.button("Sostituisci tutto").clicked() {
                actions.push(Action::ReplaceAll);
            }
        });
    }

    ui.separator();

    if close {
        find.close();
        app.focus_editor = true;
    }
}

/// Disegna l'editor del documento attivo.
///
/// `cursor_line` è la riga del cursore nota all'applicazione (calcolata una
/// volta per cambiamento del cursore): serve solo a evidenziare il numero.
#[allow(clippy::too_many_arguments)]
pub fn show(
    ui: &mut Ui,
    id: DocumentId,
    doc: &mut Document,
    prefs: &Prefs,
    highlighter: &mut Highlighter,
    find: &mut FindState,
    cursor_line: usize,
    diagnostics: &[Diagnostic],
) -> EditorOutput {
    let ctx = ui.ctx().clone();
    let font = prefs.code_font();
    let row_height = ui.fonts_mut(|fonts| fonts.row_height(&font));
    let line_height = row_height + ui.spacing().extra_text_line_spacing;

    // La ricerca dentro il file si ricalcola solo quando serve.
    find.ensure_matches(doc.text());

    let lines = doc.lines();
    let line_count = lines.count();
    let digits = line_count.to_string().len().max(2);
    let language = doc.language();

    // Piegatura dei blocchi `{ … }`. Si clicca nel canale dei numeri, quindi
    // senza canale non ha senso calcolarla.
    let folding = prefs.folding && prefs.line_numbers && line_count <= MAX_GUTTER_LINES;
    let blocks: Vec<FoldBlock> = if folding {
        ride_core::folds::fold_blocks(doc.text())
    } else {
        Vec::new()
    };
    let folds: Vec<usize> = doc.folds().to_vec();
    let mut hidden: Vec<Range<usize>> = blocks
        .iter()
        .filter(|block| folds.binary_search(&block.open_byte).is_ok())
        .map(FoldBlock::hidden_lines)
        .collect();
    ride_core::folds::merge_ranges(&mut hidden);
    let visible_lines = line_count - hidden.iter().map(|range| range.len()).sum::<usize>();

    // Funzioni e metodi: la `M` nel canale e il nome nel popup. Come la
    // piegatura, ha senso solo se c'è il canale e il file non è enorme.
    let symbols: Vec<FunctionSymbol> = if prefs.line_numbers
        && line_count <= MAX_GUTTER_LINES
        && language.is_code()
    {
        ride_core::symbols::function_symbols(doc.text(), language)
    } else {
        Vec::new()
    };
    let symbol_width = if symbols.is_empty() {
        0.0
    } else {
        SYMBOL_MARKER_WIDTH
    };

    let gutter_width = if prefs.line_numbers && line_count <= MAX_GUTTER_LINES {
        let sample = ui.fonts_mut(|fonts| fonts.layout_no_wrap(
            "0".repeat(digits),
            font.clone(),
            Color32::WHITE,
        ));
        sample.size().x + GUTTER_PAD * 2.0 + GUTTER_GAP + FOLD_MARKER_WIDTH + symbol_width
    } else {
        0.0
    };

    // Larghezza: quella misurata al frame precedente, mai meno della finestra.
    let viewport = ui.available_size();
    let minimum_width = (viewport.x - gutter_width - 24.0).max(160.0);
    let desired_width = highlighter
        .measured_width(id)
        .unwrap_or(minimum_width)
        .max(minimum_width)
        .min(100_000.0);

    // Tasti che egui non gestisce come vorremmo: li leggiamo prima.
    let enter_pressed = ui.input(|i| i.key_pressed(Key::Enter));
    let tab_pressed = ui.input(|i| i.key_pressed(Key::Tab));

    // Stato *prima* della digitazione, per gli automatismi sulle parentesi:
    // la selezione è vuota? quale carattere segue il cursore? cosa è stato
    // digitato in questo frame?
    let pre_range = TextEdit::load_state(&ctx, editor_id(id)).and_then(|s| s.cursor.char_range());
    let pre_empty = pre_range.map(|r| r.is_empty()).unwrap_or(true);
    let pre_caret = pre_range.map(|r| r.primary.index.0).unwrap_or(0);
    let typed = single_typed_char(ui);
    let next_char = ride_core::editor::char_at(doc.text(), pre_caret);

    let key = HighlightKey {
        language: doc.language(),
        find: find.stamp(),
        folds: {
            use std::hash::{Hash, Hasher};
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            for range in &hidden {
                (range.start, range.end).hash(&mut hasher);
            }
            hasher.finish()
        },
        style: prefs.font_size.to_bits() as u64 ^ ((prefs.highlight as u64) << 32),
    };
    let matches: Vec<(usize, usize)> = find.matches.clone();
    let active_match = if matches.is_empty() {
        None
    } else {
        Some(find.current)
    };
    let highlight = prefs.highlight;
    let editor_bg = prefs.editor_bg();

    let mut layouter = |ui: &Ui, buffer: &dyn egui::TextBuffer, _wrap: f32| {
        highlighter.galley(
            ui.ctx(),
            id,
            buffer.as_str(),
            language,
            key,
            highlight,
            &matches,
            active_match,
            font.clone(),
            line_height,
            0,
            &hidden,
        )
    };

    let scroll = ScrollArea::both()
        .id_salt(scroll_id(id))
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.horizontal_top(|ui| {
                if gutter_width > 0.0 {
                    // Solo spazio: i numeri vengono dipinti dopo, fuori dallo
                    // scorrimento, così restano fermi a sinistra.
                    ui.allocate_exact_size(egui::vec2(gutter_width, 0.0), Sense::hover());
                }
                let text_edit = TextEdit::multiline(doc.buffer_mut())
                    .id(editor_id(id))
                    .font(TextStyle::Monospace)
                    .code_editor()
                    .lock_focus(true)
                    .desired_width(desired_width)
                    .desired_rows(visible_lines)
                    .background_color(editor_bg)
                    .layouter(&mut layouter);
                text_edit.show(ui)
            })
            .inner
        });

    let output = scroll.inner;
    let response = &output.response.response;
    let changed = response.changed();

    // Menu contestuale: conversioni di maiuscole/minuscole. `TextEdit` di egui
    // non ne ha uno, quindi è qui che si aggiungono le voci. L'azione viene
    // applicata a fine frame (vedi `panel`), quando la selezione è nota.
    let mut actions = Vec::new();
    output.response.response.context_menu(|ui| {
        ui.label(RichText::new("Maiuscole/minuscole").weak());
        for case in ride_core::editor::Case::ALL {
            if ui
                .button(case.label())
                .on_hover_text(format!("nome file → {}", case.example()))
                .clicked()
            {
                actions.push(Action::ConvertCase(case));
                ui.close();
            }
        }
    });

    // Selezione/posizione corrente, in caratteri.
    //
    // `TextEdit` riporta il cursore solo mentre ha il fuoco: quando lo perde
    // (menu contestuale aperto, un altro pannello attivo) il valore è `None`.
    // In quel caso vale l'ultimo stato salvato, altrimenti ogni azione senza
    // fuoco lavorerebbe come se il cursore fosse all'inizio del file.
    let range = output.cursor_range.or_else(|| {
        TextEdit::load_state(&ctx, editor_id(id)).and_then(|state| state.cursor.char_range())
    });
    let (mut caret, mut selection) = match range {
        Some(range) => {
            let caret = range.primary.index.0;
            let from = range.primary.index.0.min(range.secondary.index.0);
            let to = range.primary.index.0.max(range.secondary.index.0);
            (caret, (from, to))
        }
        None => (0, (0, 0)),
    };

    // Il clic destro serve al menu contestuale, ma `TextEdit` non ne ha uno e
    // tratta la pressione come un clic qualsiasi: sposta il cursore (o azzera
    // la selezione). Qui lo rimettiamo a posto, così la conversione lavora sulla
    // parola indicata e non sulla prima del file:
    // * se il clic cade dentro una selezione, la selezione resta (è così che si
    //   converte un identificatore già selezionato);
    // * altrimenti il cursore va sotto il puntatore.
    let button = egui::PointerButton::Secondary;
    let secondary = ctx.input(|input| {
        input.pointer.button_pressed(button) || input.pointer.button_released(button)
    });
    if secondary && response.hovered() {
        if let Some(pointer) = ctx.pointer_interact_pos() {
            let at = output
                .galley
                .cursor_from_pos(pointer - output.galley_pos)
                .index
                .0;
            if selection.0 < selection.1 && selection.0 <= at && at <= selection.1 {
                select(&ctx, id, selection.0, selection.1);
            } else {
                caret = at;
                selection = (at, at);
                select(&ctx, id, at, at);
            }
        }
    }

    // Il cursore non deve restare dentro un blocco piegato: lì il testo non si
    // vede (né si può modificare sensatamente). Lo si riporta all'inizio.
    if !hidden.is_empty() {
        if let Some(clamped) = visible_char(&lines, doc.text(), caret, &hidden) {
            caret = clamped;
            selection = (clamped, clamped);
            select(&ctx, id, clamped, clamped);
        }
    }

    // `TextEdit` ha già inserito il carattere digitato: qui aggiungiamo la
    // chiusura (o scavalchiamo quella già presente). `net_changed` dice se il
    // documento è davvero cambiato; `stale` se la galley appena costruita non
    // riflette più il testo (serve un giro in più per ridisegnarlo).
    let mut net_changed = changed;
    let mut stale = false;

    if changed && pre_empty && prefs.auto_brackets {
        if let Some(typed) = typed {
            match ride_core::editor::auto_pair(next_char, typed) {
                AutoPair::Close => {
                    if let Some(close) = ride_core::editor::closing_bracket(typed) {
                        let at = ride_core::editor::byte_of_char(doc.text(), caret);
                        doc.buffer_mut().insert(at, close);
                        doc.mark_edited();
                        stale = true;
                    }
                }
                AutoPair::Skip => {
                    if caret > 0 {
                        let from = ride_core::editor::byte_of_char(doc.text(), caret - 1);
                        let to = ride_core::editor::byte_of_char(doc.text(), caret);
                        doc.buffer_mut().replace_range(from..to, "");
                        // Il cursore non si sposta: nella stringa senza il
                        // carattere appena inserito si trova già *dopo* la
                        // chiusura che era presente.
                        selection = (caret, caret);
                        select(&ctx, id, caret, caret);
                        // Il testo torna com'era: non è una vera modifica.
                        if !enter_pressed && !tab_pressed {
                            net_changed = false;
                        }
                        stale = true;
                    }
                }
                AutoPair::Plain => {}
            }
        }
    }

    // Indentazione automatica: `TextEdit` inserisce solo il carattere di a capo.
    if changed && enter_pressed {
        if let Some(new_caret) = auto_indent(&ctx, id, doc, language, caret, line_height) {
            caret = new_caret;
            selection = (new_caret, new_caret);
            stale = true;
        }
    } else if changed && tab_pressed {
        if let Some(new_caret) = expand_tab(&ctx, id, doc, language, caret) {
            caret = new_caret;
            selection = (new_caret, new_caret);
            stale = true;
        }
    }

    if stale {
        // Il testo è cambiato dopo l'impaginazione: ridisegna subito.
        ctx.request_repaint();
    }

    if gutter_width > 0.0 {
        let clicked = draw_gutter(
            ui,
            id,
            prefs,
            &font,
            line_height,
            gutter_width,
            scroll.inner_rect,
            output.galley_pos.y,
            cursor_line,
            &output.galley,
            &blocks,
            &folds,
            &symbols,
        );
        if let Some(open_byte) = clicked {
            doc.toggle_fold(open_byte);
            ctx.request_repaint();
        }
    }

    // La coppia di parentesi sotto il cursore (solo se la galley è ancora
    // valida: dopo una modifica la ridisegniamo al frame successivo).
    if prefs.match_brackets && !stale {
        if let Some((at, mate)) = ride_core::editor::bracket_pair(doc.text(), caret) {
            draw_bracket_pair(
                ui,
                &output.galley,
                output.galley_pos,
                scroll.inner_rect,
                at,
                mate,
            );
        }
    }

    // Le sottolineature della diagnostica vanno sopra il testo ma dentro la
    // finestra di scorrimento, come i numeri di riga.
    if !diagnostics.is_empty() {
        draw_diagnostics(
            ui,
            &output.galley,
            output.galley_pos,
            scroll.inner_rect,
            diagnostics,
            &hidden,
        );
    }

    EditorOutput {
        caret,
        selection,
        changed: net_changed,
        actions,
    }
}

/// Il carattere digitato in questo frame, se ne è stato digitato **uno solo**.
///
/// Serve a decidere se chiudere automaticamente una parentesi: con più di un
/// carattere (incolla, IME, …) non tocchiamo nulla.
fn single_typed_char(ui: &Ui) -> Option<char> {
    ui.input(|i| {
        let mut chars = i
            .events
            .iter()
            .filter_map(|event| match event {
                egui::Event::Text(text) => Some(text.chars()),
                _ => None,
            })
            .flatten();
        let first = chars.next()?;
        if chars.next().is_some() { None } else { Some(first) }
    })
}

/// Evidenzia la parentesi sotto il cursore e la sua gemella.
fn draw_bracket_pair(
    ui: &Ui,
    galley: &Galley,
    origin: Pos2,
    viewport: Rect,
    at: usize,
    mate: Option<usize>,
) {
    let painter = ui.painter().with_clip_rect(viewport);
    let matched = Color32::from_rgb(0x7a, 0xb0, 0xe6);
    let unmatched = Color32::from_rgb(0xe0, 0x6c, 0x6c);
    let fill = Color32::from_rgba_unmultiplied(0x2c, 0x5a, 0x8c, 130);

    let draw_one = |index: usize, color: Color32, with_fill: bool| {
        let Some(rect) = char_box(galley, origin, index) else {
            return;
        };
        // Le righe di un blocco piegato hanno altezza zero: niente da segnare.
        if rect.height() <= 0.0 {
            return;
        }
        let rect = rect.expand2(egui::vec2(1.5, 0.0));
        if with_fill {
            painter.rect_filled(rect, 2.0, fill);
        }
        painter.rect_stroke(rect, 2.0, Stroke::new(1.0, color), StrokeKind::Inside);
    };

    match mate {
        Some(m) => {
            draw_one(at, matched, true);
            draw_one(m, matched, true);
        }
        // Parentesi senza gemella: segnalata in rosso.
        None => draw_one(at, unmatched, false),
    }
}

/// Il rettangolo (in coordinate schermo) del carattere `index` di una galley.
fn char_box(galley: &Galley, origin: Pos2, index: usize) -> Option<Rect> {
    if galley.rows.is_empty() {
        return None;
    }
    let start = galley.layout_from_cursor(CCursor::new(index));
    let end = galley.layout_from_cursor(CCursor::new(index + 1));
    let row = galley.rows.get(start.row)?;
    let x0 = row.pos.x + row.x_offset(start.column);
    let x1 = if end.row == start.row {
        row.pos.x + row.x_offset(end.column)
    } else {
        row.rect().right()
    };
    Some(Rect::from_min_max(pos2(x0, row.min_y()), pos2(x1, row.max_y())).translate(origin.to_vec2()))
}

/// Se il carattere `caret` cade in una riga nascosta (blocco piegato),
/// restituisce l'indice di carattere a cui riportarlo — l'inizio del blocco.
fn visible_char(lines: &Lines, text: &str, caret: usize, hidden: &[Range<usize>]) -> Option<usize> {
    let (line, _) = lines.line_col_of_char(text, caret);
    let range = hidden.iter().find(|range| range.contains(&line))?;
    let byte = lines.start(range.start)?;
    Some(ride_core::editor::char_of_byte(text, byte))
}

/// Sottolinea le diagnostiche del language server sotto il testo interessato.
///
/// Gli intervalli sono in indici di carattere; `Galley::layout_from_cursor`
/// traduce una posizione in (riga, colonna) della galley impaginata, così la
/// sottolineatura segue esattamente il testo (anche per caratteri non ASCII).
fn draw_diagnostics(
    ui: &Ui,
    galley: &Galley,
    origin: Pos2,
    viewport: Rect,
    diagnostics: &[Diagnostic],
    hidden: &[Range<usize>],
) {
    if diagnostics.is_empty() || galley.rows.is_empty() {
        return;
    }
    let painter = ui.painter().with_clip_rect(viewport);
    let last_row = galley.rows.len() - 1;
    for diagnostic in diagnostics {
        let (r, g, b) = diagnostic.severity.rgb();
        let color = Color32::from_rgb(r, g, b);
        let start = galley.layout_from_cursor(CCursor::new(diagnostic.start));
        let end = galley.layout_from_cursor(CCursor::new(diagnostic.end.max(diagnostic.start + 1)));
        // Una diagnostica può coprire più righe: la spezziamo in segmenti.
        for row_index in start.row..=end.row.min(last_row) {
            // Le righe di un blocco piegato non si vedono: niente sottolineatura.
            if hidden.iter().any(|range| range.contains(&row_index)) {
                continue;
            }
            let row = &galley.rows[row_index];
            let left = if row_index == start.row {
                row.pos.x + row.x_offset(start.column)
            } else {
                row.rect().left()
            };
            let right = if row_index == end.row {
                row.pos.x + row.x_offset(end.column)
            } else {
                row.rect().right()
            };
            if right <= left {
                continue;
            }
            let y = origin.y + row.max_y() - 1.5;
            painter.line_segment(
                [pos2(origin.x + left, y), pos2(origin.x + right, y)],
                Stroke::new(1.5, color),
            );
        }
    }
}

/// Disegna il canale dei numeri di riga (fermo a sinistra), i marcatori di piega
/// dei blocchi `{ … }` e la `M` che segna l'inizio di un metodo o di una funzione.
///
/// Restituisce l'offset della `{` del blocco su cui si è cliccato, se è stato
/// cliccato un marcatore: chi chiama decide se piegarlo o riaprirlo.
#[allow(clippy::too_many_arguments)]
fn draw_gutter(
    ui: &Ui,
    id: DocumentId,
    prefs: &Prefs,
    font: &egui::FontId,
    line_height: f32,
    gutter_width: f32,
    viewport: Rect,
    text_top: f32,
    cursor_line: usize,
    galley: &Galley,
    blocks: &[FoldBlock],
    folds: &[usize],
    symbols: &[FunctionSymbol],
) -> Option<usize> {
    if line_height <= 0.0 || gutter_width <= 0.0 || galley.rows.is_empty() {
        return None;
    }
    let painter = ui.painter().with_clip_rect(viewport);

    // Sfondo del canale: copre il codice che scorre sotto di esso.
    let background = Rect::from_min_max(
        pos2(viewport.left(), viewport.top()),
        pos2(viewport.left() + gutter_width, viewport.bottom()),
    );
    painter.rect_filled(background, 0.0, prefs.gutter_bg());
    painter.vline(
        background.right(),
        viewport.top()..=viewport.bottom(),
        Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color),
    );

    let number_right = background.right() - GUTTER_PAD - FOLD_MARKER_WIDTH;
    let marker_x = background.right() - GUTTER_PAD - FOLD_MARKER_WIDTH * 0.5;
    let marker_left = background.right() - GUTTER_PAD - FOLD_MARKER_WIDTH;
    // La `M` sta a sinistra dei numeri.
    let symbol_center = background.left() + GUTTER_PAD + SYMBOL_MARKER_WIDTH * 0.5;
    let symbol_right = background.left() + GUTTER_PAD + SYMBOL_MARKER_WIDTH;

    // Le righe dei blocchi piegati hanno altezza zero (vedi `highlight::compact`)
    // e si saltano: le posizioni verticali vengono dalla galley, così i numeri
    // restano allineati al testo anche con la piegatura attiva.
    let local_top = viewport.top() - text_top;
    let local_bottom = viewport.bottom() - text_top;
    let start = galley
        .rows
        .partition_point(|placed| placed.pos.y + placed.row.size.y < local_top);

    let mut markers: Vec<(usize, usize)> = Vec::new(); // (riga, offset della `{`)
    for (doc_line, placed) in galley.rows.iter().enumerate().skip(start) {
        let row_height = placed.row.size.y;
        if row_height <= 0.0 {
            continue; // riga nascosta
        }
        if placed.pos.y > local_bottom {
            break;
        }
        let y = text_top + placed.pos.y;
        let color = if doc_line == cursor_line {
            prefs.gutter_active()
        } else {
            prefs.gutter_text()
        };
        painter.text(
            pos2(number_right, y),
            Align2::RIGHT_TOP,
            (doc_line + 1).to_string(),
            font.clone(),
            color,
        );

        // Il marcatore di piega, se questa riga apre un blocco. `▾` = aperto,
        // `▸` = piegato. Un blocco il cui inizio è nascosto non compare: la sua
        // riga non esiste più.
        if let Ok(index) = blocks.binary_search_by_key(&doc_line, |block| block.open_line) {
            let open_byte = blocks[index].open_byte;
            let folded = folds.binary_search(&open_byte).is_ok();
            painter.text(
                pos2(marker_x, y + row_height * 0.5),
                Align2::CENTER_CENTER,
                if folded { "▸" } else { "▾" },
                font.clone(),
                if folded {
                    prefs.gutter_active()
                } else {
                    prefs.gutter_text()
                },
            );
            markers.push((doc_line, open_byte));
        }

        // La `M` all'inizio di un metodo o di una funzione. Il nome compare in
        // un popup al passaggio del mouse (vedi sotto).
        if symbols
            .binary_search_by_key(&doc_line, |symbol| symbol.line)
            .is_ok()
        {
            painter.text(
                pos2(symbol_center, y + row_height * 0.5),
                Align2::CENTER_CENTER,
                "M",
                font.clone(),
                prefs.symbol_marker(),
            );
        }
    }

    // Il clic sui marcatori: l'intera striscia del canale è sensibile, ma si
    // agisce solo nella colonna dei marcatori.
    let strip = Rect::from_min_max(
        pos2(viewport.left(), viewport.top()),
        pos2(viewport.left() + gutter_width, viewport.bottom()),
    );
    let response = ui.interact(strip, Id::unique(("ride-gutter", id)), Sense::click());
    if response.clicked() {
        if let Some(pos) = response.interact_pointer_pos() {
            if pos.x >= marker_left {
                let local = pos.y - text_top;
                if let Some(&(_, open_byte)) = markers.iter().find(|(line, _)| {
                    galley.rows.get(*line).is_some_and(|placed| {
                        local >= placed.pos.y && local < placed.pos.y + placed.row.size.y
                    })
                }) {
                    return Some(open_byte);
                }
            }
        }
    }

    // Il nome del metodo/funzione, quando il mouse è sulla `M`.
    if let Some(pos) = response.hover_pos() {
        if pos.x <= symbol_right {
            let local = pos.y - text_top;
            let hovered = galley
                .rows
                .iter()
                .enumerate()
                .find(|(_, placed)| {
                    placed.row.size.y > 0.0
                        && local >= placed.pos.y
                        && local < placed.pos.y + placed.row.size.y
                })
                .and_then(|(doc_line, _)| {
                    symbols
                        .binary_search_by_key(&doc_line, |symbol| symbol.line)
                        .ok()
                        .map(|index| &symbols[index])
                });
            if let Some(symbol) = hovered {
                response.clone().on_hover_ui_at_pointer(|ui| {
                    ui.label(RichText::new(&symbol.name).strong());
                });
            }
        }
    }
    None
}

/// Inserisce l'indentazione (ed eventualmente apre un blocco) dopo un `Invio`.
///
/// Il *cosa* fare è deciso da [`ride_core::editor::enter_edit`]; qui si applica
/// al documento. Restituisce la nuova posizione del cursore, se il testo è
/// cambiato.
fn auto_indent(
    ctx: &egui::Context,
    id: DocumentId,
    doc: &mut Document,
    language: Language,
    caret: usize,
    _line_height: f32,
) -> Option<usize> {
    let unit = language.indent().unit();
    let plan = {
        let text = doc.text();
        ride_core::editor::enter_edit(text, caret, unit)
    };

    let new_caret = match plan {
        EnterEdit::None => return None,
        EnterEdit::Indent(insert) => {
            let at = ride_core::editor::byte_of_char(doc.text(), caret);
            doc.buffer_mut().insert_str(at, &insert);
            caret + insert.chars().count()
        }
        EnterEdit::Block {
            inner,
            close_indent,
        } => {
            let at = ride_core::editor::byte_of_char(doc.text(), caret);
            doc.buffer_mut().insert_str(at, &inner);
            doc.buffer_mut()
                .insert_str(at + inner.len(), &format!("\n{close_indent}"));
            caret + inner.chars().count()
        }
    };

    doc.mark_edited();
    select(ctx, id, new_caret, new_caret);
    // Il testo è cambiato *dopo* che `TextEdit` ha impaginato: serve un giro in
    // più perché l'indentazione appena inserita venga disegnata.
    ctx.request_repaint();
    Some(new_caret)
}

/// Sostituisce il `\t` appena inserito con l'unità di indentazione del linguaggio.
fn expand_tab(
    ctx: &egui::Context,
    id: DocumentId,
    doc: &mut Document,
    language: Language,
    caret: usize,
) -> Option<usize> {
    let unit = language.indent().unit();
    if unit == "\t" || caret == 0 {
        return None;
    }
    let from = {
        let text = doc.text();
        let at = ride_core::editor::byte_of_char(text, caret);
        if at == 0 || &text[at - 1..at] != "\t" {
            return None;
        }
        at - 1
    };
    doc.buffer_mut().replace_range(from..from + 1, unit);
    doc.mark_edited();
    let caret = caret + unit.chars().count() - 1;
    select(ctx, id, caret, caret);
    ctx.request_repaint();
    Some(caret)
}
