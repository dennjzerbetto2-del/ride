//! Il pannello del terminale: intestazione con i comandi e schermo VT100.
//!
//! Il disegno è volutamente a "celle": l'emulatore (`vt100`) fornisce i
//! caratteri e i colori, qui si raggruppano le celle consecutive con lo stesso
//! stile per non costruire una galley per ogni singolo carattere.

use std::time::{Duration, Instant};

use egui::{
    Align, Align2, Color32, FontId, Key, Layout, Pos2, Rect, RichText, Sense, Stroke, StrokeKind,
    Ui, ViewportCommand, pos2, vec2,
};

use crate::action::Action;
use crate::app::RideApp;
use crate::terminal::{TerminalState, key_bytes};

/// Disegna il terminale agganciato in fondo alla finestra.
pub fn panel(ui: &mut Ui, app: &mut RideApp) -> Vec<Action> {
    let mut actions = Vec::new();
    let panel_rect = ui.max_rect();

    header(ui, app, &mut actions);
    screen(ui, app, &mut actions, panel_rect);

    actions
}

fn header(ui: &mut Ui, app: &mut RideApp, actions: &mut Vec<Action>) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("Terminale").strong());
        if !app.terminal.shell().is_empty() {
            ui.label(RichText::new(app.terminal.shell()).weak().monospace())
                .on_hover_text("Shell del terminale");
        }
        let cwd = app.terminal.cwd();
        let folder = cwd
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| cwd.display().to_string());
        ui.label(RichText::new(format!("· {folder}")).weak())
            .on_hover_text(cwd.display().to_string());
        if let Some(error) = app.terminal.error() {
            ui.colored_label(Color32::from_rgb(0xe0, 0x7a, 0x7a), error);
        } else if let Some(exit) = app.terminal.exit_message() {
            if !app.terminal.is_running() {
                ui.label(RichText::new(exit).weak());
            }
        }

        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui
                .button("✕")
                .on_hover_text("Nascondi il terminale (Ctrl+`)")
                .clicked()
            {
                actions.push(Action::HideTerminal);
            }
            if ui
                .button("⟳")
                .on_hover_text("Riavvia lo shell nella cartella del progetto")
                .clicked()
            {
                actions.push(Action::RestartTerminal);
            }
            let auto_hide = app.terminal.auto_hide;
            if ui
                .selectable_label(auto_hide, "Autoscomparsa")
                .on_hover_text("Nasconde il terminale quando perde il fuoco; Ctrl+` lo riapre")
                .clicked()
            {
                actions.push(Action::ToggleTerminalAutoHide);
            }
        });
    });
}

fn screen(ui: &mut Ui, app: &mut RideApp, actions: &mut Vec<Action>, panel_rect: Rect) {
    let font_size = app.prefs.font_size;
    let dark = app.prefs.dark;
    let font = FontId::monospace(font_size);
    let (cell_width, row_height) = ui.ctx().fonts_mut(|fonts| {
        (
            fonts.glyph_width(&font, 'M').max(1.0),
            fonts.row_height(&font).max(1.0),
        )
    });

    let (_, rect) = ui.allocate_space(ui.available_size());
    let response = ui.interact(
        rect,
        ui.make_persistent_id("ride-terminal-screen"),
        Sense::click_and_drag(),
    );
    if response.clicked() {
        response.request_focus();
    }

    let terminal = &mut app.terminal;
    if terminal.take_focus_request() {
        response.request_focus();
    }
    let focused = response.has_focus();
    terminal.set_focused(focused, Instant::now());

    let cols = (rect.width() / cell_width).floor().max(1.0) as u16;
    let rows = (rect.height() / row_height).floor().max(1.0) as u16;
    terminal.resize(rows, cols);

    let palette = Palette::new(dark);
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, palette.background);

    // Scorrimento con la rotella.
    if response.hovered() {
        let delta = ui.input(|input| input.smooth_scroll_delta.y);
        if delta.abs() > 0.5 {
            let lines = (delta / row_height).round() as i32;
            if lines != 0 {
                terminal.scroll(lines);
            }
        }
    }

    paint_screen(
        &painter, rect, terminal, &font, cell_width, row_height, rows, cols, &palette,
    );

    if focused {
        painter.rect_stroke(
            rect,
            0.0,
            Stroke::new(1.0, ui.visuals().selection.stroke.color),
            StrokeKind::Inside,
        );
    }

    if focused {
        handle_input(ui, terminal, actions);
    }

    let pointer_inside = ui
        .ctx()
        .pointer_hover_pos()
        .is_some_and(|position| panel_rect.contains(position));
    let pointer_down = ui.input(|input| input.pointer.any_down());
    if terminal.should_auto_hide(Instant::now(), pointer_inside, pointer_down) {
        actions.push(Action::HideTerminal);
    } else if terminal.auto_hide_pending() && !pointer_inside {
        // L'autoscomparsa ha una scadenza: senza un repaint il pannello
        // resterebbe aperto finché non arriva un altro evento.
        ui.ctx().request_repaint_after(Duration::from_millis(100));
    }
}

