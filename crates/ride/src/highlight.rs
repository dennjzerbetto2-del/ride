//! Evidenziazione della sintassi e costruzione della "galley" del testo.
//!
//! # Perché una cache
//!
//! `egui` ridisegna da zero a ogni frame, ma il lavoro costoso (tokenizzare il
//! testo con `syntect` e impaginarlo) dipende solo da tre cose: il testo, il
//! tema e la ricerca attiva. La chiave [`HighlightKey`] cattura esattamente
//! questo, quindi finché il documento non cambia si restituisce la galley già
//! pronta (un `Arc`, quindi la copia è gratis).
//!
//! # Perché il layouter è nostro
//!
//! Passando un `layouter` a `TextEdit` decidiamo noi tre cose:
//!
//! 1. **il colore** di ogni token (da `syntect`);
//! 2. **niente a capo automatico** (`wrap.max_width = INFINITY`): nel codice le
//!    righe lunghe si scorrono in orizzontale, non si spezzano;
//! 3. l'altezza di riga, che rende i numeri di riga allineati al pixel con il
//!    testo (vedi `ui::editor`).

use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;

use egui::emath::GuiRounding;
use egui::epaint::text::TextOptions;
use egui::text::LayoutJob;
use egui::{Color32, FontId, Galley, Pos2, Rect, TextFormat};
use ride_core::{DocumentId, Language};
use syntect::easy::HighlightLines;
use syntect::highlighting::{FontStyle, Style, Theme, ThemeSet};
use syntect::parsing::{SyntaxReference, SyntaxSet};
use syntect::util::LinesWithEndings;

/// Oltre questo numero di righe l'evidenziazione viene disattivata: il costo per
/// keystroke crescerebbe troppo. Il testo resta perfettamente editabile.
pub const MAX_HIGHLIGHT_LINES: usize = 30_000;

/// Colori delle corrispondenze della ricerca.
pub const MATCH_BG: Color32 = Color32::from_rgb(92, 84, 44);
/// Colore della corrispondenza su cui si trova il cursore.
pub const ACTIVE_MATCH_BG: Color32 = Color32::from_rgb(150, 120, 40);

/// Tema scuro e chiaro predefiniti.
const DEFAULT_DARK_THEME: &str = "base16-ocean.dark";
const DEFAULT_LIGHT_THEME: &str = "InspiredGitHub";

/// Cosa invalida la cache di una galley.
///
/// Il **testo** non è nella chiave: viene confrontato a parte (è un `memcmp`)
/// perché `TextEdit` può cambiarlo dentro lo stesso frame in cui impagina, prima
/// che il resto dello stato sia aggiornato.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HighlightKey {
    /// Linguaggio del documento (cambia i colori: es. dopo un "salva con nome").
    pub language: Language,
    /// Impronta della ricerca attiva (0 se la barra di ricerca è chiusa).
    pub find: u64,
    /// Impronta dei blocchi piegati (le righe nascoste cambiano l'impaginazione).
    pub folds: u64,
    /// Stile: dimensione del font e interruttore dell'evidenziazione.
    pub style: u64,
}

struct Cached {
    key: HighlightKey,
    width_bucket: u32,
    galley: Arc<Galley>,
}

/// Impronta dell'atlante dei font a cui puntano le galley in cache.
///
/// `egui` **ricostruisce da zero** l'atlante dei font quando cambiano le
/// opzioni di testo (fra cui la conversione dei colori, che dipende dal tema
/// chiaro/scuro) o quando si riempie. Da quel momento i riferimenti dei glifi
/// sono diversi, e una galley tenuta in cache disegnerebbe caratteri sbagliati
/// (non solo del colore sbagliato): è il difetto che si vede cambiando tema.
#[derive(Debug, Clone, Copy, PartialEq)]
struct AtlasStamp {
    options: TextOptions,
    /// Dimensione in pixel dell'immagine dell'atlante.
    size: [usize; 2],
}

/// Motore di evidenziazione + cache delle galley.
pub struct Highlighter {
    syntaxes: SyntaxSet,
    themes: ThemeSet,
    theme_name: String,
    cache: HashMap<DocumentId, Cached>,
    /// Larghezza misurata della galley, usata per decidere la larghezza del
    /// widget al frame successivo (per lo scorrimento orizzontale).
    widths: HashMap<DocumentId, f32>,
    /// Atlante dei font a cui puntano le galley in cache (`None` finché non se
    /// ne costruisce una).
    atlas: Option<AtlasStamp>,
}

