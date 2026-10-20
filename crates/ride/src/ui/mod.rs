//! Disegno dell'interfaccia.
//!
//! Ogni modulo è una parte dello schermo e **non** prende decisioni sul modello:
//! disegna a partire da [`crate::app::RideApp`] e chiede cosa fare restituendo
//! [`crate::action::Action`]. L'unico posto che esegue le azioni è
//! [`crate::app::RideApp::dispatch`]: così l'ordine con cui egui disegna i
//! pannelli non può produrre stati incoerenti.
//!
//! ```text
//! ┌──────────────────────────────────────────────────────┐
//! │ menu                                             [1] │
//! │ tab del progetto                    [2]              │
//! ├───────────┬──────────────────────────────────┬───────┤
//! │ albero    │ editor (gutter + testo)          │ cerca │
//! │ [3]       │ [5]                              │ [4]   │
//! ├───────────┴──────────────────────────────────┴───────┤
//! │ terminale (facoltativo, Ctrl+`)                  [7] │
//! ├──────────────────────────────────────────────────────┤
//! │ barra di stato                                   [6] │
//! └──────────────────────────────────────────────────────┘
//! ```
//!
//! I dialoghi ([`dialogs`]) sono finestre sovrapposte, disegnate per ultime.

pub mod dialogs;
pub mod editor;
pub mod logo;
pub mod menu;
pub mod search;
pub mod status;
pub mod tabs;
pub mod terminal;
pub mod tree;
