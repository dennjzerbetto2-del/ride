//! Preferenze dell'interfaccia e applicazione dello stile a egui.

use egui::{Color32, Context, FontFamily, FontId, TextStyle, Theme, Visuals};

/// Preferenze dell'utente.
///
/// Sono tutte cose che si possono cambiare a runtime dal menu *Visualizza*:
/// `apply` le traduce nello stile di egui, e viene rieseguito solo quando
/// qualcosa cambia (`dirty`), non a ogni frame.
#[derive(Debug, Clone)]
pub struct Prefs {
    /// Tema scuro.
    pub dark: bool,
    /// Dimensione del font del codice.
    pub font_size: f32,
    /// Mostra il pannello del progetto.
    pub show_tree: bool,
    /// Mostra il pannello di ricerca nel progetto.
    pub show_search_panel: bool,
    /// Mostra i numeri di riga.
    pub line_numbers: bool,
    /// Evidenziazione della sintassi attiva.
    pub highlight: bool,
    /// Chiude automaticamente parentesi e graffe.
    pub auto_brackets: bool,
    /// Evidenzia la coppia di parentesi sotto il cursore.
    pub match_brackets: bool,
    /// Piegatura dei blocchi `{ … }` dal canale dei numeri di riga.
    pub folding: bool,
    /// Avvia automaticamente il language server del linguaggio attivo.
    pub lsp: bool,
    /// Tema `syntect` (nome), `None` = automatico in base a `dark`.
    pub syntax_theme: Option<String>,
    /// Mostra i file nascosti nell'albero.
    pub show_hidden: bool,
    /// Rispetta `.gitignore` nell'albero e nella ricerca.
    pub respect_gitignore: bool,
    /// Impostazioni modificate e non ancora applicate allo stile.
    dirty: bool,
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs {
            dark: true,
            font_size: 14.0,
            show_tree: true,
            show_search_panel: false,
            line_numbers: true,
            highlight: true,
            auto_brackets: true,
            match_brackets: true,
            folding: true,
            lsp: true,
            syntax_theme: None,
            show_hidden: false,
            respect_gitignore: true,
            dirty: true,
        }
    }
}

impl Prefs {
    /// Segnala che le preferenze sono cambiate.
    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    /// Il font del codice.
    pub fn code_font(&self) -> FontId {
        FontId::new(self.font_size, FontFamily::Monospace)
    }

    /// Colore di sfondo dell'area di editing.
    pub fn editor_bg(&self) -> Color32 {
        if self.dark {
            Color32::from_rgb(0x16, 0x19, 0x1d)
        } else {
            Color32::WHITE
        }
    }

    /// Colore del "canale" dei numeri di riga.
    pub fn gutter_bg(&self) -> Color32 {
        if self.dark {
            Color32::from_rgb(0x12, 0x14, 0x18)
        } else {
            Color32::from_rgb(0xf2, 0xf3, 0xf5)
        }
    }

    /// Colore del testo dei numeri di riga.
    pub fn gutter_text(&self) -> Color32 {
        if self.dark {
            Color32::from_rgb(0x50, 0x5a, 0x66)
        } else {
            Color32::from_rgb(0xa0, 0xa6, 0xad)
        }
    }

    /// Colore del numero della riga del cursore.
    pub fn gutter_active(&self) -> Color32 {
        if self.dark {
            Color32::from_rgb(0xc8, 0xd0, 0xd8)
        } else {
            Color32::from_rgb(0x40, 0x46, 0x4d)
        }
    }