impl Highlighter {
    /// Crea il motore con il tema indicato (`None` = tema scuro predefinito).
    pub fn new(theme_name: Option<&str>) -> Self {
        let themes = ThemeSet::load_defaults();
        let theme_name = theme_name
            .filter(|name| themes.themes.contains_key(*name))
            .unwrap_or(DEFAULT_DARK_THEME)
            .to_string();
        Highlighter {
            syntaxes: SyntaxSet::load_defaults_newlines(),
            themes,
            theme_name,
            cache: HashMap::new(),
            widths: HashMap::new(),
            atlas: None,
        }
    }

    /// Nomi dei temi `syntect` disponibili, divisi per chiaro/scuro.
    ///
    /// Si offrono solo i temi che si accordano con lo sfondo dell'editor: un
    /// tema chiaro su sfondo scuro (o viceversa) renderebbe il testo
    /// praticamente invisibile.
    pub fn theme_names_for(&self, dark: bool) -> Vec<String> {
        let mut names: Vec<String> = self
            .themes
            .themes
            .iter()
            .filter(|(_, theme)| theme_is_dark(theme) == dark)
            .map(|(name, _)| name.clone())
            .collect();
        names.sort();
        names
    }

    /// `true` se il tema esiste e si accorda con uno sfondo chiaro o scuro.
    pub fn theme_matches(&self, name: &str, dark: bool) -> bool {
        self.themes
            .themes
            .get(name)
            .is_some_and(|theme| theme_is_dark(theme) == dark)
    }

    pub fn theme_name(&self) -> &str {
        &self.theme_name
    }

    /// Cambia tema e svuota la cache.
    pub fn set_theme(&mut self, name: &str) {
        if self.themes.themes.contains_key(name) && name != self.theme_name {
            self.theme_name = name.to_string();
            self.cache.clear();
        }
    }

    /// Tema chiaro o scuro predefinito, in base al tema dell'interfaccia.
    pub fn set_default_for(&mut self, dark: bool) {
        self.set_theme(default_theme_name(dark));
    }

    /// Il tema `syntect` corrente.
    fn theme(&self) -> &Theme {
        self.themes
            .themes
            .get(&self.theme_name)
            .or_else(|| self.themes.themes.values().next())
            .expect("il set di temi predefinito non è vuoto")
    }

    /// Colore di sfondo predefinito del testo (usato quando l'evidenziazione è
    /// disattivata).
    pub fn default_text_color(&self) -> Color32 {
        self.theme()
            .settings
            .foreground
            .map(|c| rgba(c.r, c.g, c.b, c.a))
            .unwrap_or(Color32::LIGHT_GRAY)
    }

    /// Svuota tutte le cache (tema, font o preferenze cambiate).
    pub fn clear(&mut self) {
        self.cache.clear();
        self.widths.clear();
    }

    /// Dimentica un documento (tab chiusa).
    pub fn forget(&mut self, id: DocumentId) {
        self.cache.remove(&id);
        self.widths.remove(&id);
    }

    /// Larghezza dell'ultima galley costruita per il documento.
    pub fn measured_width(&self, id: DocumentId) -> Option<f32> {
        self.widths.get(&id).copied()
    }

    /// La galley del testo del documento, dalla cache quando possibile.
    ///
    /// `hidden` sono le righe da nascondere (i blocchi piegati, già uniti): le
    /// righe nascoste restano nella galley — così gli indici di carattere non
    /// cambiano mai — ma vengono azzerate in altezza e non disegnate.
    #[allow(clippy::too_many_arguments)]
    pub fn galley(
        &mut self,
        ctx: &egui::Context,
        id: DocumentId,
        text: &str,
        language: Language,
        key: HighlightKey,
        highlight: bool,
        matches: &[(usize, usize)],
        active_match: Option<usize>,
        font: FontId,
        line_height: f32,
        wrap_bucket: u32,
        hidden: &[Range<usize>],
    ) -> Arc<Galley> {
        // Se `egui` ha rifatto l'atlante dei font, le galley in cache non
        // valgono più: prima di usarla, si butta tutto.
        self.forget_stale_atlas(ctx);

        if let Some(cached) = self.cache.get(&id) {
            // Oltre alla chiave confrontiamo *il testo*: `TextEdit` modifica il
            // buffer e ci richiama nello stesso frame, senza che la chiave sia
            // ancora cambiata: da sola non basterebbe ad accorgersi del nuovo
            // testo. Il confronto è un `memcmp`, praticamente gratis.
            if cached.key == key
                && cached.width_bucket == wrap_bucket
                && cached.galley.job.text == text
            {
                return cached.galley.clone();
            }
        }

        let galley = self.build(
            ctx,
            text,
            language,
            &font,
            line_height,
            highlight,
            matches,
            active_match,
            hidden,
        );
        self.widths.insert(id, galley.size().x);
        self.cache.insert(
            id,
            Cached {
                key,
                width_bucket: wrap_bucket,
                galley: galley.clone(),
            },
        );
        galley
    }

