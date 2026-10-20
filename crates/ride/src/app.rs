//! L'applicazione: collega modello (`ride-core`), stato dell'interfaccia e
//! disegno (`ui`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use egui::{Color32, Context, ViewportCommand};
use ride_core::build::BuildTool;
use ride_core::editor::{self, Lines};
use ride_core::search::SearchQuery;
use ride_core::{DocumentId, Language, Session, Workspace};

use crate::action::{Action, CloseChoice};
use crate::highlight::Highlighter;
use crate::lsp::{char_offset, server_for, LspEvent, LspManager, LspStatus};
use crate::prefs::Prefs;
use crate::state::{
    BuildEvent, BuildState, CloseRequest, EditorSnapshot, FindState, Jobs, ProjectSearchState,
    Prompt, PromptAction, Status, TreeState,
};
use crate::terminal::TerminalState;
use crate::ui;

/// Chiave con cui si salva la sessione (progetti e file aperti).
const SESSION_KEY: &str = "ride.session";

/// Operazioni sulle righe (menu *Modifica*).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineOp {
    Comment,
    Indent,
    Dedent,
    Duplicate,
    MoveUp,
    MoveDown,
    Delete,
}

/// Posizione del cursore memorizzata per non ricalcolare le righe a ogni frame.
#[derive(Debug, Clone, Copy)]
struct CursorCache {
    caret: usize,
    line: usize,
    column: usize,
}

/// L'applicazione.
pub struct RideApp {
    /// Progetti + documenti aperti (il modello).
    pub workspace: Workspace,
    pub prefs: Prefs,
    /// Albero del progetto **attivo**.
    pub tree: TreeState,
    /// Alberi degli altri progetti aperti: cambiando progetto non si perdono le
    /// cartelle espanse né la selezione.
    trees: HashMap<PathBuf, TreeState>,
    /// Colori scelti per i progetti aperti, associati alla loro radice.
    pub(crate) project_colors: HashMap<PathBuf, Color32>,
    pub find: FindState,
    pub search: ProjectSearchState,
    pub dialogs: crate::state::DialogState,
    pub status: Status,
    pub jobs: Jobs,
    pub highlighter: Highlighter,
    pub lsp: LspManager,
    /// Ultima posizione/selezione dell'editor.
    pub editor: Option<EditorSnapshot>,
    /// Riga del cursore del documento attivo (per il canale dei numeri).
    pub cursor_line: usize,
    /// Selezione da impostare al prossimo frame (dopo un'operazione o una ricerca).
    pub pending_selection: Option<(usize, usize)>,
    /// Il prossimo frame deve dare il focus all'editor.
    pub focus_editor: bool,
    /// Linguaggio per cui si è già tentato di avviare il server (evita di
    /// scandire il `PATH` a ogni frame quando il server non è installato).
    pub lsp_tried: Option<Language>,
    /// Sistemi di build riconosciuti nel progetto (Maven/Gradle).
    pub toolchains: Vec<ride_core::build::Toolchain>,
    /// Plugin Maven del progetto, per il menu *Esegui*: quelli dichiarati nel
    /// `pom.xml` più i pochi utili in ogni caso.
    pub maven_plugins: Vec<&'static ride_core::build::MavenPlugin>,
    /// La console dei comandi di build.
    pub build: BuildState,
    /// Il terminale integrato (agganciato in fondo alla finestra).
    pub terminal: TerminalState,
    cursor_cache: HashMap<DocumentId, CursorCache>,
    /// Formattazioni in corso (per tenere viva l'interfaccia).
    pub pending_formats: usize,
    /// L'utente ha confermato di voler uscire.
    quit_confirmed: bool,
    /// Numero di documento da assegnare (solo per i nomi provvisori).
    pub frame: u64,
}

impl RideApp {
    /// Crea l'applicazione, aprendo quanto indicato sulla riga di comando.
    pub fn new(cc: &eframe::CreationContext<'_>, args: &[String]) -> Self {
        let mut app = RideApp {
            workspace: Workspace::new(),
            prefs: Prefs::default(),
            tree: TreeState::default(),
            trees: HashMap::new(),
            project_colors: HashMap::new(),
            find: FindState::default(),
            search: ProjectSearchState::default(),
            dialogs: Default::default(),
            status: Status::default(),
            jobs: Jobs::new(),
            highlighter: Highlighter::new(None),
            lsp: LspManager::new(),
            editor: None,
            cursor_line: 0,
            pending_selection: None,
            focus_editor: false,
            lsp_tried: None,
            toolchains: Vec::new(),
            maven_plugins: Vec::new(),
            build: BuildState::default(),
            terminal: TerminalState::default(),
            cursor_cache: HashMap::new(),
            pending_formats: 0,
            quit_confirmed: false,
            frame: 0,
        };
        if let Some(storage) = cc.storage {
            app.prefs.load(storage);
            if let Some(theme) = app.prefs.syntax_theme.clone() {
                app.highlighter.set_theme(&theme);
            }
            // Un tema di sintassi salvato che stona con lo sfondo non va
            // riproposto: il codice risulterebbe illeggibile.
            app.sync_syntax_theme();
        }
        app.tree.options.show_hidden = app.prefs.show_hidden;
        app.tree.options.respect_gitignore = app.prefs.respect_gitignore;
        app.lsp.set_repaint_context(cc.egui_ctx.clone());
        app.terminal.set_repaint_context(cc.egui_ctx.clone());
        app.terminal.auto_hide = app.prefs.terminal_auto_hide;
        app.prefs.apply(&cc.egui_ctx);
        // Senza indicazioni dalla riga di comando si riprende da dove si era:
        // con una cartella o un file chiesti esplicitamente, invece, comanda
        // quello che si è scritto.
        if args.is_empty() {
            if let Some(storage) = cc.storage {
                app.restore_session(storage);
            }
        }
        app.open_from_args(args);
        app
    }

    /// Interpreta gli argomenti della riga di comando: `ride [percorso]…`.
    ///
    /// Una cartella apre il progetto, un file apre il suo progetto e il file.
    fn open_from_args(&mut self, args: &[String]) {
        let start = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let mut project: Option<PathBuf> = None;
        for arg in args {
            let path = if Path::new(arg).is_absolute() {
                PathBuf::from(arg)
            } else {
                start.join(arg)
            };
            let Ok(path) = path.canonicalize() else {
                self.status.error(format!("{} non esiste", path.display()));
                continue;
            };
            if path.is_dir() {
                if let Err(e) = self.set_project(&path) {
                    self.status.error(format!("{e}"));
                }
            } else {
                project = project.or_else(|| path.parent().map(|p| p.to_path_buf()));
                if let Err(e) = self.workspace.open_path(&path) {
                    self.status.error(format!("{e}"));
                } else {
                    self.cursor_cache.remove(&DocumentId(0));
                }
            }
        }
        if let Some(root) = project {
            if let Err(e) = self.set_project(&root) {
                self.status.warn(format!("progetto non aperto: {e}"));
            }
        }
    }

    /// Rilegge la sessione salvata: le cartelle aperte e i loro file.
    fn restore_session(&mut self, storage: &dyn eframe::Storage) {
        let Some(text) = storage.get_string(SESSION_KEY) else {
            return;
        };
        self.restore_session_text(&text);
    }

    /// Riapre progetti e file descritti da una sessione già scritta.
    ///
    /// È separata da [`Self::restore_session`] perché così si può provare senza
    /// un archivio di preferenze vero.
    fn restore_session_text(&mut self, text: &str) {
        let session = Session::decode(text);
        if session.is_empty() {
            return;
        }
        let voci = session.len();
        let mancanti = session.restore(&mut self.workspace);
        self.project_colors.clear();
        for project in &session.projects {
            if self.workspace.index_of_project(&project.root).is_some() {
                if let Some([red, green, blue, alpha]) = project.color {
                    self.project_colors.insert(
                        project.root.clone(),
                        Color32::from_rgba_unmultiplied(red, green, blue, alpha),
                    );
                }
            }
        }
        // Gli alberi si preparano prima di mostrarne uno: `sync_shown_project`
        // prende dal deposito quello del progetto attivo.
        self.restore_trees(&session);
        self.sync_shown_project();
        if mancanti == 0 {
            self.status.info("Ripreso il lavoro dell'ultima volta");
        } else {
            self.status.warn(format!(
                "{mancanti} di {voci} elementi dell'ultima sessione non si sono potuti riaprire"
            ));
        }
    }

    // ---------------------------------------------------------------------
    // Progetto
    // ---------------------------------------------------------------------

    /// Apre una cartella come progetto e la rende attiva.
    ///
    /// Se la cartella è già aperta non se ne fa una seconda copia: si passa a
    /// quella che c'è già. È così che *Apri cartella…* aggiunge un progetto
    /// invece di sostituire quello in corso.
    fn set_project(&mut self, root: &Path) -> Result<(), ride_core::WorkspaceError> {
        let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        if let Some(index) = self.workspace.index_of_project(&root) {
            // Già aperto: lo si riattiva (o si dice che è già quello attivo).
            if self.workspace.active_project_index() == Some(index) {
                let name = self.project_name();
                self.status.info(format!("Il progetto «{name}» è già quello aperto"));
            } else {
                self.switch_project(index);
            }
            return Ok(());
        }
        // Se l'apertura fallisce non si è mosso niente.
        self.workspace.open_project(&root)?;
        self.prefs.show_tree = true;
        self.prefs.mark_dirty();
        self.sync_shown_project();
        let name = self.project_name();
        self.status.info(format!("Progetto «{name}» aperto"));
        Ok(())
    }

    /// Mostra il progetto indicato: l'albero, i file aperti, la ricerca e la
    /// build seguono.
    ///
    /// Il language server è legato a una cartella: [`Self::sync_lsp`] se ne
    /// accorge (radice diversa) e lo riavvia sul progetto nuovo.
    fn switch_project(&mut self, index: usize) -> bool {
        if !self.workspace.activate_project(index) {
            return false;
        }
        self.sync_shown_project();
        let name = self.project_name();
        self.status.info(format!("Progetto «{name}» attivo"));
        true
    }

    /// Nome del progetto attivo.
    fn project_name(&self) -> String {
        self.workspace
            .project()
            .map(|p| p.name().to_string())
            .unwrap_or_default()
    }

    /// L'albero mostrato dev'essere quello del progetto attivo: se qualcosa (un
    /// file aperto in un altro progetto, un progetto chiuso…) ha cambiato
    /// progetto lo si riallinea qui.
    ///
    /// L'albero che si lascia resta nel deposito, così espansioni e selezione
    /// non si perdono tornandoci.
    fn sync_shown_project(&mut self) {
        let shown = self.tree.root.as_ref().map(|node| node.path.as_path());
        if shown == self.workspace.root() {
            return;
        }
        let active = self.workspace.root().map(|r| r.to_path_buf());
        if let Some(shown) = shown.map(Path::to_path_buf) {
            self.trees.insert(shown, std::mem::take(&mut self.tree));
        }
        self.tree = match &active {
            Some(root) => self.adopt_project_tree(root),
            None => self.empty_tree(),
        };
        self.after_project_changed();
    }

    /// L'albero da mostrare per quella cartella: quello di prima, se c'era già
    /// (cartelle espanse, selezione), altrimenti uno letto adesso.
    fn adopt_project_tree(&mut self, root: &Path) -> TreeState {
        match self.trees.remove(root) {
            Some(tree) => tree,
            None => self.tree_for(root),
        }
    }

    /// Chiude il progetto attivo: le sue tab se ne vanno con lui.
    ///
    /// Se qualcuna è modificata si chiede prima cosa fare.
    fn close_project(&mut self) {
        let Some(index) = self.workspace.active_project_index() else {
            return;
        };
        let dirty = self.workspace.dirty_tabs();
        if !dirty.is_empty() {
            let root = self.workspace.root().map(|r| r.to_path_buf());
            self.dialogs.confirm_close = Some(CloseRequest {
                docs: dirty,
                quit: false,
                close_project: root,
            });
            return;
        }
        self.close_project_now(index);
    }

    /// Chiude davvero un progetto, insieme alle sue tab.
    fn close_project_now(&mut self, index: usize) {
        let name = self
            .workspace
            .project_at(index)
            .map(|p| p.name().to_string())
            .unwrap_or_default();
        let closed = self
            .workspace
            .project_at(index)
            .map(|p| p.root().to_path_buf());
        // Il language server teneva aperti quei documenti: vanno chiusi anche da lui.
        for id in self.workspace.tabs_of(index) {
            self.lsp.did_close(id);
            self.highlighter.forget(id);
            self.cursor_cache.remove(&id);
        }
        self.workspace.close_project_at(index);
        if let Some(root) = closed {
            self.trees.remove(&root);
            self.project_colors.remove(&root);
        }
        // Il server era legato alla cartella chiusa: ripartirà sul progetto nuovo.
        self.lsp_tried = None;
        self.sync_shown_project();
        self.status.info(format!("Progetto «{name}» chiuso"));
    }

    /// Tutto ciò che dipende dalla cartella del progetto attivo va riallineato.
    fn after_project_changed(&mut self) {
        self.refresh_toolchains();
        self.search.result = None;
        self.search.mark_dirty();
        // L'elenco per "vai al file" vale per una cartella sola.
        self.dialogs.quick_open.files.clear();
        self.dialogs.quick_open.root = self.workspace.root().map(|r| r.to_path_buf());
        // Cursore e corrispondenze valgono per il documento che si lascia.
        self.pending_selection = None;
        self.find.matches.clear();
        self.find.current = 0;
    }

    /// Le opzioni dell'albero secondo le preferenze correnti.
    fn tree_options(&self) -> ride_core::project::TreeOptions {
        ride_core::project::TreeOptions {
            show_hidden: self.prefs.show_hidden,
            respect_gitignore: self.prefs.respect_gitignore,
            ..Default::default()
        }
    }

    /// Un albero vuoto (nessun progetto) con le preferenze correnti.
    fn empty_tree(&self) -> TreeState {
        TreeState {
            options: self.tree_options(),
            ..Default::default()
        }
    }

    /// L'albero di una cartella, appena letto.
    fn tree_for(&self, root: &Path) -> TreeState {
        let mut tree = self.empty_tree();
        tree.load(root);
        tree
    }

    /// L'albero di un progetto: quello in mostra, o quello messo da parte.
    fn tree_of(&self, root: &Path) -> Option<&TreeState> {
        if self.tree.root.as_ref().is_some_and(|node| node.path == root) {
            return Some(&self.tree);
        }
        self.trees.get(root)
    }

