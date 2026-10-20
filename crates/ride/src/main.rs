//! # Ride
//!
//! Un editor di codice multi-file con gestione di progetti.
//!
//! Il binario è solo l'*integrazione*: crea la finestra e delega tutto a
//! [`app::RideApp`]. La logica che non dipende dalla grafica vive nel crate
//! `ride-core`, così è testabile senza aprire una finestra.
//!
//! Uso:
//!
//! ```text
//! ride [percorso]…
//! ```
//!
//! Una cartella viene aperta come progetto; un file viene aperto (e il suo
//! progetto con esso). Senza argomenti l'editor parte vuoto.

mod action;
mod app;
mod highlight;
mod lsp;
mod prefs;
mod state;
mod terminal;
mod ui;

use app::RideApp;

fn main() -> eframe::Result {
    // Il primo argomento è il nome del programma: lo lasciamo fuori.
    let args: Vec<String> = std::env::args().skip(1).collect();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Ride")
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([640.0, 400.0]),
        ..Default::default()
    };

    eframe::run_native(
        "Ride",
        options,
        Box::new(move |cc| Ok(Box::new(RideApp::new(cc, &args)))),
    )
}