    #[allow(clippy::too_many_arguments)]
    fn build(
        &self,
        ctx: &egui::Context,
        text: &str,
        language: Language,
        font: &FontId,
        line_height: f32,
        highlight: bool,
        matches: &[(usize, usize)],
        active_match: Option<usize>,
        hidden: &[Range<usize>],
    ) -> Arc<Galley> {
        let theme = self.theme();
        let base = theme.settings.foreground.unwrap_or(syntect::highlighting::Color::WHITE);
        let default_style = Style {
            foreground: base,
            ..Default::default()
        };

        let syntax = self.syntax_for(language);
        // L'evidenziazione è troppo costosa per file enormi: oltre la soglia il
        // testo resta editabile ma senza colori.
        let use_syntax =
            highlight && syntax.is_some() && text.lines().count() <= MAX_HIGHLIGHT_LINES;

        let mut job = LayoutJob::default();
        // Nessun a capo automatico: le righe lunghe si scorrono.
        job.wrap.max_width = f32::INFINITY;
        job.keep_trailing_whitespace = true;

        let mut highlighter = syntax.map(|syntax| HighlightLines::new(syntax, theme));
        let mut char_pos = 0usize;

        for line in LinesWithEndings::from(text) {
            let ranges = match highlighter.as_mut() {
                Some(hl) => match hl.highlight_line(line, &self.syntaxes) {
                    Ok(ranges) => ranges,
                    // Una sintassi "rotta" non deve far perdere il testo.
                    Err(_) => vec![(default_style, line)],
                },
                None => vec![(default_style, line)],
            };

            for (style, piece) in ranges {
                let start = char_pos;
                char_pos += piece.chars().count();

                let mut format = TextFormat {
                    font_id: font.clone(),
                    color: if use_syntax {
                        rgba(
                            style.foreground.r,
                            style.foreground.g,
                            style.foreground.b,
                            style.foreground.a,
                        )
                    } else {
                        rgba(base.r, base.g, base.b, base.a)
                    },
                    italics: style.font_style.contains(FontStyle::ITALIC),
                    underline: if style.font_style.contains(FontStyle::UNDERLINE) {
                        egui::Stroke::new(1.0, self.default_text_color())
                    } else {
                        egui::Stroke::NONE
                    },
                    line_height: Some(line_height),
                    ..Default::default()
                };

                if let Some((background, is_active)) =
                    overlap(start, char_pos, matches, active_match)
                {
                    format.background = background;
                    if is_active {
                        format.color = Color32::WHITE;
                    }
                }

                job.append(piece, 0.0, format);
            }
        }

        let galley = ctx.fonts_mut(|fonts| fonts.layout_job(job));
        if hidden.is_empty() {
            return galley;
        }
        let mut galley = galley;
        compact(Arc::make_mut(&mut galley), hidden);
        galley
    }