    /// La sessione da salvare: i progetti, i loro file e i loro alberi.
    ///
    /// Il modello sa quali file sono aperti; quali cartelle sono aperte lo sa
    /// solo l'interfaccia, che lo aggiunge qui.
    fn session(&self) -> Session {
        let mut session = self.workspace.session();
        for project in &mut session.projects {
            project.color = self
                .project_colors
                .get(&project.root)
                .map(|color| color.to_array());
            if let Some(tree) = self.tree_of(&project.root) {
                project.expanded = tree.expanded_paths();
                project.selected = tree.selected.clone();
            }
        }
        session
    }

    /// Rimette gli alberi come erano: cartelle aperte e voce selezionata.
    ///
    /// Gli alberi preparati finiscono nel deposito: è da lì che
    /// [`Self::sync_shown_project`] prende quello del progetto attivo.
    fn restore_trees(&mut self, session: &Session) {
        let shown = self.tree.root.as_ref().map(|node| node.path.clone());
        for project in &session.projects {
            if self.workspace.index_of_project(&project.root).is_none() {
                continue;
            }
            if shown.as_deref() == Some(project.root.as_path()) {
                // L'albero in mostra è già quello del progetto: non si mette da
                // parte una copia, o tornandoci si perderebbe quella corrente.
                continue;
            }
            let mut tree = self.tree_for(&project.root);
            tree.restore(&project.expanded, project.selected.as_deref());
            self.trees.insert(project.root.clone(), tree);
        }
    }

    fn refresh_project(&mut self) {
        self.tree.refresh();
        self.refresh_toolchains();
        self.dialogs.quick_open.files.clear();
        self.dialogs.quick_open.root = self.workspace.root().map(|r| r.to_path_buf());
        self.status.info("Albero del progetto ricaricato");
    }

    /// Riconosce i sistemi di build (Maven/Gradle) nella radice del progetto e
    /// i plugin Maven che il `pom.xml` dichiara.
    fn refresh_toolchains(&mut self) {
        match self.workspace.root() {
            Some(root) => {
                self.toolchains = ride_core::build::detect(root);
                self.maven_plugins = ride_core::build::project_plugins(root);
            }
            None => {
                self.toolchains.clear();
                self.maven_plugins.clear();
            }
        }
    }

    // ---------------------------------------------------------------------
    // File
    // ---------------------------------------------------------------------

    fn open_file(&mut self, path: &Path) {
        if path.is_dir() {
            self.dispatch_quiet(Action::OpenFolder(path.to_path_buf()));
            return;
        }
        match self.workspace.open_path(path) {
            Ok(_) => {
                // Aprire un file di un altro progetto porta lì il progetto attivo.
                self.sync_shown_project();
                if let Some(root) = self.workspace.root() {
                    if path.starts_with(root) {
                        self.tree.selected = Some(path.to_path_buf());
                        self.tree.reveal(path);
                    }
                }
                self.focus_editor = true;
            }
            Err(e) => self.status.error(format!("{e}")),
        }
    }

    /// Salva un documento; se non ha un percorso chiede dove salvarlo.
    fn save_document(&mut self, id: DocumentId) {
        match self.workspace.get(id) {
            None => return,
            Some(doc) if doc.path().is_none() => {
                self.ask_save_as(id);
                return;
            }
            Some(_) => {}
        }
        let result = match self.workspace.get_mut(id) {
            Some(doc) => doc.save().map(|()| doc.display_name().to_string()),
            None => return,
        };
        match result {
            Ok(name) => {
                self.status.info(format!("Salvato {name}"));
                if let Some(path) = self
                    .workspace
                    .get(id)
                    .and_then(|doc| doc.path().map(Path::to_path_buf))
                {
                    self.refresh_build_menu(&path);
                }
            }
            Err(e) => self.status.error(format!("{e}")),
        }
    }

    /// Se il file scritto è il `pom.xml` (o lo script Gradle) del progetto, il
    /// menu *Esegui* va rifatto: plugin e sistemi di build possono essere
    /// cambiati.
    fn refresh_build_menu(&mut self, path: &Path) {
        let Some(root) = self.workspace.root() else {
            return;
        };
        let name = path.file_name().and_then(|name| name.to_str()).unwrap_or_default();
        let build_file = matches!(name, "pom.xml" | "build.gradle" | "build.gradle.kts");
        if build_file && path.parent() == Some(root) {
            self.refresh_toolchains();
        }
    }

    fn save_all(&mut self) {
        let untitled: Vec<DocumentId> = self
            .workspace
            .docs()
            .filter(|(_, doc)| doc.is_dirty() && doc.path().is_none())
            .map(|(id, _)| id)
            .collect();
        let build_files: Vec<PathBuf> = self
            .workspace
            .docs()
            .filter(|(_, doc)| doc.is_dirty())
            .filter_map(|(_, doc)| doc.path().map(Path::to_path_buf))
            .collect();
        let errors = self.workspace.save_all();
        for path in &build_files {
            self.refresh_build_menu(path);
        }
        for (name, error) in &errors {
            self.status.error(format!("{name}: {error}"));
        }
        if errors.is_empty() {
            if let Some(first) = untitled.first().copied() {
                // I documenti senza nome vanno salvati uno per uno.
                self.ask_save_as(first);
            } else {
                self.status.info("Tutto salvato");
            }
        }
    }

    fn ask_save_as(&mut self, id: DocumentId) {
        let (name, language) = match self.workspace.get(id) {
            Some(doc) => (doc.display_name().to_string(), doc.language()),
            None => return,
        };
        let dir = self.default_dir();
        let extension = match language {
            Language::Rust => "rs",
            Language::Python => "py",
            Language::JavaScript => "js",
            Language::TypeScript => "ts",
            Language::Java => "java",
            Language::Go => "go",
            Language::Json => "json",
            Language::Toml => "toml",
            Language::Yaml => "yaml",
            Language::Markdown => "md",
            Language::Html => "html",
            Language::Css => "css",
            Language::C => "c",
            Language::Cpp => "cpp",
            Language::Shell => "sh",
            Language::Text => "txt",
        };
        let suggestion = if name.starts_with("senza nome") {
            format!("{}.{extension}", name.replace(' ', "-"))
        } else {
            name
        };
        self.dialogs.prompt = Some(Prompt::new(
            "Salva con nome",
            "Percorso completo del file (le cartelle mancanti vengono create).",
            dir.join(suggestion).display().to_string(),
            PromptAction::SaveAs(id),
        ));
    }

    fn new_file(&mut self) {
        let id = self.workspace.new_document();
        self.focus_editor = true;
        self.status.info(format!("Nuovo documento {}", id));
    }

    fn reload_active(&mut self) {
        let Some(id) = self.workspace.active_id() else {
            return;
        };
        let result = match self.workspace.get_mut(id) {
            Some(doc) => doc.reload(),
            None => return,
        };
        match result {
            Ok(()) => {
                self.status.info("File riletto dal disco");
                self.highlighter.forget(id);
                self.notify_lsp(id);
            }
            Err(e) => self.status.error(format!("{e}")),
        }
    }

    /// Comunica al language server l'ultima versione del testo di un documento,
    /// se è davvero cambiata (usata dopo le modifiche fatte dal programma, non
    /// dalla digitazione: indentazione, sostituzione, formattazione, ricarica).
    fn notify_lsp(&mut self, id: DocumentId) {
        if self.lsp.status() != LspStatus::Ready {
            return;
        }
        let text = self.workspace.get(id).map(|doc| doc.text().to_string());
        if let Some(text) = text {
            self.lsp.did_change(id, &text);
        }
    }

    /// Chiude una tab, chiedendo conferma se ci sono modifiche.
    fn close_tab(&mut self, id: DocumentId) {
        let dirty = self.workspace.get(id).map(|doc| doc.is_dirty());
        match dirty {
            None => {}
            Some(true) => {
                self.dialogs.confirm_close = Some(CloseRequest {
                    docs: vec![id],
                    quit: false,
                    close_project: None,
                });
            }
            Some(false) => {
                self.workspace.close(id);
                self.lsp.did_close(id);
                self.highlighter.forget(id);
                self.cursor_cache.remove(&id);
            }
        }
    }

    /// Chiede cosa fare con i documenti modificati, poi esegue `then`.
    fn close_or_confirm(&mut self, docs: Vec<DocumentId>, quit: bool) {
        if docs.is_empty() {
            if quit {
                self.quit_confirmed = true;
            } else {
                let ids: Vec<DocumentId> = self.workspace.docs().map(|(id, _)| id).collect();
                for id in ids {
                    self.lsp.did_close(id);
                }
                self.workspace.close_all();
                self.highlighter.clear();
                self.cursor_cache.clear();
            }
            return;
        }
        self.dialogs.confirm_close = Some(CloseRequest {
            docs,
            quit,
            close_project: None,
        });
    }

    fn close_all_tabs(&mut self) {
        let dirty = self.workspace.dirty_tabs();
        self.close_or_confirm(dirty, false);
    }

    fn request_quit(&mut self) {
        let dirty = self.workspace.dirty_docs();
        self.close_or_confirm(dirty, true);
    }

    // ---------------------------------------------------------------------
    // Ricerca
    // ---------------------------------------------------------------------

    /// Mostra il pannello "cerca nel progetto" e mette a fuoco la query.
    fn open_search_panel(&mut self) {
        self.prefs.show_search_panel = true;
        self.prefs.mark_dirty();
        self.search.focus_query = true;
    }

    /// Lancia la ricerca nel progetto, se la query (o il progetto) è cambiata.
    ///
    /// Il rilancio manuale non ha un percorso separato: il pulsante *Cerca*
    /// chiama `ProjectSearchState::mark_dirty`, e questa funzione riparte al
    /// frame successivo come per una query appena scritta.
    fn start_search(&mut self) {
        let Some(root) = self.workspace.root().map(|r| r.to_path_buf()) else {
            self.status.warn("Apri un progetto per cercare nei file");
            return;
        };
        if self.search.query.is_empty() {
            self.search.result = None;
            return;
        }
        if !self.search.needs_run(Some(&root)) {
            return;
        }
        let generation = self.search.begin(Some(&root));
        let mut options = self.tree.options.clone();
        // Le preferenze valgono anche per la ricerca: così gli interruttori del
        // pannello ("Nascosti", "Rispetta .gitignore") possono ampliarla fino a
        // coprire *tutti* i file.
        options.show_hidden = self.prefs.show_hidden;
        options.respect_gitignore = self.prefs.respect_gitignore;
        let query = self.search.search_query();
        self.jobs.start_search(generation, root, options, query);
    }

    fn goto_line(&mut self, line: usize) {
        let Some(id) = self.workspace.active_id() else {
            return;
        };
        let Some(doc) = self.workspace.get_mut(id) else {
            return;
        };
        let text = doc.text();
        let lines = Lines::new(text);
        let line = line.min(lines.count().saturating_sub(1));
        let byte = lines.byte_of_line_col(text, line, 0);
        let position = editor::char_of_byte(text, byte);
        self.pending_selection = Some((position, position));
        self.focus_editor = true;
    }

    /// Apre la barra "trova" (o "trova e sostituisci").
    ///
    /// Se l'editor ha una selezione su una sola riga, la propone come testo da
    /// cercare: è il comportamento che ci si aspetta da `Ctrl+F`.
    fn open_find(&mut self, show_replace: bool) {
        self.find.open_with_selection(self.editor.map(|s| s.selection));
        self.find.show_replace = show_replace;
        self.find.stale = true;

        let Some(snapshot) = self.editor else { return };
        let (from, to) = snapshot.selection;
        if to <= from {
            return;
        }
        let Some(doc) = self.workspace.get(snapshot.doc) else {
            return;
        };
        let text = doc.text();
        // Solo una selezione su una riga sola è un buon testo da cercare.
        let (first, last) = snapshot.line_range(text);
        if first != last {
            return;
        }
        let bytes = (
            editor::byte_of_char(text, from),
            editor::byte_of_char(text, to),
        );
        if let Some(slice) = text.get(bytes.0..bytes.1) {
            if slice.chars().count() <= 200 {
                self.find.query = slice.to_string();
                self.find.stale = true;
            }
        }
    }

    fn find_step(&mut self, forward: bool) {
        // Se la barra era già aperta, chi sta navigando è la barra stessa: il
        // focus resta sul campo di ricerca. Se era chiusa (F3 dall'editor) la
        // apriamo e portiamo il focus sull'editor, per mostrare il risultato.
        let was_open = self.find.open;
        self.find.open = true;
        if self.find.stale {
            if let Some(doc) = self.workspace.active() {
                let text = doc.text().to_string();
                self.find.recompute(&text);
            }
        }
        if let Some(range) = self.find.step(forward) {
            self.pending_selection = Some(range);
            if was_open {
                self.find.focus_query = true;
            } else {
                self.focus_editor = true;
            }
        } else {
            self.status.warn("Nessuna corrispondenza");
        }
    }

    /// Sostituisce la corrispondenza attiva (o tutte).
    fn replace(&mut self, all: bool) {
        let Some(id) = self.workspace.active_id() else {
            return;
        };
        let query = SearchQuery {
            text: self.find.query.clone(),
            case_sensitive: self.find.case_sensitive,
            whole_word: self.find.whole_word,
            use_regex: self.find.use_regex,
            ..Default::default()
        };
        let Ok(regex) = query.compile() else {
            self.status.error("Espressione regolare non valida");
            return;
        };
        let replacement = if self.find.use_regex {
            self.find.replace.clone()
        } else {
            // Senza regex il `$` della sostituzione non è un riferimento a gruppo.
            self.find.replace.replace('$', "$$")
        };

        let Some(doc) = self.workspace.get_mut(id) else {
            return;
        };
        if all {
            let text = doc.text().to_string();
            let new_text = regex.replace_all(&text, replacement.as_str()).to_string();
            if new_text == text {
                self.status.warn("Nessuna sostituzione");
                return;
            }
            doc.set_text(new_text);
            self.status.info("Sostituzioni applicate");
        } else {
            let Some((from, to)) = self.find.active_range() else {
                self.status.warn("Nessuna corrispondenza da sostituire");
                return;
            };
            let text = doc.text();
            let bytes = (
                editor::byte_of_char(text, from),
                editor::byte_of_char(text, to),
            );
            doc.buffer_mut().replace_range(bytes.0..bytes.1, &replacement);
            doc.mark_edited();
            self.pending_selection = Some((from, from + replacement.chars().count()));
            self.focus_editor = true;
        }
        self.notify_lsp(id);
        self.find.stale = true;
        if self.find.show_replace {
            self.find.focus_query = true;
        }
    }

    // ---------------------------------------------------------------------
    // Modifiche alle righe
    // ---------------------------------------------------------------------