/// Tasti, testo e appunti vanno al processo.
fn handle_input(ui: &Ui, terminal: &mut TerminalState, actions: &mut Vec<Action>) {
    let events = ui.input(|input| input.events.clone());
    let application_cursor = terminal.screen().application_cursor();

    for event in events {
        match event {
            egui::Event::Text(text) => terminal.send_bytes(text.as_bytes()),
            egui::Event::Paste(text) => terminal.paste(&text),
            egui::Event::Cut => terminal.send_bytes(&[0x18]),
            egui::Event::Copy => {
                // Su Linux/Windows Ctrl+C arriva come "Copy": in un terminale
                // deve mandare SIGINT. Il testo si copia con Ctrl+Shift+C.
                #[cfg(not(target_os = "macos"))]
                terminal.send_bytes(&[0x03]);
                #[cfg(target_os = "macos")]
                ui.ctx().copy_text(terminal.screen().contents());
            }
            egui::Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } => {
                if modifiers.command && modifiers.shift && key == Key::C {
                    ui.ctx().copy_text(terminal.screen().contents());
                    continue;
                }
                if modifiers.command && modifiers.shift && key == Key::V {
                    ui.ctx().send_viewport_cmd(ViewportCommand::RequestPaste);
                    continue;
                }
                if modifiers.shift && key == Key::PageUp {
                    terminal.scroll(10);
                    continue;
                }
                if modifiers.shift && key == Key::PageDown {
                    terminal.scroll(-10);
                    continue;
                }
                if !terminal.is_running() && key == Key::Enter {
                    actions.push(Action::RestartTerminal);
                    continue;
                }
                if let Some(bytes) = key_bytes(key, modifiers, application_cursor) {
                    terminal.send_bytes(&bytes);
                }
            }
            _ => {}
        }
    }
}

/// Colori del terminale.
struct Palette {
    background: Color32,
    foreground: Color32,
    light: bool,
}

impl Palette {
    fn new(dark: bool) -> Self {
        Palette {
            background: if dark {
                Color32::from_rgb(0x0e, 0x12, 0x16)
            } else {
                Color32::from_rgb(0xfb, 0xfb, 0xf8)
            },
            foreground: if dark {
                Color32::from_rgb(0xd8, 0xde, 0xe6)
            } else {
                Color32::from_rgb(0x20, 0x24, 0x28)
            },
            light: !dark,
        }
    }

    fn color(&self, color: vt100::Color, default: Color32, bold: bool) -> Color32 {
        match color {
            vt100::Color::Default => default,
            vt100::Color::Rgb(red, green, blue) => Color32::from_rgb(red, green, blue),
            vt100::Color::Idx(index) => {
                let index = if bold && index < 8 { index + 8 } else { index };
                ansi(index, self.light)
            }
        }
    }
}

/// I 256 colori ANSI, con qualche aggiustamento per il tema chiaro.
fn ansi(index: u8, light: bool) -> Color32 {
    const BASE: [(u8, u8, u8); 16] = [
        (0x00, 0x00, 0x00),
        (0x80, 0x00, 0x00),
        (0x00, 0x80, 0x00),
        (0x80, 0x80, 0x00),
        (0x00, 0x00, 0x80),
        (0x80, 0x00, 0x80),
        (0x00, 0x80, 0x80),
        (0xc0, 0xc0, 0xc0),
        (0x80, 0x80, 0x80),
        (0xff, 0x00, 0x00),
        (0x00, 0xff, 0x00),
        (0xff, 0xff, 0x00),
        (0x00, 0x00, 0xff),
        (0xff, 0x00, 0xff),
        (0x00, 0xff, 0xff),
        (0xff, 0xff, 0xff),
    ];

    if light {
        // Il bianco e il grigio chiaro sparirebbero sullo sfondo chiaro.
        match index {
            7 => return Color32::from_rgb(0x3b, 0x40, 0x48),
            8 => return Color32::from_rgb(0x6b, 0x72, 0x80),
            15 => return Color32::from_rgb(0x1f, 0x23, 0x28),
            _ => {}
        }
    }

    let (red, green, blue) = if (index as usize) < BASE.len() {
        BASE[index as usize]
    } else if index < 232 {
        // Cubo 6×6×6.
        let index = index - 16;
        let levels = [0, 95, 135, 175, 215, 255];
        (
            levels[(index / 36) as usize],
            levels[((index % 36) / 6) as usize],
            levels[(index % 6) as usize],
        )
    } else {
        // Scala di grigi.
        let level = 8 + (index - 232) * 10;
        (level, level, level)
    };
    Color32::from_rgb(red, green, blue)
}