    /// Svuota le cache quando `egui` ha ricostruito l'atlante dei font.
    ///
    /// `epaint` rifà l'atlante (e azzera le proprie cache) quando cambiano le
    /// opzioni di testo o quando l'atlante si riempie; le nostre galley
    /// punterebbero ai glifi vecchi e il testo uscirebbe con caratteri
    /// sbagliati. Qui si tiene traccia dell'atlante e si reagisce allo stesso
    /// modo.
    ///
    /// L'atlante che si *ingrandisce* mentre si disegna non è un problema: le
    /// UV dei glifi sono in pixel e vengono normalizzate da `epaint` al momento
    /// della tessellatura, con la dimensione corrente. Una ricostruzione invece
    /// riparte da un atlante vuoto e basso: la si riconosce dal fatto che
    /// rimpicciolisce.
    fn forget_stale_atlas(&mut self, ctx: &egui::Context) {
        let (options, size) = ctx.fonts(|fonts| (*fonts.options(), fonts.font_image_size()));
        let rebuilt = self.atlas.is_some_and(|previous| {
            previous.options != options || size[0] < previous.size[0] || size[1] < previous.size[1]
        });
        self.atlas = Some(AtlasStamp { options, size });
        if rebuilt {
            self.clear();
        }
    }

    /// La sintassi `syntect` per un linguaggio.
    fn syntax_for(&self, language: Language) -> Option<&SyntaxReference> {
        let hint = language.syntect_hint()?;
        self.syntaxes
            .find_syntax_by_name(hint)
            .or_else(|| self.syntaxes.find_syntax_by_extension(&hint.to_lowercase()))
            .or_else(|| Some(self.syntaxes.find_syntax_plain_text()))
    }
}

/// "Comprime" una galley nascondendo le righe indicate (i blocchi piegati).
///
/// Non si possono togliere righe: il numero di caratteri di tutte le righe deve
/// restare uguale a quello del testo, altrimenti i cursori di `TextEdit`
/// andrebbero fuori scala (e in debug scatta un `debug_assert`). Si azzera
/// invece l'altezza delle righe nascoste e si risale quelle successive,
/// svuotando la loro mesh: così il testo non viene disegnato e lo scorrimento
/// tiene conto solo delle righe visibili.
fn compact(galley: &mut Galley, hidden: &[Range<usize>]) {
    let pixels_per_point = galley.pixels_per_point;
    let mut y = galley.rect.min.y;
    let mut bounds = Rect::NOTHING;
    let mut vertices = 0usize;
    let mut indices = 0usize;
    let mut hidden_iter = hidden.iter().peekable();

    for (line, placed) in galley.rows.iter_mut().enumerate() {
        while let Some(range) = hidden_iter.peek() {
            if range.end <= line {
                hidden_iter.next();
            } else {
                break;
            }
        }
        let is_hidden = hidden_iter.peek().is_some_and(|range| range.contains(&line));
        placed.pos = Pos2::new(placed.pos.x, y).round_to_pixels(pixels_per_point);
        if is_hidden {
            let row = Arc::make_mut(&mut placed.row);
            row.size.y = 0.0;
            row.visuals.mesh.clear();
            row.visuals.mesh_bounds = Rect::NOTHING;
            row.visuals.glyph_index_start = 0;
            row.visuals.glyph_vertex_range = 0..0;
            // Con la mesh vuota gli indici dei vertici dei glifi non valgono più.
            // `paint_text_selection` li usa per ricolorare il testo selezionato:
            // azzerarli evita che vada fuori scala (le righe sono nascoste, non
            // si vede nulla).
            for glyph in &mut row.glyphs {
                glyph.first_vertex = 0;
            }
        } else {
            y += placed.row.size.y;
            bounds |= placed
                .row
                .visuals
                .mesh_bounds
                .translate(placed.pos.to_vec2());
            vertices += placed.row.visuals.mesh.vertices.len();
            indices += placed.row.visuals.mesh.indices.len();
        }
    }

    galley.rect.max.y = y.max(galley.rect.min.y);
    galley.mesh_bounds = bounds;
    galley.num_vertices = vertices;
    galley.num_indices = indices;
}

/// Sfondo da usare per un pezzo di testo che interseca le corrispondenze.
fn overlap(
    start: usize,
    end: usize,
    matches: &[(usize, usize)],
    active_match: Option<usize>,
) -> Option<(Color32, bool)> {
    if matches.is_empty() || end <= start {
        return None;
    }
    let mut found = None;
    let upper = matches.partition_point(|(from, _)| *from < end);
    for index in (0..upper).rev() {
        let (_, to) = matches[index];
        if to <= start {
            break;
        }
        let is_active = active_match == Some(index);
        found = Some((if is_active { ACTIVE_MATCH_BG } else { MATCH_BG }, is_active));
        if is_active {
            break;
        }
    }
    found
}