    fn line_op(&mut self, op: LineOp) {
        let Some(id) = self.workspace.active_id() else {
            return;
        };
        let snapshot = self.editor.filter(|s| s.doc == id);
        let Some(doc) = self.workspace.get_mut(id) else {
            return;
        };

        // Se l'editor non ha ancora una selezione (primo frame), uso il cursore.
        let selection = snapshot.map(|s| s.selection).unwrap_or((0, 0));
        let lines = Lines::new(doc.text());
        let (first, _) = lines.line_col_of_char(doc.text(), selection.0);
        let (last, _) = lines.line_col_of_char(doc.text(), selection.1);

        let unit = doc.language().indent().unit();
        let prefix = doc.language().line_comment();
        let (new_first, new_last) = match op {
            LineOp::Indent => editor::indent_lines(doc.text_mut(), first, last, unit, false),
            LineOp::Dedent => editor::indent_lines(doc.text_mut(), first, last, unit, true),
            LineOp::Comment => match prefix {
                Some(prefix) => editor::toggle_comment(doc.text_mut(), first, last, prefix),
                None => {
                    self.status
                        .warn(format!("{} non ha commenti di riga", doc.language().name()));
                    return;
                }
            },
            LineOp::Duplicate => editor::duplicate_lines(doc.text_mut(), first, last),
            LineOp::MoveUp => editor::move_lines(doc.text_mut(), first, last, true),
            LineOp::MoveDown => editor::move_lines(doc.text_mut(), first, last, false),
            LineOp::Delete => {
                let at = editor::delete_lines(doc.text_mut(), first, last);
                (at, at)
            }
        };

        // Selezione = righe toccate.
        let text = doc.text();
        let lines = Lines::new(text);
        let from = lines.byte_of_line_col(text, new_first, 0);
        let to = if new_last + 1 < lines.count() {
            lines.byte_of_line_col(text, new_last + 1, 0)
        } else {
            text.len()
        };
        self.pending_selection = Some((
            editor::char_of_byte(text, from),
            editor::char_of_byte(text, to),
        ));
        self.focus_editor = true;
        self.notify_lsp(id);
    }

    /// Formatta il documento attivo.
    ///
    /// Prima scelta: il formatter esterno del linguaggio (`rustfmt`, `black`…),
    /// se installato. Se non c'è, si ripiega sul **language server**
    /// (`textDocument/formatting`): è così che i file Java si formattano con
    /// `jdtls` senza dover installare `google-java-format`.
    fn format_active(&mut self) {
        let Some(id) = self.workspace.active_id() else {
            return;
        };
        let Some(doc) = self.workspace.get(id) else {
            return;
        };
        let language = doc.language();

        // 1. Formatter esterno, se c'è ed è installato.
        if ride_core::format::formatter_for(language).is_some()
            && ride_core::format::can_format(language)
        {
            let text = doc.text().to_string();
            self.pending_formats += 1;
            self.jobs.start_format(id, language, text);
            self.status.info("Formattazione in corso…");
            return;
        }

        // 2. Language server, se sa formattare.
        if crate::lsp::server_can_format(language) {
            let name = server_for(language).map(|c| c.name).unwrap_or("il language server");
            match self.lsp.status() {
                LspStatus::Ready => {}
                LspStatus::Starting => {
                    self.status
                        .warn(format!("{name} si sta ancora avviando, riprova tra un istante"));
                    return;
                }
                _ => {
                    self.status.warn(format!(
                        "Serve {name} (language server) oppure un formatter esterno: nessuno dei due è disponibile"
                    ));
                    return;
                }
            }
            let indent = language.indent();
            let insert_spaces = !matches!(indent, ride_core::Indent::Tabs);
            if self
                .lsp
                .request_formatting(id, indent.width() as usize, insert_spaces)
            {
                self.status.info(format!("Formattazione con {name}…"));
            } else {
                self.status
                    .warn("Formattazione già in corso per questo documento");
            }
            return;
        }

        // 3. Niente da fare: spieghiamo cosa manca.
        match ride_core::format::formatter_name(language) {
            Some(name) => self
                .status
                .error(format!("{name} non è installato")),
            None => self.status.warn(format!(
                "Nessun formatter configurato per {}",
                language.name()
            )),
        }
    }

    // ---------------------------------------------------------------------
    // Comandi di build (Maven / Gradle)
    // ---------------------------------------------------------------------

    /// Avvia un comando di build (Maven/Gradle) nella radice del progetto.
    ///
    /// `index` individua la voce in [`ride_core::build::commands`]. L'output
    /// compare nella console, che si apre da sola.
    fn run_build(&mut self, tool: BuildTool, index: usize) {
        let Some(command) = ride_core::build::commands(tool).get(index) else {
            return;
        };
        self.launch_build(tool, command.args);
    }

    /// Avvia un goal di plugin, con le sue opzioni.
    ///
    /// Gli argomenti si separano sugli spazi: `dependency:tree -Dverbose` sono
    /// due argomenti. Non c'è gestione delle virgolette — non serve per i goal
    /// e le proprietà, che non contengono spazi.
    fn run_build_args(&mut self, tool: BuildTool, args: &str) {
        let args: Vec<&str> = args.split_whitespace().collect();
        if args.is_empty() {
            return;
        }
        self.launch_build(tool, &args);
    }

    /// Chiede all'utente quali argomenti passare al sistema di build.
    fn ask_build_args(&mut self, tool: BuildTool) {
        self.dialogs.prompt = Some(
            Prompt::new(
                format!("Comando {}", tool.name()),
                "Goal di un plugin e sue opzioni, separati da spazi",
                "",
                PromptAction::RunBuildArgs(tool),
            )
            .with_placeholder("es. dependency:tree -Dverbose"),
        );
    }

    /// Il corpo comune: controlla che si possa partire e lancia il comando.
    fn launch_build(&mut self, tool: BuildTool, args: &[&str]) {
        if self.build.running {
            self.build.open = true;
            self.status.warn("Un comando di build è già in esecuzione");
            return;
        }
        let Some(root) = self.workspace.root().map(|r| r.to_path_buf()) else {
            self.status.warn("Nessun progetto aperto");
            return;
        };
        let toolchain = ride_core::build::toolchain_for(&root, tool);
        if !toolchain.available() {
            self.status.error(format!(
                "{} non è installato (serve «{}» nel PATH o il wrapper nel progetto)",
                tool.name(),
                tool.program()
            ));
            return;
        }

        let args = toolchain.arguments(args);
        let mut label = toolchain.label();
        for arg in &args {
            label.push(' ');
            label.push_str(arg);
        }
        self.build.start(label);
        // Ricorda che ci sono modifiche non salvate: la build legge i file su
        // disco, non il testo in memoria.
        let dirty = self.workspace.dirty_count();
        if dirty > 0 {
            self.build.push(format!(
                "⚠ {dirty} documento/i con modifiche non salvate: la build userà la versione su disco"
            ));
        }
        self.build
            .push(format!("$ {} {}", toolchain.program.display(), args.join(" ")));
        self.jobs
            .start_build(root, toolchain.program, args, self.build.cancel.clone());
        self.status.info(format!("{} in esecuzione…", tool.name()));
    }

    /// Interrompe il comando di build in corso.
    fn stop_build(&mut self) {
        if self.build.running {
            self.build.request_stop();
            self.status.info("Interruzione richiesta…");
        }
    }

    // ---------------------------------------------------------------------
    // Terminale integrato
    // ---------------------------------------------------------------------

    /// Mostra o nasconde il terminale. Al primo avvio parte lo shell nella
    /// cartella del progetto attivo.
    fn toggle_terminal(&mut self) {
        if self.terminal.open {
            self.hide_terminal();
            return;
        }
        self.terminal.auto_hide = self.prefs.terminal_auto_hide;
        let root = self.workspace.root().map(Path::to_path_buf);
        self.terminal.open(root.as_deref());
        if let Some(error) = self.terminal.error() {
            self.status.error(error.to_string());
        }
    }

    /// Nasconde il pannello lasciando vivo lo shell. Se era il terminale ad
    /// avere il fuoco, la tastiera torna all'editor.
    fn hide_terminal(&mut self) {
        if self.terminal.focused {
            self.focus_editor = true;
        }
        self.terminal.close();
    }

    /// Riavvia lo shell (utile dopo un comando che ha lasciato il terminale in
    /// uno stato strano o quando cambia la cartella di lavoro).
    fn restart_terminal(&mut self) {
        let root = self.workspace.root().map(Path::to_path_buf);
        self.terminal.restart(root.as_deref());
        if let Some(error) = self.terminal.error() {
            self.status.error(error.to_string());
        }
    }

    // ---------------------------------------------------------------------
    // Language server (LSP)
    // ---------------------------------------------------------------------

    /// Accende il server del linguaggio attivo, se serve e se è disponibile.
    ///
    /// Gira una volta per frame: apre il server quando il documento in primo
    /// piano ha un linguaggio supportato, e lo riavvia quando si passa a un
    /// linguaggio diverso (un solo server alla volta).
    fn sync_lsp(&mut self) {
        if !self.prefs.lsp {
            return;
        }
        let Some(root) = self.workspace.root().map(|r| r.to_path_buf()) else {
            return;
        };
        let Some(language) = self.workspace.active().map(|d| d.language()) else {
            return;
        };
        if server_for(language).is_none() {
            return;
        }
        let need_start = match self.lsp.status() {
            LspStatus::Off => self.lsp_tried != Some(language),
            // Il server è legato a una cartella: se si cambia progetto va
            // riavviato anche quando il linguaggio resta lo stesso.
            LspStatus::Starting | LspStatus::Ready => {
                self.lsp.language() != Some(language)
                    || self.lsp.project_root() != Some(root.as_path())
            }
            // Un errore non si ritenta da solo (eviterebbe un ciclo di avvii):
            // serve *Riavvia il language server*.
            LspStatus::Failed => false,
        };
        if need_start {
            self.lsp_tried = Some(language);
            if LspManager::server_available(language) {
                let _ = self.lsp.start(&root, language);
            } else if let Some(config) = server_for(language) {
                self.status
                    .warn(format!("{} non è installato: niente LSP", config.program));
            }
        }
        if !self.lsp.status().is_ready() {
            return;
        }
        // Apriamo (una sola volta ciascuno) i documenti del *linguaggio del
        // server*: un server conosce un solo linguaggio, non tutti quelli
        // aperti a fianco. Ci limitiamo a quelli non ancora aperti, così non
        // ricopiamo il testo a ogni frame.
        let server_language = self.lsp.language();
        let open: Vec<(DocumentId, PathBuf, Language, String)> = self
            .workspace
            .docs()
            .filter(|(id, doc)| {
                server_language == Some(doc.language())
                    && doc.path().is_some()
                    && !self.lsp.is_open(*id)
            })
            .map(|(id, doc)| {
                (
                    id,
                    doc.path().unwrap().to_path_buf(),
                    doc.language(),
                    doc.text().to_string(),
                )
            })
            .collect();
        for (id, path, language, text) in open {
            self.lsp.did_open(id, &path, language, &text);
        }
    }

    /// Raccoglie gli eventi del server (diagnostica, definizioni, log).
    fn poll_lsp(&mut self) {
        for event in self.lsp.poll() {
            match event {
                LspEvent::Definition {
                    path,
                    line,
                    character,
                } => {
                    self.open_file(&path);
                    self.goto_position(line, character);
                }
                LspEvent::Formatted { doc, text } => self.apply_lsp_format(doc, text),
                LspEvent::Log(message) => self.status.warn(message),
                LspEvent::Diagnostics(id) => self.refresh_problems(id),
                LspEvent::Status => {}
            }
        }
    }

    /// Applica il testo formattato dal language server.
    fn apply_lsp_format(&mut self, doc: DocumentId, text: String) {
        let changed = self
            .workspace
            .get(doc)
            .is_some_and(|active| active.text() != text);
        let Some(active) = self.workspace.get_mut(doc) else {
            return;
        };
        active.set_text(text);
        // `notify_lsp` vuole riprendere `self`: prima si chiude il prestito.
        if changed {
            self.notify_lsp(doc);
            self.status.info("Documento formattato");
        } else {
            self.status.info("Il documento è già formattato");
        }
    }

    /// Decide se aprire il popup dei problemi dopo un aggiornamento della
    /// diagnostica: sì se ci sono **errori** e l'insieme non è quello che
    /// l'utente ha già chiuso. Solo per il documento in primo piano.
    fn refresh_problems(&mut self, id: DocumentId) {
        if self.workspace.active_id() != Some(id) {
            return;
        }
        if self.lsp.diagnostics(id).is_empty() {
            // Problemi spariti: quando si ripresenteranno il popup dovrà poter
            // ricomparire, anche se sono gli stessi di prima.
            self.dialogs.problems_ack = 0;
            return;
        }
        let diagnostics = self.lsp.diagnostics(id);
        let has_error = diagnostics
            .iter()
            .any(|d| d.severity == crate::lsp::Severity::Error);
        let signature = crate::lsp::signature(diagnostics);
        if !has_error || signature == self.dialogs.problems_ack {
            return;
        }
        self.dialogs.problems = true;
    }

    /// Porta il cursore sul problema che inizia al carattere indicato, riaprendo
    /// i blocchi piegati che lo nascondono.
    fn goto_diagnostic(&mut self, start: usize) {
        let Some(id) = self.workspace.active_id() else {
            return;
        };
        let Some(doc) = self.workspace.get_mut(id) else {
            return;
        };
        // Blocchi piegati che contengono la riga del problema: vanno riaperti,
        // altrimenti il cursore verrebbe riportato fuori (riga nascosta).
        let openings: Vec<usize> = {
            let text = doc.text();
            let lines = Lines::new(text);
            let (line, _) = lines.line_col_of_char(text, start);
            let blocks = ride_core::folds::fold_blocks(text);
            doc.folds()
                .iter()
                .copied()
                .filter(|open| {
                    blocks
                        .iter()
                        .any(|b| b.open_byte == *open && b.hidden_lines().contains(&line))
                })
                .collect()
        };
        for open in openings {
            doc.unfold(open);
        }
        self.pending_selection = Some((start, start));
        self.focus_editor = true;
    }

    /// Sposta cursore e selezione su (riga, colonna UTF-16) del documento attivo.
    fn goto_position(&mut self, line: usize, character: usize) {
        let Some(id) = self.workspace.active_id() else {
            return;
        };
        let Some(doc) = self.workspace.get(id) else {
            return;
        };
        let text = doc.text();
        let position = char_offset(text, line, character);
        self.pending_selection = Some((position, position));
        self.focus_editor = true;
    }

