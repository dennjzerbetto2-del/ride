//! # ride-core
//!
//! Tutta la logica dell'editor che **non** dipende dalla GUI.
//!
//! Tenere il core separato ha tre vantaggi concreti:
//!
//! 1. si testa senza aprire una finestra (`cargo test -p ride-core`);
//! 2. la GUI resta un livello di solo disegno e interazione;
//! 3. domani si può riusare il core per una TUI, un server LSP o un plugin.
//!
//! Moduli:
//!
//! * [`document`] – un file aperto (testo, EOL, stato "modificato", salvataggio);
//! * [`workspace`] – l'insieme dei documenti aperti (le tab) e il progetto corrente;
//! * [`session`] – com'era la scrivania (progetti, file aperti, alberi) da ritrovare
//!   al prossimo avvio;
//! * [`project`] – il progetto (cartella radice) e l'albero dei file;
//! * [`search`] – ricerca testuale/regolare dentro il progetto;
//! * [`editor`] – operazioni di editing (indenta, commenta, duplica righe, …);
//! * [`folds`] – i blocchi `{ … }` che si possono piegare nell'editor;
//! * [`syntax`] – linguaggio dedotto dall'estensione e convenzioni di stile;
//! * [`format`] – invocazione di formatter esterni (rustfmt, prettier, …);
//! * [`build`] – riconoscimento e invocazione del sistema di build (Maven, Gradle).

pub mod build;
pub mod document;
pub mod editor;
pub mod folds;
pub mod format;
pub mod project;
pub mod search;
pub mod session;
pub mod syntax;
pub mod workspace;

pub use document::{Cursor, Document, DocumentError, Eol};
pub use project::{FileNode, Project, TreeOptions};
pub use search::{SearchHit, SearchQuery, SearchResult};
pub use session::{ProjectSession, Session};
pub use syntax::{Indent, Language};
pub use workspace::{DocumentId, Workspace, WorkspaceError};