/// Tema `syntect` predefinito per uno sfondo scuro o chiaro.
pub fn default_theme_name(dark: bool) -> &'static str {
    if dark {
        DEFAULT_DARK_THEME
    } else {
        DEFAULT_LIGHT_THEME
    }
}

/// Luminanza percepita di un colore (`0` = nero, `255` = bianco).
fn luminance(color: syntect::highlighting::Color) -> f32 {
    0.2126 * f32::from(color.r) + 0.7152 * f32::from(color.g) + 0.0722 * f32::from(color.b)
}

/// `true` se il tema è pensato per uno sfondo scuro.
///
/// Decide lo sfondo dichiarato dal tema; se manca, si guarda il colore del
/// testo (chiaro = tema scuro).
fn theme_is_dark(theme: &Theme) -> bool {
    if let Some(background) = theme.settings.background {
        return luminance(background) < 128.0;
    }
    theme
        .settings
        .foreground
        .is_none_or(|foreground| luminance(foreground) >= 128.0)
}

/// Da `syntect` a `egui`.
fn rgba(r: u8, g: u8, b: u8, a: u8) -> Color32 {
    if a == 0 || a == 255 {
        Color32::from_rgb(r, g, b)
    } else {
        Color32::from_rgba_unmultiplied(r, g, b, a)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ride_core::Document;

    #[test]
    fn la_cache_si_invalida_solo_quando_il_testo_cambia() {
        let ctx = egui::Context::default();
        // I font esistono solo dopo un frame completo: `begin_pass` non basta.
        let mut output = ctx.run_ui(egui::RawInput::default(), |_ui| {});
        // Le texture generate dal frame vanno "consumate": qui non c'è
        // renderer, quindi si scartano esplicitamente.
        output.textures_delta.clear();

        let mut hl = Highlighter::new(None);
        let mut doc = Document::new_untitled("1");
        doc.set_text("fn main() {}\n");
        let id = DocumentId(1);
        let font = FontId::monospace(12.0);
        let key = HighlightKey {
            language: doc.language(),
            find: 0,
            folds: 0,
            style: 0,
        };

        let first = hl.galley(
            &ctx, id, doc.text(), doc.language(), key, true, &[], None, font.clone(), 14.0, 0, &[],
        );
        let width = hl.measured_width(id).expect("larghezza misurata");
        let second = hl.galley(
            &ctx, id, doc.text(), doc.language(), key, true, &[], None, font.clone(), 14.0, 0, &[],
        );
        // Stessa chiave e stesso testo: cache hit, è lo stesso `Arc`.
        assert!(Arc::ptr_eq(&first, &second));

        // Il testo cambia: la galley è ricostruita (lo si vede dalla larghezza
        // aggiornata) e non è più la stessa. Nota: la chiave resta identica —
        // è il *testo*, non la revisione, a invalidare la cache.
        doc.set_text("fn main() { let x = 42; }\n");
        let third = hl.galley(
            &ctx, id, doc.text(), doc.language(), key, true, &[], None, font, 14.0, 0, &[],
        );
        assert!(!Arc::ptr_eq(&first, &third));
        assert_ne!(hl.measured_width(id), Some(width));
    }

    /// Cambiare tema cambia le opzioni di testo: `epaint` getta via l'atlante
    /// dei font e lo ricostruisce, quindi le galley in cache puntano a glifi che
    /// non esistono più. È il difetto per cui, dopo il cambio di tema, il codice
    /// si vedeva con caratteri sbagliati.
    #[test]
    fn la_ricostruzione_dell_atlante_butta_la_cache() {
        let ctx = egui::Context::default();
        ctx.set_visuals(egui::Visuals::dark());
        let mut output = ctx.run_ui(egui::RawInput::default(), |_ui| {});
        output.textures_delta.clear();

        let mut hl = Highlighter::new(None);
        let id = DocumentId(5);
        let font = FontId::monospace(12.0);
        let key = HighlightKey {
            language: Language::Rust,
            find: 0,
            folds: 0,
            style: 0,
        };
        let testo = "fn main() {}\n";

        let before = hl.galley(
            &ctx, id, testo, Language::Rust, key, true, &[], None, font.clone(), 14.0, 0, &[],
        );
        // Stessa chiave e stesso testo: la cache risponde.
        let again = hl.galley(
            &ctx, id, testo, Language::Rust, key, true, &[], None, font.clone(), 14.0, 0, &[],
        );
        assert!(Arc::ptr_eq(&before, &again));

        // Cambio di tema: al prossimo `begin_pass` `egui` rifà l'atlante.
        ctx.set_visuals(egui::Visuals::light());
        let mut output = ctx.run_ui(egui::RawInput::default(), |_ui| {});
        output.textures_delta.clear();

        // Un altro testo impaginato per primo si prende le prime posizioni
        // dell'atlante nuovo, quindi il nostro glifo finisce altrove: è
        // esattamente il motivo per cui i caratteri si vedevano sbagliati.
        let _altra = hl.galley(
            &ctx,
            DocumentId(6),
            "mmm\n",
            Language::Text,
            key,
            true,
            &[],
            None,
            font.clone(),
            14.0,
            0,
            &[],
        );
        let after = hl.galley(
            &ctx, id, testo, Language::Rust, key, true, &[], None, font, 14.0, 0, &[],
        );

        assert!(
            !Arc::ptr_eq(&before, &after),
            "la galley vecchia punta all'atlante vecchio"
        );
        let posizione = |galley: &Arc<Galley>| {
            galley.rows[0]
                .row
                .glyphs
                .iter()
                .find(|glifo| glifo.chr == 'n')
                .map(|glifo| glifo.uv_rect.min)
        };
        assert_ne!(
            posizione(&before),
            posizione(&after),
            "il glifo deve stare in un'altra posizione dell'atlante"
        );
    }

    #[test]
    fn temi_disponibili_e_riconosciuti() {
        let mut hl = Highlighter::new(None);
        hl.set_theme(DEFAULT_LIGHT_THEME);
        assert_eq!(hl.theme_name(), DEFAULT_LIGHT_THEME);
        // Un tema inesistente non cambia nulla.
        hl.set_theme("non-esiste");
        assert_eq!(hl.theme_name(), DEFAULT_LIGHT_THEME);
    }

    /// I temi proposti si leggono sullo sfondo dell'editor: mai un tema chiaro
    /// con sfondo scuro (o viceversa), che renderebbe il codice invisibile.
    #[test]
    fn i_temi_proposti_si_leggono_sullo_sfondo() {
        let hl = Highlighter::new(None);
        for dark in [true, false] {
            let names = hl.theme_names_for(dark);
            assert!(!names.is_empty(), "nessun tema per dark = {dark}");
            for name in names {
                assert!(hl.theme_matches(&name, dark), "{name} con dark = {dark}");
            }
        }
        // I predefiniti stanno dalla parte giusta...
        assert!(hl.theme_matches(DEFAULT_DARK_THEME, true));
        assert!(hl.theme_matches(DEFAULT_LIGHT_THEME, false));
        assert_eq!(default_theme_name(true), DEFAULT_DARK_THEME);
        assert_eq!(default_theme_name(false), DEFAULT_LIGHT_THEME);
        // ...e i due insiemi non si mescolano.
        assert!(!hl.theme_matches(DEFAULT_LIGHT_THEME, true));
        assert!(!hl.theme_matches(DEFAULT_DARK_THEME, false));
    }

    /// Il testo del codice deve staccarsi dallo sfondo, nei due temi: è il
    /// difetto che faceva "sparire" i caratteri.
    #[test]
    fn il_testo_contrasta_con_lo_sfondo() {
        let ctx = egui::Context::default();
        let mut output = ctx.run_ui(egui::RawInput::default(), |_ui| {});
        output.textures_delta.clear();

        for dark in [true, false] {
            let mut prefs = crate::prefs::Prefs::default();
            prefs.dark = dark;
            let mut hl = Highlighter::new(None);
            hl.set_default_for(dark);
            assert!(
                contrasto(hl.default_text_color(), prefs.editor_bg()) >= 3.0,
                "testo poco leggibile con dark = {dark}"
            );

            // Anche i singoli token (parole chiave, commenti, stringhe) devono
            // restare leggibili.
            let text = "// commento\nfn main() { let x = 42; println!(\"ciao\"); }\n";
            let galley = hl.galley(
                &ctx,
                DocumentId(3),
                text,
                Language::Rust,
                HighlightKey {
                    language: Language::Rust,
                    find: 0,
                    folds: 0,
                    style: 0,
                },
                true,
                &[],
                None,
                FontId::monospace(14.0),
                16.0,
                0,
                &[],
            );
            let mut peggiore = f32::INFINITY;
            for section in &galley.job.sections {
                peggiore = peggiore.min(contrasto(section.format.color, prefs.editor_bg()));
            }
            assert!(
                peggiore >= 2.0,
                "token poco leggibili con dark = {dark}: contrasto {peggiore}"
            );
        }
    }

    /// Rapporto di contrasto WCAG fra due colori (1.0 = identici).
    fn contrasto(a: Color32, b: Color32) -> f32 {
        fn canale(valore: u8) -> f32 {
            let v = f32::from(valore) / 255.0;
            if v <= 0.03928 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        }
        let luminanza = |c: Color32| {
            0.2126 * canale(c.r()) + 0.7152 * canale(c.g()) + 0.0722 * canale(c.b())
        };
        let (a, b) = (luminanza(a), luminanza(b));
        let (alto, basso) = if a > b { (a, b) } else { (b, a) };
        (alto + 0.05) / (basso + 0.05)
    }

    #[test]
    fn il_testo_nuovo_non_resta_nella_cache_vecchia() {
        let ctx = egui::Context::default();
        let mut output = ctx.run_ui(egui::RawInput::default(), |_ui| {});
        output.textures_delta.clear();

        let mut hl = Highlighter::new(None);
        let id = DocumentId(7);
        let font = FontId::monospace(12.0);
        // Stessa chiave ma testo diverso: è esattamente ciò che succede quando
        // `TextEdit` impagina di nuovo nello stesso frame dopo una battuta,
        // prima che il resto dello stato sia aggiornato.
        let key = HighlightKey {
            language: Language::Rust,
            find: 0,
            folds: 0,
            style: 0,
        };
        let short = hl.galley(
            &ctx, id, "a\n", Language::Rust, key, true, &[], None, font.clone(), 14.0, 0, &[],
        );
        let longer = hl.galley(
            &ctx, id, "aaaaaaaaaa\n", Language::Rust, key, true, &[], None, font, 14.0, 0, &[],
        );
        // Il testo è cambiato: la galley va ricostruita, non riusata.
        assert!(!Arc::ptr_eq(&short, &longer));
        assert!(longer.size().x > short.size().x);
    }

    #[test]
    fn riconosce_la_sintassi_rust() {
        let hl = Highlighter::new(None);
        let syntax = hl.syntax_for(Language::Rust).unwrap();
        assert!(syntax.name.to_lowercase().contains("rust"));
    }

    /// Le righe di un blocco piegato restano nella galley (gli indici di
    /// carattere non cambiano) ma perdono altezza e mesh.
    #[test]
    fn le_righe_piegate_non_occupano_spazio() {
        let ctx = egui::Context::default();
        let mut output = ctx.run_ui(egui::RawInput::default(), |_ui| {});
        output.textures_delta.clear();

        let mut hl = Highlighter::new(None);
        let id = DocumentId(11);
        let font = FontId::monospace(12.0);
        let text = "a\nb\nc\nd\n";
        let key = HighlightKey {
            language: Language::Text,
            find: 0,
            folds: 1,
            style: 0,
        };

        let full = hl.galley(
            &ctx,
            id,
            text,
            Language::Text,
            HighlightKey { folds: 0, ..key },
            true,
            &[],
            None,
            font.clone(),
            20.0,
            0,
            &[],
        );
        let hide = 1usize..3;
        let folded = hl.galley(
            &ctx,
            id,
            text,
            Language::Text,
            key,
            true,
            &[],
            None,
            font,
            20.0,
            0,
            &[hide],
        );

        assert_eq!(full.rows.len(), 5);
        assert_eq!(folded.rows.len(), 5, "le righe restano tutte");
        // Due righe nascoste: meno altezza, e le righe nascoste sono vuote.
        assert!(folded.size().y < full.size().y);
        assert!(folded.rows[1].row.visuals.mesh.is_empty());
        assert!(folded.rows[2].row.visuals.mesh.is_empty());
        assert!(!folded.rows[0].row.visuals.mesh.is_empty());
        // Le righe visibili dopo il blocco risalgono al posto di quelle nascoste.
        assert!((folded.rows[3].pos.y - 20.0).abs() < 0.5);
        assert!((folded.rows[4].pos.y - 40.0).abs() < 0.5);
    }
}