    /// Chiede al server la definizione del simbolo sotto il cursore.
    fn goto_definition(&mut self) {
        let Some(snapshot) = self.editor else {
            self.status.warn("Nessun documento attivo");
            return;
        };
        if !self.lsp.status().is_ready() {
            self.status.warn("Il language server non è pronto");
            return;
        }
        self.lsp.request_definition(snapshot.doc, snapshot.selection.0);
    }

    /// Spegne e riaccende il server del linguaggio attivo.
    fn restart_lsp(&mut self) {
        self.lsp.shutdown();
        let Some(root) = self.workspace.root().map(|r| r.to_path_buf()) else {
            self.status.warn("Apri un progetto prima di avviare il language server");
            return;
        };
        let Some(language) = self.workspace.active().map(|d| d.language()) else {
            self.status.warn("Apri un file per avviare il language server");
            return;
        };
        self.lsp_tried = Some(language);
        match self.lsp.start(&root, language) {
            Ok(name) => self.status.info(format!("{name} avviato")),
            Err(e) => self.status.warn(e),
        }
    }

    // ---------------------------------------------------------------------
    // Dialoghi
    // ---------------------------------------------------------------------

    fn default_dir(&self) -> PathBuf {
        if let Some(root) = self.workspace.root() {
            return root.to_path_buf();
        }
        if let Some(path) = self.workspace.active().and_then(|d| d.path()) {
            if let Some(parent) = path.parent() {
                return parent.to_path_buf();
            }
        }
        std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
    }

    fn open_folder_dialog(&mut self) {
        let start = self.default_dir();
        #[cfg(feature = "native-dialogs")]
        {
            // Con le finestre native il percorso lo sceglie il sistema.
            if let Some(folder) = rfd::FileDialog::new()
                .set_title("Apri una cartella come progetto")
                .set_directory(&start)
                .pick_folder()
            {
                self.dispatch_quiet(Action::OpenFolder(folder));
            }
        }
        #[cfg(not(feature = "native-dialogs"))]
        {
            self.dialogs.prompt = Some(Prompt::new(
                "Apri cartella",
                "Cartella da aprire come progetto.",
                start.display().to_string(),
                PromptAction::OpenFolder,
            ));
        }
    }

    fn open_file_dialog(&mut self) {
        let start = self.default_dir();
        #[cfg(feature = "native-dialogs")]
        {
            if let Some(file) = rfd::FileDialog::new()
                .set_title("Apri file")
                .set_directory(&start)
                .pick_file()
            {
                self.dispatch_quiet(Action::OpenPath(file));
            }
        }
        #[cfg(not(feature = "native-dialogs"))]
        {
            self.dialogs.prompt = Some(Prompt::new(
                "Apri file",
                "Percorso del file da aprire.",
                start.display().to_string(),
                PromptAction::OpenFile,
            ));
        }
    }

    /// Applica il risultato di un dialogo di percorso.
    fn apply_prompt(&mut self, prompt: Prompt) {
        let value = prompt.value.trim().to_string();
        if value.is_empty() {
            return;
        }
        let path = PathBuf::from(&value);
        match prompt.action {
            PromptAction::OpenFolder => self.dispatch_quiet(Action::OpenFolder(path)),
            PromptAction::OpenFile => self.dispatch_quiet(Action::OpenPath(path)),
            PromptAction::RunBuildArgs(tool) => self.run_build_args(tool, &value),
            PromptAction::SaveAs(id) => {
                let result = match self.workspace.get_mut(id) {
                    Some(doc) => doc.save_as(&path).map_err(|e| e.to_string()),
                    None => return,
                };
                match result {
                    Ok(()) => {
                        let name = self
                            .workspace
                            .get(id)
                            .map(|d| d.display_name().to_string())
                            .unwrap_or_default();
                        self.status.info(format!("Salvato come {name}"));
                        if let Some(file) = self
                            .workspace
                            .get(id)
                            .and_then(|d| d.path().map(|p| p.to_path_buf()))
                        {
                            if let Some(root) = self.workspace.root() {
                                if file.starts_with(root) {
                                    self.tree.refresh();
                                    self.tree.reveal(&file);
                                }
                            }
                        }
                        self.highlighter.forget(id);
                        // Il percorso (e quindi l'URI) è cambiato: riapriamo il
                        // documento al nuovo indirizzo al prossimo frame.
                        self.lsp.did_close(id);
                    }
                    Err(e) => self.status.error(e),
                }
            }
            PromptAction::NewFile(dir) => {
                let target = dir.join(&value);
                if target.exists() {
                    self.status.warn(format!("{} esiste già", target.display()));
                    return;
                }
                match std::fs::write(&target, b"") {
                    Ok(()) => {
                        self.tree.refresh();
                        self.tree.reveal(&target);
                        self.open_file(&target);
                    }
                    Err(e) => self.status.error(format!("{e}")),
                }
            }
            PromptAction::NewFolder(dir) => {
                let target = dir.join(&value);
                match std::fs::create_dir_all(&target) {
                    Ok(()) => {
                        self.tree.refresh();
                        self.tree.reveal(&target);
                        self.status.info(format!("Cartella {} creata", target.display()));
                    }
                    Err(e) => self.status.error(format!("{e}")),
                }
            }
            PromptAction::Rename(old) => {
                let Some(parent) = old.parent() else { return };
                let target = parent.join(&value);
                if target == old {
                    return;
                }
                if target.exists() {
                    self.status.warn(format!("{} esiste già", target.display()));
                    return;
                }
                match std::fs::rename(&old, &target) {
                    Ok(()) => {
                        // Se il file era aperto, la tab deve seguirlo.
                        let ids: Vec<DocumentId> = self
                            .workspace
                            .docs()
                            .filter(|(_, doc)| doc.path() == Some(old.as_path()))
                            .map(|(id, _)| id)
                            .collect();
                        for id in ids {
                            if let Some(doc) = self.workspace.get_mut(id) {
                                doc.adopt_path(target.clone());
                            }
                            // Il documento cambia URI: lo riapriamo al nuovo
                            // percorso al prossimo frame.
                            self.lsp.did_close(id);
                        }
                        self.tree.refresh();
                        self.tree.reveal(&target);
                        self.status.info(format!("Rinominato in {}", target.display()));
                    }
                    Err(e) => self.status.error(format!("{e}")),
                }
            }
        }
    }

    fn delete_path(&mut self, path: PathBuf) {
        let result = if path.is_dir() {
            std::fs::remove_dir_all(&path)
        } else {
            std::fs::remove_file(&path)
        };
        match result {
            Ok(()) => {
                // Chiudo le tab che puntavano al file eliminato.
                let ids: Vec<DocumentId> = self
                    .workspace
                    .docs()
                    .filter(|(_, doc)| doc.path().is_some_and(|p| p.starts_with(&path)))
                    .map(|(id, _)| id)
                    .collect();
                for id in ids {
                    self.workspace.close(id);
                    self.lsp.did_close(id);
                    self.highlighter.forget(id);
                }
                self.tree.refresh();
                self.status.info(format!("{} eliminato", path.display()));
            }
            Err(e) => self.status.error(format!("{e}")),
        }
    }

    fn open_quick_open(&mut self) {
        self.dialogs.quick_open.open = true;
        self.dialogs.quick_open.focus = true;
        self.dialogs.quick_open.selected = 0;
        let root = self.workspace.root().map(|r| r.to_path_buf());
        if let Some(root) = root {
            if self.dialogs.quick_open.root.as_deref() != Some(root.as_path())
                || self.dialogs.quick_open.files.is_empty()
            {
                self.dialogs.quick_open.root = Some(root.clone());
                self.dialogs.quick_open.loading = true;
                self.dialogs.quick_open.generation += 1;
                let generation = self.dialogs.quick_open.generation;
                let options = self.tree.options.clone();
                self.jobs.start_list_files(generation, root, options);
            }
        } else {
            self.status.warn("Apri un progetto per usare l'apertura rapida");
        }
    }

    fn toggle_syntax_theme(&mut self) {
        // Solo i temi che si accordano con lo sfondo attuale: un tema chiaro su
        // fondo scuro renderebbe il codice illeggibile.
        let names = self.highlighter.theme_names_for(self.prefs.dark);
        if names.is_empty() {
            return;
        }
        let current = self.highlighter.theme_name().to_string();
        let next = match names.iter().position(|name| *name == current) {
            Some(index) => names[(index + 1) % names.len()].clone(),
            // Il tema corrente non si legge su questo sfondo: si passa al primo
            // compatibile.
            None => names[0].clone(),
        };
        self.highlighter.set_theme(&next);
        self.prefs.syntax_theme = Some(next);
    }

    fn toggle_theme(&mut self) {
        self.prefs.dark = !self.prefs.dark;
        self.prefs.mark_dirty();
        // Il tema di sintassi scelto si tiene se si legge anche sul nuovo
        // sfondo; altrimenti si torna al predefinito del tema chiaro/scuro.
        self.sync_syntax_theme();
        self.highlighter.clear();
    }

    /// Allinea il tema di sintassi al tema chiaro/scuro dell'interfaccia.
    ///
    /// Se quello corrente non si legge sul nuovo sfondo, si passa al
    /// predefinito; in ogni caso lo si registra nelle preferenze.
    fn sync_syntax_theme(&mut self) {
        let dark = self.prefs.dark;
        let current = self.highlighter.theme_name().to_string();
        if !self.highlighter.theme_matches(&current, dark) {
            self.highlighter.set_default_for(dark);
        }
        self.prefs.syntax_theme = Some(self.highlighter.theme_name().to_string());
    }

    fn change_font_size(&mut self, delta: f32) {
        let size = (self.prefs.font_size + delta).clamp(8.0, 40.0);
        if size != self.prefs.font_size {
            self.prefs.font_size = size;
            self.prefs.mark_dirty();
            self.highlighter.clear();
        }
    }

    // ---------------------------------------------------------------------
    // Background
    // ---------------------------------------------------------------------

    fn poll_jobs(&mut self, ctx: &Context) {
        while let Some(message) = self.jobs.try_search() {
            self.search.finish(message.generation, message.outcome);
            ctx.request_repaint();
        }
        while let Some(message) = self.jobs.try_format() {
            self.pending_formats = self.pending_formats.saturating_sub(1);
            match message.outcome {
                Ok(text) => {
                    if let Some(doc) = self.workspace.get_mut(message.doc) {
                        doc.set_text(text);
                    }
                    self.notify_lsp(message.doc);
                    self.status.info("Documento formattato");
                }
                Err(e) => self.status.error(e),
            }
            ctx.request_repaint();
        }
        while let Some(message) = self.jobs.try_files() {
            if message.generation == self.dialogs.quick_open.generation {
                self.dialogs.quick_open.files = message.files;
                self.dialogs.quick_open.loading = false;
            }
            ctx.request_repaint();
        }
        while let Some(event) = self.jobs.try_build() {
            match event {
                BuildEvent::Line(line) => self.build.push(line),
                BuildEvent::Finished { success, cancelled } => {
                    self.build.running = false;
                    self.build.last = Some(success);
                    let label = self.build.label.clone();
                    if cancelled {
                        self.status.warn(format!("{label}: interrotto"));
                    } else if success {
                        self.status.info(format!("{label}: completato"));
                    } else {
                        self.status.error(format!("{label}: non riuscito"));
                    }
                }
            }
            ctx.request_repaint();
        }
        let terminal_changed = self.terminal.poll();
        if let Some(message) = self.terminal.take_exit_notice() {
            self.status.warn(message);
        }
        if terminal_changed {
            ctx.request_repaint();
        }
    }

    fn handle_dropped_files(&mut self, ctx: &Context) {
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        for file in dropped {
            let path = file.path().to_path_buf();
            if path.is_dir() {
                self.dispatch_quiet(Action::OpenFolder(path));
            } else {
                self.dispatch_quiet(Action::OpenPath(path));
            }
        }
    }

    // ---------------------------------------------------------------------
    // Dispatch
    // ---------------------------------------------------------------------

    /// Esegue un'azione che non dipende dal contesto egui.
    pub fn dispatch_quiet(&mut self, action: Action) {
        self.dispatch(action);
    }