#[allow(clippy::too_many_arguments)]
fn paint_screen(
    painter: &egui::Painter,
    rect: Rect,
    terminal: &TerminalState,
    font: &FontId,
    cell_width: f32,
    row_height: f32,
    rows: u16,
    cols: u16,
    palette: &Palette,
) {
    let screen = terminal.screen();

    for row in 0..rows {
        let y = rect.top() + row as f32 * row_height;
        let mut col = 0;
        while col < cols {
            let Some(cell) = screen.cell(row, col) else {
                break;
            };
            let (fg, bg, underline) = style(cell, palette);
            let start = col;
            let mut text = String::new();

            while col < cols {
                let Some(cell) = screen.cell(row, col) else {
                    break;
                };
                if style(cell, palette) != (fg, bg, underline) {
                    break;
                }
                if cell.is_wide_continuation() {
                    col += 1;
                    continue;
                }
                let contents = cell.contents();
                if contents.is_empty() {
                    text.push(' ');
                } else {
                    text.push_str(contents);
                }
                col += 1;
            }

            let x = rect.left() + start as f32 * cell_width;
            let width = (col - start) as f32 * cell_width;
            if bg != palette.background {
                painter.rect_filled(
                    Rect::from_min_size(pos2(x, y), vec2(width, row_height)),
                    0.0,
                    bg,
                );
            }
            if !text.trim().is_empty() {
                painter.text(pos2(x, y), Align2::LEFT_TOP, text, font.clone(), fg);
            }
            if underline {
                painter.line_segment(
                    [
                        pos2(x, y + row_height - 1.0),
                        pos2(x + width, y + row_height - 1.0),
                    ],
                    Stroke::new(1.0, fg),
                );
            }
        }
    }

    // Cursore: un blocco, ridisegnando il carattere sotto di esso nel colore di
    // sfondo.
    let (cursor_row, cursor_col) = screen.cursor_position();
    let show_cursor = terminal.focused
        && terminal.scroll_offset() == 0
        && !screen.hide_cursor()
        && cursor_row < rows
        && cursor_col < cols;
    if show_cursor {
        let position = pos2(
            rect.left() + cursor_col as f32 * cell_width,
            rect.top() + cursor_row as f32 * row_height,
        );
        painter.rect_filled(
            Rect::from_min_size(position, vec2(cell_width, row_height)),
            0.0,
            palette.foreground,
        );
        if let Some(cell) = screen.cell(cursor_row, cursor_col) {
            let contents = cell.contents();
            let text = if contents.is_empty() { " " } else { contents };
            painter.text(
                position,
                Align2::LEFT_TOP,
                text,
                font.clone(),
                palette.background,
            );
        }
    }

    // Posizione nello scorrimento, quando non si è al fondo.
    if terminal.scroll_offset() > 0 {
        painter.text(
            pos2(rect.right() - 6.0, rect.top() + 4.0),
            Align2::RIGHT_TOP,
            format!("↥ {}", terminal.scroll_offset()),
            FontId::proportional(11.0),
            palette.foreground,
        );
    }
}

fn style(cell: &vt100::Cell, palette: &Palette) -> (Color32, Color32, bool) {
    let mut fg = palette.color(cell.fgcolor(), palette.foreground, cell.bold());
    let mut bg = palette.color(cell.bgcolor(), palette.background, false);
    if cell.inverse() {
        std::mem::swap(&mut fg, &mut bg);
    }
    (fg, bg, cell.underline())
}

/// Posizione del testo in una cella (usata dai test del disegno).
#[allow(dead_code)]
fn cell_position(rect: Rect, col: u16, row: u16, cell_width: f32, row_height: f32) -> Pos2 {
    pos2(
        rect.left() + col as f32 * cell_width,
        rect.top() + row as f32 * row_height,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn i_colori_ansi_coprono_i_256_indici() {
        assert_eq!(ansi(1, false), Color32::from_rgb(0x80, 0, 0));
        assert_eq!(ansi(196, false), Color32::from_rgb(0xff, 0, 0));
        assert_eq!(ansi(232, false), Color32::from_rgb(8, 8, 8));
        assert_eq!(ansi(255, false), Color32::from_rgb(238, 238, 238));
        // Sul tema chiaro il bianco diventa scuro, altrimenti sarebbe invisibile.
        assert_eq!(ansi(15, true), Color32::from_rgb(0x1f, 0x23, 0x28));
    }

    #[test]
    fn la_posizione_di_una_cella_e_calcolata_dal_pannello() {
        let rect = Rect::from_min_size(pos2(10.0, 20.0), vec2(200.0, 100.0));
        assert_eq!(cell_position(rect, 0, 0, 8.0, 16.0), pos2(10.0, 20.0));
        assert_eq!(cell_position(rect, 2, 3, 8.0, 16.0), pos2(26.0, 68.0));
    }
}
