//! Il logo mostrato nella finestra "Informazioni su Ride".
//!
//! Il PNG è incorporato nel binario, così l'eseguibile resta autosufficiente.
//! Viene decodificato alla prima apertura della finestra, ritagliato dal bordo
//! bianco che lo circonda e tenuto in una texture di egui.

use egui::{Color32, ColorImage, Context, Id, Rect, Sense, TextureHandle, TextureOptions, Vec2};

/// Logo originale (1408×768, sfondo quasi bianco).
const LOGO_PNG: &[u8] = include_bytes!("../../assets/ride-about.png");

/// Margine attorno al logo dopo il ritaglio, in pixel dell'immagine.
const CROP_MARGIN: usize = 4;
/// Larghezza massima con cui il logo viene disegnato, in punti.
const MAX_WIDTH: f32 = 320.0;
/// Spazio bianco attorno al logo, in punti.
const PADDING: f32 = 12.0;

/// Disegna il logo, centrato e su una placca chiara (il logo nasce per uno
/// sfondo bianco e i suoi colori scuri sparirebbero sul tema scuro).
pub fn show(ui: &mut egui::Ui) {
    let texture = texture(ui.ctx());
    let scale = (MAX_WIDTH / texture.size()[0] as f32).min(1.0);
    let logo = texture.size_vec2() * scale;
    let card = logo + Vec2::splat(PADDING * 2.0);

    // Il chiamante la dispone in un layout centrato; qui basta la placca.
    let (rect, _) = ui.allocate_exact_size(card, Sense::hover());
    let inner = Rect::from_center_size(rect.center(), logo);

    let painter = ui.painter();
    painter.rect_filled(rect, 10.0, Color32::from_rgb(0xfd, 0xfd, 0xfd));
    painter.image(
        texture.id(),
        inner,
        Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
        Color32::WHITE,
    );
}

/// La texture del logo, decodificata e caricata una sola volta.
fn texture(ctx: &Context) -> TextureHandle {
    let id = Id::unique("ride-about-logo");
    if let Some(texture) = ctx.data(|data| data.get_temp::<TextureHandle>(id)) {
        return texture;
    }
    let texture = ctx.load_texture("ride-about-logo", decode(), TextureOptions::LINEAR);
    ctx.data_mut(|data| data.insert_temp(id, texture.clone()));
    texture
}

/// Decodifica il PNG e ritaglia il bordo quasi bianco.
fn decode() -> ColorImage {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(LOGO_PNG));
    decoder.set_transformations(
        png::Transformations::normalize_to_color8() | png::Transformations::ALPHA,
    );
    let mut reader = decoder
        .read_info()
        .expect("il logo incorporato è un PNG valido");
    let mut buf = vec![
        0;
        reader
            .output_buffer_size()
            .expect("il logo incorporato è un PNG valido")
    ];
    let info = reader
        .next_frame(&mut buf)
        .expect("il logo incorporato è un PNG valido");
    buf.truncate(info.buffer_size());

    debug_assert_eq!(info.color_type, png::ColorType::Rgba);
    let width = info.width as usize;
    let height = info.height as usize;
    let Some((x0, y0, x1, y1)) = crop_bounds(&buf, width, height) else {
        return ColorImage::from_rgba_unmultiplied([width, height], &buf);
    };

    let crop_width = x1 - x0;
    let crop_height = y1 - y0;
    let mut cropped = Vec::with_capacity(crop_width * crop_height * 4);
    for y in y0..y1 {
        let start = (y * width + x0) * 4;
        cropped.extend_from_slice(&buf[start..start + crop_width * 4]);
    }
    ColorImage::from_rgba_unmultiplied([crop_width, crop_height], &cropped)
}

/// Rettangolo (esclusivo a destra e in basso) dei pixel che non sono sfondo.
///
/// Lo sfondo del logo è bianco con una leggera grana: basta cercare il primo e
/// l'ultimo pixel abbastanza scuro da non essere bianco.
fn crop_bounds(rgba: &[u8], width: usize, height: usize) -> Option<(usize, usize, usize, usize)> {
    let mut min = (width, height);
    let mut max = (0, 0);
    for y in 0..height {
        for x in 0..width {
            let i = (y * width + x) * 4;
            if rgba[i..i + 3].iter().any(|&channel| channel < 235) {
                min = (min.0.min(x), min.1.min(y));
                max = (max.0.max(x), max.1.max(y));
            }
        }
    }
    if min.0 > max.0 {
        return None;
    }
    Some((
        min.0.saturating_sub(CROP_MARGIN),
        min.1.saturating_sub(CROP_MARGIN),
        (max.0 + 1 + CROP_MARGIN).min(width),
        (max.1 + 1 + CROP_MARGIN).min(height),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn il_logo_si_decodifica_e_si_ritaglia() {
        let image = decode();
        // L'originale è 1408×768 con tanto bordo bianco: dopo il ritaglio resta
        // il marchio vero e proprio.
        assert!(
            image.size[0] > 700 && image.size[0] < 900,
            "{:?}",
            image.size
        );
        assert!(
            image.size[1] > 250 && image.size[1] < 350,
            "{:?}",
            image.size
        );
    }
}