    /// Esegue un'azione.
    pub fn dispatch(&mut self, action: Action) {
        match action {
            Action::NewFile => self.new_file(),
            Action::OpenFileDialog => self.open_file_dialog(),
            Action::OpenFolderDialog => self.open_folder_dialog(),
            Action::OpenPath(path) => self.open_file(&path),
            Action::OpenFolder(path) => {
                if let Err(e) = self.set_project(&path) {
                    self.status.error(format!("{e}"));
                }
            }
            Action::SwitchProject(index) => {
                self.switch_project(index);
            }
            Action::CloseProject => self.close_project(),
            Action::Save => {
                if let Some(id) = self.workspace.active_id() {
                    self.save_document(id);
                }
            }
            Action::SaveAs => {
                if let Some(id) = self.workspace.active_id() {
                    self.ask_save_as(id);
                }
            }
            Action::SaveAll => self.save_all(),
            Action::ReloadFile => self.reload_active(),
            Action::CloseTab => {
                if let Some(id) = self.workspace.active_id() {
                    self.close_tab(id);
                }
            }
            Action::CloseTabAt(index) => {
                let id = self.workspace.docs().nth(index).map(|(id, _)| id);
                if let Some(id) = id {
                    self.close_tab(id);
                }
            }
            Action::CloseOtherTabs => {
                let Some(keep) = self.workspace.active_id() else {
                    return;
                };
                let dirty: Vec<DocumentId> = self
                    .workspace
                    .dirty_tabs()
                    .into_iter()
                    .filter(|id| *id != keep)
                    .collect();
                if dirty.is_empty() {
                    self.workspace.close_others(keep);
                } else {
                    self.status.warn(format!(
                        "{} documento/i con modifiche non salvate",
                        dirty.len()
                    ));
                }
            }
            Action::CloseAllTabs => self.close_all_tabs(),
            Action::Quit => self.request_quit(),
            Action::NextTab => self.workspace.activate_next(),
            Action::PreviousTab => self.workspace.activate_previous(),
            Action::ActivateTab(id) => {
                if self.workspace.set_active(id) {
                    self.focus_editor = true;
                }
            }
            Action::MoveTab(id, to) => self.workspace.move_tab(id, to),
            Action::QuickOpen => self.open_quick_open(),
            Action::GotoLine => {
                self.dialogs.goto_line = true;
                let line = self.cursor_line + 1;
                self.dialogs.goto_line_value = line.to_string();
            }
            Action::RevealActiveFile => {
                let path = self
                    .workspace
                    .active()
                    .and_then(|doc| doc.path().map(|p| p.to_path_buf()));
                match path {
                    Some(path) => {
                        self.prefs.show_tree = true;
                        self.prefs.mark_dirty();
                        self.tree.selected = Some(path.clone());
                        self.tree.reveal(&path);
                    }
                    None => self.status.warn("Il documento non è ancora salvato su disco"),
                }
            }
            Action::Find => self.open_find(false),
            Action::Replace => self.open_find(true),
            Action::ReplaceCurrent => self.replace(false),
            Action::ReplaceAll => self.replace(true),
            Action::FindNext => self.find_step(true),
            Action::FindPrevious => self.find_step(false),
            Action::SearchInProject => self.open_search_panel(),
            Action::Cancel => self.cancel(),
            Action::ToggleComment => self.line_op(LineOp::Comment),
            Action::Indent => self.line_op(LineOp::Indent),
            Action::Dedent => self.line_op(LineOp::Dedent),
            Action::DuplicateLines => self.line_op(LineOp::Duplicate),
            Action::MoveLinesUp => self.line_op(LineOp::MoveUp),
            Action::MoveLinesDown => self.line_op(LineOp::MoveDown),
            Action::DeleteLines => self.line_op(LineOp::Delete),
            Action::FormatDocument => self.format_active(),
            Action::RunBuild(tool, index) => self.run_build(tool, index),
            Action::RunBuildArgs(tool, args) => self.run_build_args(tool, &args),
            Action::AskBuildArgs(tool) => self.ask_build_args(tool),
            Action::ShowBuildConsole => self.build.open = true,
            Action::StopBuild => self.stop_build(),
            Action::ToggleTerminal => self.toggle_terminal(),
            Action::HideTerminal => self.hide_terminal(),
            Action::RestartTerminal => self.restart_terminal(),
            Action::ToggleTerminalAutoHide => {
                self.prefs.terminal_auto_hide = !self.prefs.terminal_auto_hide;
                self.terminal.auto_hide = self.prefs.terminal_auto_hide;
                self.prefs.mark_dirty();
                let state = if self.prefs.terminal_auto_hide {
                    "attiva"
                } else {
                    "disattivata"
                };
                self.status.info(format!("Autoscomparsa del terminale {state}"));
            }
            Action::ToggleTree => {
                self.prefs.show_tree = !self.prefs.show_tree;
                self.prefs.mark_dirty();
            }
            Action::ToggleSearchPanel => {
                if self.prefs.show_search_panel {
                    self.prefs.show_search_panel = false;
                    self.prefs.mark_dirty();
                } else {
                    self.open_search_panel();
                }
            }
            Action::ToggleHighlight => {
                self.prefs.highlight = !self.prefs.highlight;
                self.prefs.mark_dirty();
                self.highlighter.clear();
            }
            Action::ToggleLineNumbers => {
                self.prefs.line_numbers = !self.prefs.line_numbers;
                self.prefs.mark_dirty();
            }
            Action::ToggleAutoBrackets => {
                self.prefs.auto_brackets = !self.prefs.auto_brackets;
                self.prefs.mark_dirty();
            }
            Action::ToggleBracketMatch => {
                self.prefs.match_brackets = !self.prefs.match_brackets;
                self.prefs.mark_dirty();
            }
            Action::ToggleFolding => {
                self.prefs.folding = !self.prefs.folding;
                self.prefs.mark_dirty();
            }
            Action::NextSyntaxTheme => self.toggle_syntax_theme(),
            Action::NextTheme => self.toggle_theme(),
            Action::ZoomIn => self.change_font_size(1.0),
            Action::ZoomOut => self.change_font_size(-1.0),
            Action::ZoomReset => {
                self.prefs.font_size = 14.0;
                self.prefs.mark_dirty();
                self.highlighter.clear();
            }
            Action::RefreshTree => self.refresh_project(),
            Action::ShowShortcuts => self.dialogs.shortcuts = true,
            Action::ShowAbout => self.dialogs.about = true,
            Action::ShowLspInfo => self.dialogs.lsp_info = true,
            Action::ShowProblems => self.dialogs.problems = true,
            Action::GotoDiagnostic(start) => self.goto_diagnostic(start),
            Action::RestartLsp => self.restart_lsp(),
            Action::ToggleLsp => {
                self.prefs.lsp = !self.prefs.lsp;
                self.prefs.mark_dirty();
                if self.prefs.lsp {
                    self.lsp_tried = None;
                    self.status.info("Language server attivo");
                } else {
                    self.lsp.shutdown();
                    self.lsp_tried = None;
                    self.status.info("Language server disattivato");
                }
            }
            Action::GotoDefinition => self.goto_definition(),
            Action::DeletePath(path) => self.delete_path(path),
            Action::GotoLineAt(line) => self.goto_line(line.saturating_sub(1)),
            Action::ResolveClose(choice) => self.resolve_close(choice),
        }
    }

    /// Conclusione del dialogo sui documenti modificati.
    fn resolve_close(&mut self, choice: CloseChoice) {
        if choice.save {
            let errors = self.workspace.save_all();
            let untitled: Vec<DocumentId> = choice
                .docs
                .iter()
                .copied()
                .filter(|id| {
                    self.workspace
                        .get(*id)
                        .is_some_and(|doc| doc.is_dirty() && doc.path().is_none())
                })
                .collect();
            if let Some((name, error)) = errors.first() {
                self.status.error(format!("{name}: {error}"));
                return;
            }
            if let Some(first) = untitled.first().copied() {
                self.status.warn("Salva il documento nuovo per poter uscire");
                self.ask_save_as(first);
                return;
            }
        }
        for id in &choice.docs {
            self.workspace.close(*id);
            self.lsp.did_close(*id);
            self.highlighter.forget(*id);
            self.cursor_cache.remove(id);
        }
        if choice.quit {
            self.quit_confirmed = true;
            self.dialogs.confirm_close = None;
            return;
        }
        self.dialogs.confirm_close = None;
        // Chiudere un progetto porta via anche le sue tab.
        if let Some(root) = choice.close_project {
            if let Some(index) = self.workspace.index_of_project(&root) {
                self.close_project_now(index);
            }
        }
    }

    /// `Esc`: chiude il dialogo o la barra di ricerca aperta.
    fn cancel(&mut self) {
        if self.dialogs.prompt.is_some()
            || self.dialogs.confirm_close.is_some()
            || self.dialogs.confirm_delete.is_some()
            || self.dialogs.goto_line
            || self.dialogs.shortcuts
            || self.dialogs.about
            || self.dialogs.lsp_info
            || self.dialogs.quick_open.open
        {
            self.dialogs = Default::default();
            return;
        }
        if self.dialogs.problems {
            // Chiudere il popup è un "preso atto": gli stessi problemi non lo
            // riaprono da soli (per questo non si azzera `problems_ack` come
            // farebbe il ramo qui sopra).
            self.dialogs.problems = false;
            if let Some(id) = self.workspace.active_id() {
                self.dialogs.problems_ack = crate::lsp::signature(self.lsp.diagnostics(id));
            }
            return;
        }
        if self.find.open {
            self.find.close();
            self.focus_editor = true;
            return;
        }
        if self.prefs.show_search_panel {
            self.prefs.show_search_panel = false;
            self.prefs.mark_dirty();
        }
    }

    // ---------------------------------------------------------------------
    // Cursore
    // ---------------------------------------------------------------------

    /// Riga/colonna del cursore, con cache sul cambio di posizione.
    ///
    /// Il testo viene letto dal documento attivo, così i chiamanti non devono
    /// tenere in vita un prestito su `workspace`.
    pub(crate) fn cursor_position(&mut self, id: DocumentId, caret: usize) -> (usize, usize) {
        if let Some(entry) = self.cursor_cache.get(&id) {
            if entry.caret == caret {
                return (entry.line, entry.column);
            }
        }
        let Some(doc) = self.workspace.get(id) else {
            return (0, 0);
        };
        let text = doc.text();
        let lines = Lines::new(text);
        let (line, column) = lines.line_col_of_char(text, caret);
        self.cursor_cache.insert(
            id,
            CursorCache {
                caret,
                line,
                column,
            },
        );
        (line, column)
    }
}