    /// Applica le preferenze allo stile di egui, se serve.
    ///
    /// `egui` 0.37 tiene *due* stili, uno per tema: qui li aggiorniamo entrambi e
    /// poi scegliamo quello attivo con `set_theme`, così il passaggio
    /// chiaro/scuro non richiede di riapplicare lo stile a ogni frame.
    pub fn apply(&mut self, ctx: &Context) {
        if !self.dirty {
            return;
        }
        self.dirty = false;

        let mut dark = Visuals::dark();
        dark.panel_fill = Color32::from_rgb(0x1b, 0x1f, 0x24);
        dark.window_fill = Color32::from_rgb(0x22, 0x27, 0x2d);
        dark.extreme_bg_color = Color32::from_rgb(0x16, 0x19, 0x1d);
        dark.faint_bg_color = Color32::from_rgb(0x24, 0x29, 0x30);
        dark.hyperlink_color = Color32::from_rgb(0x6d, 0xa8, 0xe0);
        dark.selection.bg_fill = Color32::from_rgb(0x2c, 0x5a, 0x8c);
        // Lo sfondo del codice lo disegniamo noi (vedi `ui::editor`).
        dark.code_bg_color = Color32::TRANSPARENT;

        let mut light = Visuals::light();
        light.panel_fill = Color32::from_rgb(0xef, 0xf1, 0xf4);
        light.window_fill = Color32::WHITE;
        light.extreme_bg_color = Color32::WHITE;
        light.faint_bg_color = Color32::from_rgb(0xe6, 0xe9, 0xec);
        light.hyperlink_color = Color32::from_rgb(0x14, 0x54, 0x9c);
        light.selection.bg_fill = Color32::from_rgb(0xb4, 0xcf, 0xf0);
        light.code_bg_color = Color32::TRANSPARENT;

        ctx.set_visuals_of(Theme::Dark, dark);
        ctx.set_visuals_of(Theme::Light, light);
        ctx.set_theme(if self.dark { Theme::Dark } else { Theme::Light });

        let font_size = self.font_size;
        ctx.all_styles_mut(|style| {
            style.text_styles.insert(
                TextStyle::Monospace,
                FontId::new(font_size, FontFamily::Monospace),
            );
            style.text_styles.insert(
                TextStyle::Body,
                FontId::new((font_size * 0.95).max(11.0), FontFamily::Proportional),
            );
            style.spacing.item_spacing = egui::vec2(6.0, 4.0);
            style.spacing.button_padding = egui::vec2(6.0, 3.0);
            style.spacing.scroll.bar_width = 10.0;
        });
    }

    /// Salva le preferenze nello `storage` di eframe.
    ///
    /// Una chiave per valore: non serve `serde`, ci bastano stringhe.
    pub fn store(&self, storage: &mut dyn eframe::Storage) {
        storage.set_string("ride.dark", self.dark.to_string());
        storage.set_string("ride.font_size", self.font_size.to_string());
        storage.set_string("ride.show_tree", self.show_tree.to_string());
        storage.set_string(
            "ride.show_search_panel",
            self.show_search_panel.to_string(),
        );
        storage.set_string("ride.line_numbers", self.line_numbers.to_string());
        storage.set_string("ride.highlight", self.highlight.to_string());
        storage.set_string("ride.auto_brackets", self.auto_brackets.to_string());
        storage.set_string("ride.match_brackets", self.match_brackets.to_string());
        storage.set_string("ride.folding", self.folding.to_string());
        storage.set_string("ride.lsp", self.lsp.to_string());
        storage.set_string("ride.show_hidden", self.show_hidden.to_string());
        storage.set_string(
            "ride.respect_gitignore",
            self.respect_gitignore.to_string(),
        );
        if let Some(theme) = &self.syntax_theme {
            storage.set_string("ride.syntax_theme", theme.clone());
        } else {
            storage.remove_string("ride.syntax_theme");
        }
    }

    /// Rilegge le preferenze salvate; per le chiavi mancanti restano i valori
    /// predefiniti. Segna lo stile come "da applicare".
    pub fn load(&mut self, storage: &dyn eframe::Storage) {
        let bool_at = |key: &str, slot: &mut bool| {
            if let Some(value) = storage.get_string(key) {
                if let Ok(parsed) = value.parse() {
                    *slot = parsed;
                }
            }
        };
        bool_at("ride.dark", &mut self.dark);
        bool_at("ride.show_tree", &mut self.show_tree);
        bool_at("ride.show_search_panel", &mut self.show_search_panel);
        bool_at("ride.line_numbers", &mut self.line_numbers);
        bool_at("ride.highlight", &mut self.highlight);
        bool_at("ride.auto_brackets", &mut self.auto_brackets);
        bool_at("ride.match_brackets", &mut self.match_brackets);
        bool_at("ride.folding", &mut self.folding);
        bool_at("ride.lsp", &mut self.lsp);
        bool_at("ride.show_hidden", &mut self.show_hidden);
        bool_at("ride.respect_gitignore", &mut self.respect_gitignore);
        if let Some(value) = storage.get_string("ride.font_size") {
            if let Ok(size) = value.parse::<f32>() {
                self.font_size = size.clamp(8.0, 40.0);
            }
        }
        self.syntax_theme = storage.get_string("ride.syntax_theme");
        self.dirty = true;
    }
}