impl eframe::App for RideApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.frame += 1;
        let now = Instant::now();
        // `Context` è condiviso (`Arc`): lo cloniamo per poter usare `ui` in modo
        // mutabile per i pannelli e avere comunque accesso al contesto dopo.
        let ctx = ui.ctx().clone();

        // Lo stile di egui si riapplica solo quando qualcosa è cambiato.
        self.prefs.apply(&ctx);

        // Chiusura della finestra: prima si salva, poi si esce.
        if ctx.input(|i| i.viewport().close_requested())
            && !self.quit_confirmed
            && self.workspace.dirty_count() > 0
        {
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            self.request_quit();
        }

        self.poll_jobs(&ctx);
        self.poll_lsp();
        self.handle_dropped_files(&ctx);

        // 1. Scorciatoie da tastiera: applicate subito, così il frame corrente
        //    ne tiene già conto (es. la selezione impostata da "trova").
        //    Con un dialogo aperto la tastiera appartiene al dialogo: passa
        //    solo `Esc`, che lo chiude.
        let dialog_open = self.dialogs.any_open();
        let terminal_focused = self.terminal.focused;
        let shortcuts = if terminal_focused {
            // Il terminale è un programma a sé: non deve perdere Ctrl+C,
            // Ctrl+W e simili perché l'editor li ha consumati.
            Action::collect_terminal(&ctx)
        } else {
            Action::collect(&ctx)
        };
        for action in shortcuts {
            if dialog_open && !matches!(action, Action::Cancel) {
                continue;
            }
            self.dispatch(action);
        }

        // 2. Disegno: le azioni prodotte dai widget vengono raccolte e applicate
        //    in blocco alla fine, per non incrociare i prestiti di `self`.
        //    Prima si riallinea l'albero al progetto attivo: una scorciatoia o
        //    un file appena aperto possono averlo cambiato.
        self.sync_shown_project();
        let mut actions: Vec<Action> = Vec::new();

        egui::Panel::top("ride-menu").show(ui, |ui| {
            actions.extend(ui::menu::bar(ui, self));
        });
        egui::Panel::top("ride-tabs").show(ui, |ui| {
            actions.extend(ui::tabs::bar(ui, self));
        });
        egui::Panel::bottom("ride-status").show(ui, |ui| {
            ui::status::bar(ui, self, now, &mut actions);
        });
        if self.terminal.open {
            egui::Panel::bottom("ride-terminal")
                .resizable(true)
                .default_size(260.0)
                .size_range(110.0..=720.0)
                .show(ui, |ui| {
                    actions.extend(ui::terminal::panel(ui, self));
                });
        }
        if self.prefs.show_tree {
            egui::Panel::left("ride-tree")
                .resizable(true)
                .default_size(260.0)
                .size_range(160.0..=560.0)
                .show(ui, |ui| {
                    actions.extend(ui::tree::panel(ui, self));
                });
        }
        if self.prefs.show_search_panel {
            egui::Panel::right("ride-search")
                .resizable(true)
                .default_size(420.0)
                .size_range(280.0..=900.0)
                .show(ui, |ui| {
                    actions.extend(ui::search::panel(ui, self));
                });
        }
        egui::CentralPanel::default().show(ui, |ui| {
            actions.extend(ui::editor::panel(ui, self));
        });

        if let Some(prompt) = ui::dialogs::windows(&ctx, self, &mut actions) {
            self.apply_prompt(prompt);
        }

        for action in actions {
            self.dispatch(action);
        }

        // Language server: apre o riavvia il server giusto per il documento
        // attivo (e gli comunica i documenti aperti).
        self.sync_lsp();

        // Ricerca nel progetto: parte da sola quando la query cambia. Il
        // pannello visibile è l'unica condizione: se è aperto (anche da
        // *Visualizza* o ripristinato dalle preferenze) la ricerca deve girare.
        if self.prefs.show_search_panel {
            if let Some(root) = self.workspace.root().map(|r| r.to_path_buf()) {
                if !self.search.query.is_empty() && self.search.needs_run(Some(&root)) {
                    self.start_search();
                }
            }
        }

        // 3. Ripensare: se ci sono lavori in corso o messaggi da far scadere.
        let busy = self.search.running
            || self.dialogs.quick_open.loading
            || self.pending_formats > 0
            || self.build.running
            || self.terminal.is_running()
            // Durante l'handshake vogliamo animare lo stato; a regime la GUI
            // viene risvegliata dai thread del client quando arriva qualcosa.
            || self.lsp.status() == LspStatus::Starting
            || self.status.is_alive(now);
        if busy {
            ctx.request_repaint_after(std::time::Duration::from_millis(120));
        }

        if self.quit_confirmed {
            ctx.send_viewport_cmd(ViewportCommand::Close);
        }
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        self.prefs.store(storage);
        // La sessione è una fotografia del lavoro: si scrive insieme alle
        // preferenze, e come loro si rilegge all'avvio.
        storage.set_string(SESSION_KEY, self.session().encode());
        storage.flush();
    }

    fn on_exit(&mut self) {
        self.lsp.shutdown();
        self.terminal.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ride_core::Workspace;

    fn app_for_test() -> RideApp {
        RideApp {
            workspace: Workspace::new(),
            prefs: Prefs::default(),
            tree: TreeState::default(),
            trees: HashMap::new(),
            project_colors: HashMap::new(),
            find: FindState::default(),
            search: ProjectSearchState::default(),
            dialogs: Default::default(),
            status: Status::default(),
            jobs: Jobs::new(),
            highlighter: Highlighter::new(None),
            lsp: LspManager::new(),
            editor: None,
            cursor_line: 0,
            pending_selection: None,
            focus_editor: false,
            lsp_tried: None,
            toolchains: Vec::new(),
            maven_plugins: Vec::new(),
            build: BuildState::default(),
            terminal: TerminalState::default(),
            cursor_cache: HashMap::new(),
            pending_formats: 0,
            quit_confirmed: false,
            frame: 0,
        }
    }

    #[test]
    fn commuta_il_tema_senza_perdere_il_documento() {
        let mut app = app_for_test();
        app.workspace.new_document();
        let dark = app.prefs.dark;
        app.toggle_theme();
        assert_ne!(app.prefs.dark, dark);
        assert_eq!(app.workspace.len(), 1);
    }

    /// Ciclare i temi di sintassi (o passare da chiaro a scuro) non deve mai
    /// lasciare un tema che si legge male sullo sfondo dell'editor.
    #[test]
    fn cambiare_tema_non_rende_illeggibile_il_codice() {
        let mut app = app_for_test();
        app.workspace.new_document();
        for _ in 0..8 {
            app.dispatch(Action::NextSyntaxTheme);
            assert!(
                app.highlighter
                    .theme_matches(app.highlighter.theme_name(), app.prefs.dark),
                "tema {} con dark = {}",
                app.highlighter.theme_name(),
                app.prefs.dark
            );
        }
        for _ in 0..2 {
            app.dispatch(Action::NextTheme);
            assert!(
                app.highlighter
                    .theme_matches(app.highlighter.theme_name(), app.prefs.dark),
                "tema {} con dark = {}",
                app.highlighter.theme_name(),
                app.prefs.dark
            );
        }
    }

    /// Un tema salvato che non si accorda con lo sfondo viene corretto: non
    /// deve arrivare sullo schermo.
    #[test]
    fn un_tema_salvato_che_stona_viene_corretto() {
        let mut app = app_for_test();
        app.prefs.dark = true;
        app.highlighter.set_theme("InspiredGitHub"); // tema chiaro su sfondo scuro
        app.sync_syntax_theme();
        assert!(app
            .highlighter
            .theme_matches(app.highlighter.theme_name(), true));
        assert_eq!(
            app.prefs.syntax_theme.as_deref(),
            Some(app.highlighter.theme_name())
        );

        app.prefs.dark = false;
        app.highlighter.set_theme("base16-ocean.dark"); // tema scuro su sfondo chiaro
        app.sync_syntax_theme();
        assert!(app
            .highlighter
            .theme_matches(app.highlighter.theme_name(), false));
    }

    /// Il cambio di tema ridisegna l'editor (e svuota la cache delle galley)
    /// senza rompere nulla.
    #[test]
    fn l_editor_si_disegna_nei_due_temi() {
        let ctx = Context::default();
        let mut app = app_for_test();
        let root = temp_project("tema-frame");
        let file = root.join("main.rs");
        std::fs::write(&file, "fn main() { let x = 42; } // ciao\n").unwrap();
        app.workspace.open_path(&file).unwrap();
        app.focus_editor = true;

        for _ in 0..3 {
            frame_editor(&ctx, &mut app, vec![]);
            app.dispatch(Action::NextTheme);
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn il_pannello_di_ricerca_si_apre_anche_dal_menu() {
        let mut app = app_for_test();
        assert!(!app.prefs.show_search_panel);

        // Aperto dal menu *Visualizza* il pannello deve essere pronto a
        // cercare: la visibilità è l'unica condizione (regressione: una
        // seconda bandiera restava spenta e la ricerca non partiva mai).
        app.dispatch(Action::ToggleSearchPanel);
        assert!(app.prefs.show_search_panel);

        let root = std::env::temp_dir().join(format!("ride-app-search-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("a.rs"), "fn main() {}\n").unwrap();
        app.dispatch(Action::OpenFolder(root.clone()));

        app.search.query.text = "main".into();
        let project = app.workspace.root().map(|r| r.to_path_buf());
        assert!(app.search.needs_run(project.as_deref()));

        // `Esc` chiude il pannello.
        app.dispatch(Action::Cancel);
        assert!(!app.prefs.show_search_panel);

        let _ = std::fs::remove_dir_all(&root);
    }

    /// Lancia la ricerca e aspetta che il thread finisca, restituendo il numero
    /// di risultati trovati.
    fn conta_risultati_di_ricerca(app: &mut RideApp) -> usize {
        app.start_search();
        // La ricerca gira su un thread: la aspettiamo raccogliendo i risultati.
        let ctx = Context::default();
        let deadline = Instant::now() + std::time::Duration::from_secs(10);
        while app.search.running && Instant::now() < deadline {
            app.poll_jobs(&ctx);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        app.search
            .result
            .as_ref()
            .map(|r| r.hits.len())
            .expect("nessun risultato")
    }

    #[test]
    fn la_ricerca_nel_progetto_riporta_i_risultati() {
        let mut app = app_for_test();
        let root = std::env::temp_dir().join(format!("ride-app-search-run-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/a.rs"), "fn main() {}\n").unwrap();
        std::fs::write(root.join("b.md"), "nessuna corrispondenza\n").unwrap();

        app.dispatch(Action::OpenFolder(root.clone()));
        app.dispatch(Action::SearchInProject);
        app.search.query.text = "main".into();

        assert_eq!(conta_risultati_di_ricerca(&mut app), 1);
        let hit = &app.search.result.as_ref().unwrap().hits[0];
        assert!(hit.path.ends_with("a.rs"));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn la_ricerca_puo_includere_i_file_nascosti() {
        let mut app = app_for_test();
        let root = std::env::temp_dir().join(format!("ride-app-hidden-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("visibile.rs"), "token\n").unwrap();
        std::fs::write(root.join(".nascosto.rs"), "token\n").unwrap();

        app.dispatch(Action::OpenFolder(root.clone()));
        app.dispatch(Action::SearchInProject);
        app.search.query.text = "token".into();

        // Di default i file nascosti restano fuori.
        assert_eq!(conta_risultati_di_ricerca(&mut app), 1);

        // Con la preferenza attiva la ricerca arriva su *tutti* i file.
        app.prefs.show_hidden = true;
        app.search.mark_dirty();
        assert_eq!(conta_risultati_di_ricerca(&mut app), 2);

        let _ = std::fs::remove_dir_all(&root);
    }

    /// Aprire più cartelle le tiene aperte tutte insieme: le linguette in cima
    /// all'albero servono a passare dall'una all'altra senza perdere il segno.
    #[test]
    fn piu_progetti_aperti_e_linguette_per_cambiarli() {
        let mut app = app_for_test();
        let a = temp_project("multi-a");
        let b = temp_project("multi-b");
        for (root, file) in [(&a, "a.rs"), (&b, "b.rs")] {
            std::fs::create_dir_all(root.join("src")).unwrap();
            std::fs::write(root.join("src").join(file), "fn main() {}\n").unwrap();
        }

        // Due cartelle: restano aperte entrambe, la seconda è quella attiva.
        app.dispatch(Action::OpenFolder(a.clone()));
        app.dispatch(Action::OpenFolder(b.clone()));
        assert_eq!(app.workspace.project_count(), 2);
        assert_eq!(app.workspace.active_project_index(), Some(1));
        assert_eq!(app.tree.root.as_ref().unwrap().path, canonical(&b));

        // Passando al primo, l'albero diventa il suo. La cartella `src` che
        // espandiamo resta espansa anche dopo un giro sull'altro progetto.
        app.dispatch(Action::SwitchProject(0));
        assert_eq!(app.workspace.active_project_index(), Some(0));
        assert_eq!(app.tree.root.as_ref().unwrap().path, canonical(&a));
        espandi(&mut app.tree, "src");
        assert!(e_espanso(&app.tree, "src"));

        app.dispatch(Action::SwitchProject(1));
        assert_eq!(app.tree.root.as_ref().unwrap().path, canonical(&b));
        assert!(!e_espanso(&app.tree, "src"));
        app.dispatch(Action::SwitchProject(0));
        assert!(e_espanso(&app.tree, "src"), "l'albero non è tornato com'era");

        // "Aprire" di nuovo la cartella già attiva non ricostruisce l'albero.
        app.dispatch(Action::OpenFolder(a.clone()));
        assert!(e_espanso(&app.tree, "src"));
        assert_eq!(app.workspace.project_count(), 2);

        // Ripetere l'apertura di una cartella già aperta non la duplica:
        // la si riattiva e basta.
        app.dispatch(Action::OpenFolder(b.clone()));
        assert_eq!(app.workspace.project_count(), 2);
        assert_eq!(app.workspace.active_project_index(), Some(1));
        assert_eq!(app.tree.root.as_ref().unwrap().path, canonical(&b));

        // Chiudere il progetto attivo dà il posto all'altro.
        app.dispatch(Action::CloseProject);
        assert_eq!(app.workspace.project_count(), 1);
        assert_eq!(app.tree.root.as_ref().unwrap().path, canonical(&a));
        assert!(e_espanso(&app.tree, "src"));

        // Chiuso anche quello, non resta nessun progetto né albero.
        app.dispatch(Action::CloseProject);
        assert!(app.workspace.project_count() == 0);
        assert!(app.tree.root.is_none());

        let _ = (std::fs::remove_dir_all(&a), std::fs::remove_dir_all(&b));
    }

    /// Ogni progetto mostra i *suoi* file aperti: cambiando linguetta cambiano
    /// le tab, e chiudere il progetto le porta via con sé.
    #[test]
    fn cambiare_progetto_mostra_i_suoi_file_aperti() {
        let mut app = app_for_test();
        let a = temp_project("tab-progetto-a");
        let b = temp_project("tab-progetto-b");
        std::fs::write(a.join("a.rs"), "fn a() {}\n").unwrap();
        std::fs::write(b.join("b.rs"), "fn b() {}\n").unwrap();

        app.dispatch(Action::OpenFolder(a.clone()));
        app.dispatch(Action::OpenPath(a.join("a.rs")));
        assert_eq!(app.workspace.len(), 1);
        let ta = app.workspace.active_id().unwrap();

        // Nel secondo progetto si parte senza tab: quelle di A non si vedono.
        app.dispatch(Action::OpenFolder(b.clone()));
        assert!(app.workspace.is_empty());
        app.dispatch(Action::OpenPath(b.join("b.rs")));
        assert_eq!(app.workspace.len(), 1);
        let tb = app.workspace.active_id().unwrap();
        assert_ne!(ta, tb);

        // Tornando ad A si rivede la sua tab (e B tiene la sua).
        app.dispatch(Action::SwitchProject(0));
        assert_eq!(app.workspace.len(), 1);
        assert_eq!(app.workspace.active_id(), Some(ta));
        app.dispatch(Action::SwitchProject(1));
        assert_eq!(app.workspace.active_id(), Some(tb));

        // Un file di A aperto mentre è attivo B porta con sé il progetto: le
        // tab di A tornano in vista e l'albero lo segue.
        app.dispatch(Action::SwitchProject(1));
        app.dispatch(Action::OpenPath(a.join("a.rs")));
        assert_eq!(app.workspace.active_project_index(), Some(0));
        assert_eq!(app.workspace.active_id(), Some(ta));
        assert_eq!(app.tree.root.as_ref().unwrap().path, canonical(&a));

        // Chiudendo B se ne vanno le sue tab, non quelle di A.
        app.dispatch(Action::SwitchProject(1));
        app.dispatch(Action::CloseProject);
        assert_eq!(app.workspace.project_count(), 1);
        assert_eq!(app.workspace.active_id(), Some(ta));
        assert!(app.workspace.get(tb).is_none());

        let _ = (std::fs::remove_dir_all(&a), std::fs::remove_dir_all(&b));
    }

    /// Chiudere un progetto con modifiche non salvate chiede prima conferma.
    #[test]
    fn chiudere_un_progetto_con_modifiche_chiede_conferma() {
        let mut app = app_for_test();
        let a = temp_project("chiudi-sporco-a");
        let b = temp_project("chiudi-sporco-b");
        std::fs::write(a.join("a.rs"), "fn a() {}\n").unwrap();
        std::fs::write(b.join("b.rs"), "fn b() {}\n").unwrap();

        app.dispatch(Action::OpenFolder(a.clone()));
        app.dispatch(Action::OpenPath(a.join("a.rs")));
        app.dispatch(Action::OpenFolder(b.clone()));
        app.dispatch(Action::OpenPath(b.join("b.rs")));

        // Sporca il file di B e prova a chiudere B.
        let tb = app.workspace.active_id().unwrap();
        app.workspace.get_mut(tb).unwrap().text_mut().push('x');
        app.dispatch(Action::CloseProject);
        // Niente è stato chiuso: prima si chiede.
        assert_eq!(app.workspace.project_count(), 2);
        assert!(app.dialogs.confirm_close.is_some());

        // "Salva e chiudi" porta via B e la sua tab; il file finisce su disco.
        app.dispatch(Action::ResolveClose(CloseChoice {
            docs: vec![tb],
            quit: false,
            save: true,
            close_project: Some(canonical(&b)),
        }));
        assert_eq!(app.workspace.project_count(), 1);
        assert_eq!(app.tree.root.as_ref().unwrap().path, canonical(&a));
        assert!(app.workspace.get(tb).is_none());
        assert_eq!(std::fs::read_to_string(b.join("b.rs")).unwrap(), "fn b() {}\nx");

        let _ = (std::fs::remove_dir_all(&a), std::fs::remove_dir_all(&b));
    }

    /// Alla chiusura si scrive com'era la scrivania; al prossimo avvio si
    /// ritrovano le stesse cartelle, le loro tab, gli alberi e il progetto
    /// attivo.
    #[test]
    fn la_sessione_si_ritrova_al_prossimo_avvio() {
        let mut app = app_for_test();
        let a = temp_project("sessione-a");
        let b = temp_project("sessione-b");
        for (root, file) in [(&a, "a.rs"), (&a, "anche.rs")] {
            std::fs::write(root.join(file), "fn f() {}\n").unwrap();
        }
        std::fs::create_dir_all(a.join("src")).unwrap();
        std::fs::write(a.join("src/lib.rs"), "pub fn l() {}\n").unwrap();
        std::fs::create_dir_all(b.join("gui")).unwrap();
        std::fs::write(b.join("b.rs"), "fn b() {}\n").unwrap();

        app.dispatch(Action::OpenFolder(a.clone()));
        app.project_colors
            .insert(canonical(&a), Color32::from_rgb(0x12, 0xab, 0xff));
        app.dispatch(Action::OpenPath(a.join("a.rs")));
        // Due tab in A, ma in primo piano resta `a.rs`.
        let primo = app.workspace.active_id().unwrap();
        app.dispatch(Action::OpenPath(a.join("anche.rs")));
        app.dispatch(Action::ActivateTab(primo));
        // In A si apre `src` e si sceglie un file che ci sta dentro.
        espandi(&mut app.tree, "src");
        app.tree.selected = Some(canonical(&a).join("src/lib.rs"));

        app.dispatch(Action::OpenFolder(b.clone()));
        app.project_colors
            .insert(canonical(&b), Color32::from_rgb(0xe0, 0x78, 0x55));
        app.dispatch(Action::OpenPath(b.join("b.rs")));
        espandi(&mut app.tree, "gui");
        let testo = app.session().encode();

        // Un editor appena avviato: niente progetti, niente tab.
        let mut nuovo = app_for_test();
        assert!(nuovo.workspace.is_empty());
        nuovo.restore_session_text(&testo);

        assert_eq!(nuovo.workspace.project_count(), 2);
        assert_eq!(
            nuovo.project_colors.get(&canonical(&a)),
            Some(&Color32::from_rgb(0x12, 0xab, 0xff))
        );
        assert_eq!(
            nuovo.project_colors.get(&canonical(&b)),
            Some(&Color32::from_rgb(0xe0, 0x78, 0x55))
        );
        // L'ultimo progetto attivo era B, ed è da B che si riparte.
        assert_eq!(nuovo.workspace.active_project_index(), Some(1));
        assert_eq!(nuovo.tree.root.as_ref().unwrap().path, canonical(&b));
        assert_eq!(nuovo.workspace.len(), 1);
        assert_eq!(
            nuovo.workspace.active().and_then(|d| d.path()),
            Some(canonical(&b).join("b.rs").as_path())
        );
        // E l'albero di B è aperto dov'era.
        assert!(e_espanso(&nuovo.tree, "gui"));

        // A ritrova le sue due tab, con quella che era in primo piano, e
        // l'albero con `src` aperto e il file scelto.
        nuovo.dispatch(Action::SwitchProject(0));
        assert_eq!(nuovo.workspace.len(), 2);
        assert_eq!(
            nuovo.workspace.active().and_then(|d| d.path()),
            Some(canonical(&a).join("a.rs").as_path())
        );
        assert!(e_espanso(&nuovo.tree, "src"));
        assert_eq!(nuovo.tree.selected, Some(canonical(&a).join("src/lib.rs")));
        // `src` è stato letto: dentro c'è il file che era selezionato.
        assert!(
            nuovo
                .tree
                .root
                .as_ref()
                .unwrap()
                .find(&canonical(&a).join("src/lib.rs"))
                .is_some()
        );

        let _ = (std::fs::remove_dir_all(&a), std::fs::remove_dir_all(&b));
    }

    #[test]
    fn una_sessione_rovinata_non_impedisce_l_avvio() {
        let mut app = app_for_test();
        app.restore_session_text("P/cartella/che/non/esiste\nF/cartella/che/non/esiste/x.rs\n?boh");
        assert_eq!(app.workspace.project_count(), 0);
        assert!(app.workspace.is_empty());
        // Una sessione vuota non tocca niente.
        app.restore_session_text("");
        assert_eq!(app.workspace.project_count(), 0);
        assert!(app.workspace.is_empty());
    }

    #[test]
    fn disattivare_il_lsp_spegne_il_server() {
        let mut app = app_for_test();
        assert!(app.prefs.lsp);
        app.dispatch(Action::ToggleLsp);
        assert!(!app.prefs.lsp);
        assert_eq!(app.lsp.status(), LspStatus::Off);
    }

    #[test]
    fn vai_alla_posizione_utf16() {
        let mut app = app_for_test();
        let id = app.workspace.new_document();
        if let Some(doc) = app.workspace.get_mut(id) {
            doc.set_text("aaa\nbbb\n");
        }
        // Riga 1, colonna 2 → indice di carattere 4 + 2 = 6.
        app.goto_position(1, 2);
        assert_eq!(app.pending_selection, Some((6, 6)));
        assert!(app.focus_editor);
    }

    /// Regressione: digitare deve aggiornare il testo *e* segnare il documento
    /// come modificato. `TextEdit` scrive direttamente nel buffer, quindi senza
    /// `mark_edited` la revisione restava ferma e l'evidenziazione continuava a
    /// mostrare la galley vecchia (il carattere digitato non compariva).
    #[test]
    fn digitare_aggiorna_testo_e_revisione() {
        let ctx = Context::default();
        let mut app = app_for_test();
        let id = app.workspace.new_document();
        app.focus_editor = true;

        // Primo frame: l'editor chiede il focus (valido dal frame successivo).
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            crate::ui::editor::panel(ui, &mut app);
        });
        output.textures_delta.clear();

        let revision_before = app.workspace.get(id).unwrap().revision();

        // Secondo frame: arriva un carattere dalla tastiera.
        let input = egui::RawInput {
            events: vec![egui::Event::Text("x".to_string())],
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| {
            crate::ui::editor::panel(ui, &mut app);
        });
        output.textures_delta.clear();

        let doc = app.workspace.get(id).unwrap();
        assert_eq!(doc.text(), "x");
        assert!(doc.is_dirty());
        assert!(doc.revision() > revision_before);
    }

    /// Esegue un frame dell'editor con gli eventi indicati.
    fn frame_editor(ctx: &Context, app: &mut RideApp, events: Vec<egui::Event>) {
        let input = egui::RawInput {
            events,
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| {
            crate::ui::editor::panel(ui, app);
        });
        output.textures_delta.clear();
    }

    /// Esegue un frame del terminale con gli eventi indicati.
    fn frame_terminal(ctx: &Context, app: &mut RideApp, events: Vec<egui::Event>) {
        let input = egui::RawInput {
            events,
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| {
            crate::ui::terminal::panel(ui, app);
        });
        output.textures_delta.clear();
    }

    #[test]
    fn il_terminale_si_disegna_senza_rompere_nulla() {
        let ctx = Context::default();
        let mut app = app_for_test();
        // Un terminale aperto ma senza processo: l'intestazione e uno schermo
        // vuoto devono comunque disegnarsi.
        app.terminal.open = true;
        frame_terminal(&ctx, &mut app, Vec::new());
        frame_terminal(&ctx, &mut app, Vec::new());

        // E il pannello si nasconde con l'azione corrispondente.
        app.dispatch(Action::HideTerminal);
        assert!(!app.terminal.open);
    }

    #[cfg(unix)]
    #[test]
    fn il_terminale_avviato_dal_menu_si_disegna() {
        let ctx = Context::default();
        let mut app = app_for_test();
        app.terminal.set_repaint_context(ctx.clone());
        app.dispatch(Action::ToggleTerminal);
        assert!(app.terminal.open);
        assert!(app.terminal.has_session());
        for _ in 0..3 {
            frame_terminal(&ctx, &mut app, Vec::new());
        }
    }

    #[test]
    fn le_parentesi_si_chiudono_da_sole() {
        let ctx = Context::default();
        let mut app = app_for_test();
        let id = app.workspace.new_document();
        app.focus_editor = true;
        frame_editor(&ctx, &mut app, vec![]);
        // Un solo carattere digitato: viene inserita anche la chiusura.
        frame_editor(&ctx, &mut app, vec![egui::Event::Text("(".into())]);

        assert_eq!(app.workspace.get(id).unwrap().text(), "()");
        // Il cursore resta *fra* le due parentesi.
        assert_eq!(app.editor.as_ref().unwrap().selection, (1, 1));
        assert!(app.workspace.get(id).unwrap().is_dirty());
    }

    #[test]
    fn la_chiusura_esistente_viene_scavalcata() {
        let ctx = Context::default();
        let mut app = app_for_test();
        let id = app.workspace.new_document();
        if let Some(doc) = app.workspace.get_mut(id) {
            doc.set_text("()");
        }
        app.focus_editor = true;
        frame_editor(&ctx, &mut app, vec![]);
        crate::ui::editor::select(&ctx, id, 1, 1);
        let revision = app.workspace.get(id).unwrap().revision();

        frame_editor(&ctx, &mut app, vec![egui::Event::Text(")".into())]);

        // Nessun `))`, il cursore passa oltre la chiusura.
        assert_eq!(app.workspace.get(id).unwrap().text(), "()");
        assert_eq!(app.editor.as_ref().unwrap().selection, (2, 2));
        // Il testo non è cambiato davvero: nessuna nuova revisione.
        assert_eq!(app.workspace.get(id).unwrap().revision(), revision);
    }

    #[test]
    fn invio_dentro_le_graffe_apre_il_blocco() {
        let ctx = Context::default();
        let mut app = app_for_test();
        let id = app.workspace.new_document();
        if let Some(doc) = app.workspace.get_mut(id) {
            doc.set_text("{}");
        }
        app.focus_editor = true;
        frame_editor(&ctx, &mut app, vec![]);
        crate::ui::editor::select(&ctx, id, 1, 1);

        let enter = egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        frame_editor(&ctx, &mut app, vec![enter]);

        // La graffa chiusura va a capo, il cursore resta indentato.
        assert_eq!(app.workspace.get(id).unwrap().text(), "{\n    \n}");
        assert_eq!(app.editor.as_ref().unwrap().selection, (6, 6));
    }

    #[test]
    fn la_dimensione_del_font_resta_nei_limiti() {
        let mut app = app_for_test();
        for _ in 0..100 {
            app.change_font_size(1.0);
        }
        assert_eq!(app.prefs.font_size, 40.0);
        for _ in 0..100 {
            app.change_font_size(-1.0);
        }
        assert_eq!(app.prefs.font_size, 8.0);
    }

    #[test]
    fn la_piegatura_si_puo_disattivare() {
        let mut app = app_for_test();
        assert!(app.prefs.folding);
        app.dispatch(Action::ToggleFolding);
        assert!(!app.prefs.folding);
    }

    /// Un errore nuovo apre il popup dei problemi; una volta chiuso non si
    /// riapre finché i problemi restano gli stessi.
    #[test]
    fn un_errore_lsp_apre_il_popup_dei_problemi() {
        use crate::lsp::{Diagnostic, Severity};

        let mut app = app_for_test();
        let id = app.workspace.new_document();
        let error = |message: &str| Diagnostic {
            start: 0,
            end: 1,
            line: 0,
            severity: Severity::Error,
            message: message.to_string(),
        };

        app.lsp.set_diagnostics(id, vec![error("boom")]);
        app.refresh_problems(id);
        assert!(app.dialogs.problems, "l'errore apre il popup");

        // L'utente lo chiude: gli stessi problemi non lo riaprono.
        app.dialogs.problems = false;
        app.dialogs.problems_ack = crate::lsp::signature(app.lsp.diagnostics(id));
        app.refresh_problems(id);
        assert!(!app.dialogs.problems);

        // Un problema diverso invece sì.
        app.lsp.set_diagnostics(id, vec![error("bang")]);
        app.refresh_problems(id);
        assert!(app.dialogs.problems);

        // Un semplice avviso non apre nulla.
        app.dialogs.problems = false;
        app.dialogs.problems_ack = 0;
        app.lsp.set_diagnostics(
            id,
            vec![Diagnostic {
                severity: Severity::Warning,
                ..error("attenzione")
            }],
        );
        app.refresh_problems(id);
        assert!(!app.dialogs.problems, "gli avvisi da soli non aprono il popup");

        // Se i problemi spariscono e poi tornano gli stessi, il popup riappare.
        app.dialogs.problems = false;
        app.dialogs.problems_ack = 0;
        app.lsp.set_diagnostics(id, vec![error("boom")]);
        app.refresh_problems(id);
        app.dialogs.problems = false;
        app.dialogs.problems_ack = crate::lsp::signature(app.lsp.diagnostics(id));
        app.lsp.set_diagnostics(id, Vec::new());
        app.refresh_problems(id);
        app.lsp.set_diagnostics(id, vec![error("boom")]);
        app.refresh_problems(id);
        assert!(app.dialogs.problems, "tornando, gli stessi errori riaprono");
    }

    /// Il popup si disegna senza sorprese (finestra non modale).
    #[test]
    fn il_popup_dei_problemi_si_disegna() {
        use crate::lsp::{Diagnostic, Severity};

        let ctx = Context::default();
        let mut app = app_for_test();
        let id = app.workspace.new_document();
        app.lsp.set_diagnostics(
            id,
            vec![Diagnostic {
                start: 0,
                end: 1,
                line: 0,
                severity: Severity::Error,
                message: "boom".into(),
            }],
        );
        app.dialogs.problems = true;

        let mut actions = Vec::new();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            crate::ui::dialogs::windows(ui.ctx(), &mut app, &mut actions);
        });
        output.textures_delta.clear();

        assert!(app.dialogs.problems);
        assert!(actions.is_empty());
    }

    /// Saltare a un problema dentro un blocco piegato lo riapre.
    #[test]
    fn saltare_a_un_problema_riapre_il_blocco_piegato() {
        let mut app = app_for_test();
        let id = app.workspace.new_document();
        let text = "fn main() {\n    let x = 1;\n    let y = 2;\n}\n";
        let open = text.find('{').unwrap();
        if let Some(doc) = app.workspace.get_mut(id) {
            doc.set_text(text);
            doc.toggle_fold(open);
        }
        let start = text.find("let y").unwrap();

        app.goto_diagnostic(start);

        assert!(app.workspace.get(id).unwrap().folds().is_empty());
        assert_eq!(app.pending_selection, Some((start, start)));
        assert!(app.focus_editor);
    }

    /// Il cursore non deve restare "dentro" un blocco piegato: lì il testo non
    /// si vede. Viene riportato all'inizio della riga nascosta.
    #[test]
    fn il_cursore_esce_dai_blocchi_piegati() {
        let ctx = Context::default();
        let mut app = app_for_test();
        let id = app.workspace.new_document();
        let text = "fn main() {\n    let x = 1;\n    let y = 2;\n}\n";
        let open = text.find('{').unwrap();
        if let Some(doc) = app.workspace.get_mut(id) {
            doc.set_text(text);
            doc.toggle_fold(open);
        }
        app.focus_editor = true;
        // Un primo frame perché l'editor prenda il focus.
        frame_editor(&ctx, &mut app, vec![]);
        // Il cursore è nella prima riga del blocco (indice 20 = riga 1).
        app.pending_selection = Some((20, 20));
        frame_editor(&ctx, &mut app, vec![]);

        // Riportato all'inizio della riga 1 (carattere 12).
        assert_eq!(app.editor.as_ref().unwrap().selection, (12, 12));
    }

    /// Regressione: una selezione che attraversa un blocco piegato non deve far
    /// andare in panico `egui` (che ridisegna la selezione riga per riga e, per
    /// le righe nascoste, ha la mesh svuotata).
    #[test]
    fn selezionare_attraverso_un_blocco_piegato_non_va_in_panico() {
        let ctx = Context::default();
        let mut app = app_for_test();
        let id = app.workspace.new_document();
        let text = "fn main() {\n    let x = 1;\n    let y = 2;\n}\n";
        let open = text.find('{').unwrap();
        if let Some(doc) = app.workspace.get_mut(id) {
            doc.set_text(text);
            doc.toggle_fold(open);
        }
        app.focus_editor = true;
        frame_editor(&ctx, &mut app, vec![]);

        // Selezione da prima del blocco fino a una riga nascosta.
        app.pending_selection = Some((0, 20));
        frame_editor(&ctx, &mut app, vec![]);

        // Il cursore (l'estremo mobile) è stato riportato fuori dal blocco.
        assert_eq!(app.editor.as_ref().unwrap().selection, (12, 12));
    }

    /// Cartella temporanea per i test che aprono un progetto.
    fn temp_project(nome: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("ride-build-{}-{nome}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    /// Percorso normalizzato: le radici dei progetti lo sono già, e i confronti
    /// nei test devono reggere anche se la cartella temporanea è un link.
    fn canonical(path: &Path) -> PathBuf {
        path.canonicalize().unwrap()
    }

    /// Espande una cartella figlia della radice dell'albero.
    fn espandi(tree: &mut TreeState, nome: &str) {
        let options = tree.options.clone();
        let Some(root) = tree.root.as_mut() else {
            return;
        };
        let Some(children) = root.children.as_mut() else {
            return;
        };
        if let Some(child) = children.iter_mut().find(|c| c.name == nome) {
            child.load(&options);
            child.expanded = true;
        }
    }

    /// `true` se quella cartella figlia risulta espansa.
    fn e_espanso(tree: &TreeState, nome: &str) -> bool {
        tree.root
            .as_ref()
            .and_then(|root| root.children.as_ref())
            .is_some_and(|children| children.iter().any(|c| c.name == nome && c.expanded))
    }

    /// Attende la fine di un comando di build.
    fn attendi_la_build(app: &mut RideApp) {
        let ctx = Context::default();
        let deadline = Instant::now() + std::time::Duration::from_secs(15);
        while app.build.running && Instant::now() < deadline {
            app.poll_jobs(&ctx);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        // Le ultime righe possono arrivare appena dopo la fine.
        std::thread::sleep(std::time::Duration::from_millis(100));
        app.poll_jobs(&ctx);
    }

    #[test]
    fn il_menu_esegui_compare_solo_per_maven_e_gradle() {
        let mut app = app_for_test();
        let root = temp_project("menu");
        std::fs::write(root.join("a.rs"), "fn main() {}\n").unwrap();
        app.dispatch(Action::OpenFolder(root.clone()));
        assert!(app.toolchains.is_empty());

        std::fs::write(root.join("pom.xml"), "<project/>").unwrap();
        app.dispatch(Action::RefreshTree);
        assert_eq!(app.toolchains.len(), 1);
        assert_eq!(app.toolchains[0].tool, BuildTool::Maven);

        std::fs::write(root.join("build.gradle"), "").unwrap();
        app.dispatch(Action::RefreshTree);
        assert_eq!(app.toolchains.len(), 2);
        assert_eq!(app.toolchains[1].tool, BuildTool::Gradle);

        // Chiudendo il progetto sparisce anche il menu.
        app.dispatch(Action::CloseProject);
        assert!(app.toolchains.is_empty());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn la_build_senza_progetto_avvisa_e_non_parte() {
        let mut app = app_for_test();
        app.dispatch(Action::RunBuild(BuildTool::Maven, 0));
        assert!(!app.build.running);
        assert!(app.build.lines.is_empty());
    }

    #[test]
    fn interrompere_il_comando_ne_chiede_la_fine() {
        let mut app = app_for_test();
        app.build.running = true;
        app.dispatch(Action::StopBuild);
        assert!(app.build.cancel.load(std::sync::atomic::Ordering::Relaxed));
    }

    /// End-to-end: un progetto Maven col wrapper viene eseguito davvero e
    /// l'output compare nella console.
    #[cfg(unix)]
    #[test]
    fn un_progetto_maven_esegue_il_wrapper_del_progetto() {
        use std::os::unix::fs::PermissionsExt;

        let mut app = app_for_test();
        let root = temp_project("maven");
        std::fs::write(root.join("pom.xml"), "<project/>").unwrap();
        let wrapper = root.join("mvnw");
        std::fs::write(&wrapper, "#!/bin/sh\necho compilato \"$@\"\n").unwrap();
        std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();

        app.dispatch(Action::OpenFolder(root.clone()));
        assert_eq!(app.toolchains.len(), 1);

        // La prima voce di menu è "Compila" → goal `compile`.
        app.dispatch(Action::RunBuild(BuildTool::Maven, 0));
        assert!(app.build.running);
        assert!(app.build.open);
        assert!(app.build.label.contains("mvnw"));

        attendi_la_build(&mut app);
        assert!(!app.build.running);
        assert_eq!(app.build.last, Some(true));
        assert!(
            app.build.lines.iter().any(|l| l == "compilato -B compile"),
            "output inatteso: {:?}",
            app.build.lines
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// I plugin proposti nel menu sono quelli che il `pom.xml` dichiara, più i
    /// tre che Maven risolve da sé.
    #[test]
    fn i_plugin_del_menu_vengono_dal_pom() {
        let mut app = app_for_test();
        let root = temp_project("plugin");
        // Un `pom.xml` realistico: parent, dipendenze, `pluginManagement` e
        // profili non devono confondere il riconoscimento.
        std::fs::write(
            root.join("pom.xml"),
            r#"<?xml version="1.0" encoding="UTF-8"?>
<project xmlns="http://maven.apache.org/POM/4.0.0">
  <parent>
    <groupId>org.springframework.boot</groupId>
    <artifactId>spring-boot-starter-parent</artifactId>
    <version>3.3.0</version>
  </parent>
  <artifactId>demo</artifactId>
  <dependencies>
    <dependency>
      <groupId>org.springframework.boot</groupId>
      <artifactId>spring-boot-starter-web</artifactId>
    </dependency>
  </dependencies>
  <build>
    <pluginManagement>
      <plugins>
        <plugin>
          <artifactId>maven-compiler-plugin</artifactId>
        </plugin>
      </plugins>
    </pluginManagement>
    <plugins>
      <plugin>
        <groupId>org.springframework.boot</groupId>
        <artifactId>spring-boot-maven-plugin</artifactId>
      </plugin>
      <plugin>
        <artifactId>spotless-maven-plugin</artifactId>
      </plugin>
    </plugins>
  </build>
  <profiles>
    <profile>
      <build>
        <plugins>
          <plugin>
            <artifactId>maven-surefire-plugin</artifactId>
          </plugin>
        </plugins>
      </build>
    </profile>
  </profiles>
</project>
"#,
        )
        .unwrap();
        app.dispatch(Action::OpenFolder(root.clone()));
        let prefissi: Vec<&str> = app.maven_plugins.iter().map(|p| p.prefix).collect();
        assert_eq!(
            prefissi,
            vec!["spring-boot", "compiler", "surefire", "spotless", "dependency", "help", "versions"]
        );

        // Chiudendo il progetto spariscono anche i plugin.
        app.dispatch(Action::CloseProject);
        assert!(app.maven_plugins.is_empty());

        let _ = std::fs::remove_dir_all(&root);
    }

    /// Salvando il `pom.xml` il menu dei plugin si aggiorna da solo.
    #[test]
    fn salvare_il_pom_aggiorna_i_plugin_del_menu() {
        let mut app = app_for_test();
        let root = temp_project("pom-salvato");
        let pom = root.join("pom.xml");
        std::fs::write(&pom, "<project/>").unwrap();
        app.dispatch(Action::OpenFolder(root.clone()));
        // Senza plugin dichiarati restano solo i tre "sempre utili".
        assert_eq!(app.maven_plugins.len(), 3);

        // Si aggiunge un plugin al `pom.xml` e si salva dall'editor.
        app.workspace.open_path(&pom).unwrap();
        let id = app.workspace.active_id().unwrap();
        app.workspace.get_mut(id).unwrap().set_text(
            "<project><build><plugins><plugin>\
             <artifactId>spotless-maven-plugin</artifactId>\
             </plugin></plugins></build></project>",
        );
        app.dispatch(Action::Save);
        assert!(
            app.maven_plugins.iter().any(|p| p.prefix == "spotless"),
            "plugin non aggiornati: {:?}",
            app.maven_plugins.iter().map(|p| p.prefix).collect::<Vec<_>>()
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// Un goal di plugin finisce sulla riga di comando, con le sue opzioni.
    #[cfg(unix)]
    #[test]
    fn un_goal_di_plugin_si_esegue() {
        use std::os::unix::fs::PermissionsExt;

        let mut app = app_for_test();
        let root = temp_project("goal");
        std::fs::write(root.join("pom.xml"), "<project/>").unwrap();
        let wrapper = root.join("mvnw");
        std::fs::write(&wrapper, "#!/bin/sh\necho eseguito \"$@\"\n").unwrap();
        std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
        app.dispatch(Action::OpenFolder(root.clone()));

        app.dispatch(Action::RunBuildArgs(
            BuildTool::Maven,
            "dependency:tree -Dverbose".to_string(),
        ));
        assert!(app.build.running);
        attendi_la_build(&mut app);
        assert_eq!(app.build.last, Some(true));
        assert!(
            app.build
                .lines
                .iter()
                .any(|l| l == "eseguito -B dependency:tree -Dverbose"),
            "output inatteso: {:?}",
            app.build.lines
        );

        // Senza argomenti non si avvia niente.
        app.dispatch(Action::RunBuildArgs(BuildTool::Maven, "   ".to_string()));
        assert!(!app.build.running);

        let _ = std::fs::remove_dir_all(&root);
    }

    /// La voce "Comando…" apre il dialogo; quel che si scrive viene eseguito.
    #[cfg(unix)]
    #[test]
    fn il_comando_libero_chiede_e_poi_esegue() {
        use std::os::unix::fs::PermissionsExt;

        let mut app = app_for_test();
        let root = temp_project("comando-libero");
        std::fs::write(root.join("pom.xml"), "<project/>").unwrap();
        let wrapper = root.join("mvnw");
        std::fs::write(&wrapper, "#!/bin/sh\necho eseguito \"$@\"\n").unwrap();
        std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
        app.dispatch(Action::OpenFolder(root.clone()));

        app.dispatch(Action::AskBuildArgs(BuildTool::Maven));
        let mut prompt = app.dialogs.prompt.clone().expect("il dialogo si apre");
        assert_eq!(prompt.action, PromptAction::RunBuildArgs(BuildTool::Maven));
        // Il dialogo prende la tastiera (a differenza del popup dei problemi).
        assert!(app.dialogs.any_open());

        prompt.value = "help:effective-pom -Dverbose".to_string();
        app.apply_prompt(prompt);
        assert!(app.build.running);
        attendi_la_build(&mut app);
        assert!(
            app.build
                .lines
                .iter()
                .any(|l| l == "eseguito -B help:effective-pom -Dverbose"),
            "output inatteso: {:?}",
            app.build.lines
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// Il menu «Esegui» si disegna senza sorprese e senza agire da sé.
    #[test]
    fn il_menu_di_build_si_disegna() {
        let ctx = Context::default();
        let mut app = app_for_test();
        let root = temp_project("menu-draw");
        std::fs::write(root.join("pom.xml"), "<project/>").unwrap();
        app.dispatch(Action::OpenFolder(root.clone()));
        assert_eq!(app.toolchains.len(), 1);

        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let actions = crate::ui::menu::bar(ui, &app);
            assert!(actions.is_empty(), "il menu non deve agire da sé");
        });
        output.textures_delta.clear();

        let _ = std::fs::remove_dir_all(&root);
    }

    /// La barra di stato si disegna anche per un file Java (dove il formatter
    /// esterno manca e si passa dal language server).
    #[test]
    fn la_barra_di_stato_indica_il_formatter() {
        let ctx = Context::default();
        let mut app = app_for_test();
        let dir = temp_project("status");
        let file = dir.join("Main.java");
        std::fs::write(&file, "class Main {}\n").unwrap();
        app.workspace.open_path(&file).unwrap();

        let mut actions = Vec::new();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            crate::ui::status::bar(ui, &app, Instant::now(), &mut actions);
        });
        output.textures_delta.clear();

        assert!(actions.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Un documento senza formattatore (né esterno né LSP) avvisa e non fa nulla.
    #[test]
    fn un_documento_senza_formattatore_avvisa() {
        let mut app = app_for_test();
        app.workspace.new_document();
        app.dispatch(Action::FormatDocument);
        let (_, message) = app
            .status
            .current(Instant::now())
            .expect("nessun messaggio di stato");
        assert!(message.contains("Nessun formatter"), "{message}");
    }

    /// End-to-end: senza `google-java-format` la formattazione di un file Java
    /// passa dal language server (qui un server finto, in produzione `jdtls`).
    #[test]
    #[cfg(unix)]
    fn la_formattazione_java_passa_dal_lsp() {
        use crate::lsp::{LanguageServerConfig, LspStatus};

        if !ride_core::format::is_available("python3") {
            return; // niente Python: il test si salta.
        }
        // Se il formatter esterno c'è davvero, questa strada la prende lui.
        if ride_core::format::can_format(Language::Java) {
            return;
        }

        let dir = std::env::temp_dir().join(format!("ride-app-format-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("fake_lsp.py");
        std::fs::write(&script, crate::lsp::FAKE_SERVER).unwrap();
        let file = dir.join("Main.java");
        std::fs::write(&file, "fn main() {}\n").unwrap();

        let mut app = app_for_test();
        let config = LanguageServerConfig {
            name: "finto",
            program: "python3",
            args: &[],
            language_id: "java",
            formatting: true,
        };
        app.lsp
            .start_command(
                config,
                Language::Java,
                &dir,
                "python3".into(),
                vec![script.display().to_string()],
            )
            .unwrap();

        let deadline = Instant::now() + std::time::Duration::from_secs(10);
        while app.lsp.status() != LspStatus::Ready && Instant::now() < deadline {
            app.lsp.poll();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(app.lsp.status(), LspStatus::Ready, "errore: {:?}", app.lsp.error());

        let id = app.workspace.open_path(&file).unwrap();
        app.lsp.did_open(id, &file, Language::Java, "fn main() {}\n");

        app.dispatch(Action::FormatDocument);

        let deadline = Instant::now() + std::time::Duration::from_secs(10);
        while Instant::now() < deadline
            && app.workspace.get(id).unwrap().text() != "FN main() {}\n"
        {
            app.poll_lsp();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        assert_eq!(app.workspace.get(id).unwrap().text(), "FN main() {}\n");
        app.lsp.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// La console di build si disegna anche con output e comando concluso.
    #[test]
    fn la_console_di_build_si_disegna() {
        let ctx = Context::default();
        let mut app = app_for_test();
        app.build.open = true;
        app.build.push("$ mvnw -B compile");
        app.build.push("BUILD SUCCESS");
        app.build.last = Some(true);

        let mut actions = Vec::new();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            crate::ui::dialogs::windows(ui.ctx(), &mut app, &mut actions);
        });
        output.textures_delta.clear();

        assert!(app.build.open);
        assert!(actions.is_empty());
    }
}
