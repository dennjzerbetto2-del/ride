//! Stato dell'interfaccia.
//!
//! egui è *immediate mode*: l'interfaccia viene ridisegnata da zero a ogni
//! frame. Tutto ciò che deve sopravvivere tra un frame e l'altro sta qui, ed è
//! logicamente distinto dal *modello* (che vive in `ride-core`).

use std::hash::{Hash, Hasher};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use ride_core::project::{FileNode, TreeOptions};
use ride_core::search::{self, SearchQuery, SearchResult};
use ride_core::{DocumentId, Language};

/// Quanti risultati di ricerca in-file evidenziare al massimo.
pub const MAX_FIND_MATCHES: usize = 20_000;

// ---------------------------------------------------------------------------
// Albero del progetto
// ---------------------------------------------------------------------------

/// Stato del pannello del progetto.
#[derive(Debug, Default)]
pub struct TreeState {
    /// Radice dell'albero, se c'è un progetto aperto.
    pub root: Option<FileNode>,
    pub options: TreeOptions,
    /// Ultimo elemento cliccato (evidenziato).
    pub selected: Option<PathBuf>,
    /// Percorso da portare in vista al prossimo frame.
    pub scroll_to: Option<PathBuf>,
    /// Errore da mostrare accanto al progetto.
    pub error: Option<String>,
}

impl TreeState {
    /// (Ri)costruisce l'albero per una cartella.
    pub fn load(&mut self, root: &Path) {
        let mut node = FileNode::dir(root.to_path_buf());
        node.load(&self.options);
        node.expanded = true;
        self.root = Some(node);
        self.error = None;
    }

    /// Ricarica i figli mantenendo le cartelle espanse.
    pub fn refresh(&mut self) {
        let options = self.options.clone();
        if let Some(root) = self.root.as_mut() {
            root.refresh(&options);
        }
    }

    /// Espande il cammino fino al file indicato.
    pub fn reveal(&mut self, path: &Path) {
        let options = self.options.clone();
        if let Some(root) = self.root.as_mut() {
            if root.reveal(path, &options) {
                self.scroll_to = Some(path.to_path_buf());
            } else {
                self.error = Some(format!("{} non è (più) nel progetto", path.display()));
            }
        }
    }

    /// Percorsi delle cartelle aperte: è questo che si salva per ritrovare
    /// l'albero com'era.
    pub fn expanded_paths(&self) -> Vec<PathBuf> {
        self.root
            .as_ref()
            .map(|node| node.expanded_paths())
            .unwrap_or_default()
    }

    /// Riapre le cartelle indicate e riseleziona la voce scelta.
    ///
    /// Va chiamata su un albero appena letto: le cartelle si aprono leggendo i
    /// figli solo per il cammino che serve (e solo se il genitore è aperto,
    /// com'era).
    pub fn restore(&mut self, expanded: &[PathBuf], selected: Option<&Path>) {
        let options = self.options.clone();
        if let Some(root) = self.root.as_mut() {
            root.restore_expansion(expanded, &options);
        }
        self.selected = selected.map(Path::to_path_buf);
    }
}

// ---------------------------------------------------------------------------
// Ricerca dentro il file corrente
// ---------------------------------------------------------------------------

/// Stato della barra "trova/sostituisci".
#[derive(Debug, Default)]
pub struct FindState {
    pub open: bool,
    pub query: String,
    pub replace: String,
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub use_regex: bool,
    pub show_replace: bool,
    /// Intervalli di caratteri trovati.
    pub matches: Vec<(usize, usize)>,
    /// Indice della corrispondenza attiva.
    pub current: usize,
    pub error: Option<String>,
    /// Metti a fuoco il campo di ricerca al prossimo frame.
    pub focus_query: bool,
    /// I match vanno ricalcolati (testo o opzioni cambiati).
    pub stale: bool,
    /// Impronta dell'ultima ricerca eseguita.
    last_stamp: u64,
}

impl FindState {
    /// Apre la barra.
    pub fn open_with_selection(&mut self, selection: Option<(usize, usize)>) {
        self.open = true;
        self.focus_query = true;
        self.stale = true;
        if let Some((from, to)) = selection {
            if to > from {
                self.show_replace = false;
            }
        }
    }

    pub fn close(&mut self) {
        self.open = false;
        self.matches.clear();
        self.error = None;
        self.last_stamp = 0;
    }

    /// Impronta della ricerca: se cambia, vanno ricalcolati i risultati.
    pub fn stamp(&self) -> u64 {
        if !self.open || self.query.is_empty() {
            return 0;
        }
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.query.hash(&mut hasher);
        self.case_sensitive.hash(&mut hasher);
        self.whole_word.hash(&mut hasher);
        self.use_regex.hash(&mut hasher);
        hasher.finish() | 1 // 0 è riservato a "nessuna ricerca"
    }

    /// Ricalcola i match se la query (o il testo) è cambiata.
    pub fn ensure_matches(&mut self, text: &str) {
        if self.stale || self.stamp() != self.last_stamp {
            self.recompute(text);
        }
    }

    /// Ricalcola le corrispondenze sul testo corrente.
    pub fn recompute(&mut self, text: &str) {
        let stamp = self.stamp();
        self.last_stamp = stamp;
        self.stale = false;
        self.error = None;
        if stamp == 0 {
            self.matches.clear();
            self.current = 0;
            return;
        }
        let query = SearchQuery {
            text: self.query.clone(),
            case_sensitive: self.case_sensitive,
            whole_word: self.whole_word,
            use_regex: self.use_regex,
            ..Default::default()
        };
        match query.compile() {
            Ok(regex) => {
                self.matches = search::find_all(text, &regex, MAX_FIND_MATCHES);
                if self.current >= self.matches.len() {
                    self.current = 0;
                }
            }
            Err(e) => {
                self.matches.clear();
                self.error = Some(e.to_string());
            }
        }
    }

    /// Va alla corrispondenza successiva/precedente; ritorna l'intervallo da
    /// selezionare.
    pub fn step(&mut self, forward: bool) -> Option<(usize, usize)> {
        if self.matches.is_empty() {
            return None;
        }
        let n = self.matches.len();
        self.current = if forward {
            (self.current + 1) % n
        } else {
            (self.current + n - 1) % n
        };
        Some(self.matches[self.current])
    }

    /// La corrispondenza attiva.
    pub fn active_range(&self) -> Option<(usize, usize)> {
        self.matches.get(self.current).copied()
    }
}

// ---------------------------------------------------------------------------
// Ricerca nel progetto
// ---------------------------------------------------------------------------

/// Stato del pannello "cerca nel progetto".
///
/// La visibilità del pannello è decisa da `Prefs::show_search_panel` (è una
/// preferenza persistente), quindi qui non c'è un secondo flag: due sorgenti di
/// verità finirebbero per contraddirsi e la ricerca non partirebbe.
#[derive(Debug, Default)]
pub struct ProjectSearchState {
    pub query: SearchQuery,
    /// Filtro glob sui file (`*.rs`).
    pub include: String,
    pub result: Option<SearchResult>,
    pub error: Option<String>,
    pub running: bool,
    /// Generazione della ricerca in corso: i risultati vecchi si scartano.
    pub generation: u64,
    pub focus_query: bool,
    /// Impronta dell'ultima ricerca completata.
    last_stamp: u64,
}

impl ProjectSearchState {
    /// Impronta della ricerca corrente (query + opzioni + filtro).
    pub fn stamp(&self, project: Option<&Path>) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.query.text.hash(&mut hasher);
        self.query.case_sensitive.hash(&mut hasher);
        self.query.whole_word.hash(&mut hasher);
        self.query.use_regex.hash(&mut hasher);
        self.include.hash(&mut hasher);
        project.hash(&mut hasher);
        hasher.finish() | 1
    }

    /// Segna la ricerca come "da rifare" (es. dal pulsante *Cerca*).
    pub fn mark_dirty(&mut self) {
        self.last_stamp = 0;
    }

    /// `true` se c'è una ricerca da (ri)lanciare.
    pub fn needs_run(&self, project: Option<&Path>) -> bool {
        !self.running && !self.query.is_empty() && self.stamp(project) != self.last_stamp
    }

    /// Segna come "in esecuzione" e ritorna la generazione assegnata.
    pub fn begin(&mut self, project: Option<&Path>) -> u64 {
        self.last_stamp = self.stamp(project);
        self.generation += 1;
        self.running = true;
        self.error = None;
        self.generation
    }

    /// Accetta un risultato se appartiene alla generazione corrente.
    pub fn finish(&mut self, generation: u64, outcome: Result<SearchResult, String>) {
        if generation != self.generation {
            return;
        }
        self.running = false;
        match outcome {
            Ok(result) => {
                self.result = Some(result);
                self.error = None;
            }
            Err(e) => {
                self.result = None;
                self.error = Some(e);
            }
        }
    }

    /// Applica la query con le opzioni correnti (ricostruendo il filtro glob).
    pub fn search_query(&self) -> SearchQuery {
        let mut query = self.query.clone();
        query.include = if self.include.trim().is_empty() {
            None
        } else {
            Some(self.include.trim().to_string())
        };
        query
    }
}

// ---------------------------------------------------------------------------
// Apertura rapida (Ctrl+P)
// ---------------------------------------------------------------------------

/// Stato della finestra "vai al file".
#[derive(Debug, Default)]
pub struct QuickOpen {
    pub open: bool,
    pub query: String,
    /// Elenco dei file del progetto (riempito in background).
    pub files: Vec<PathBuf>,
    pub loading: bool,
    pub generation: u64,
    pub selected: usize,
    pub focus: bool,
    /// Radice per cui è stato calcolato l'elenco.
    pub root: Option<PathBuf>,
}

impl QuickOpen {
    /// I file che corrispondono alla query, con punteggio decrescente.
    pub fn matches(&self, project_root: Option<&Path>) -> Vec<(i32, &Path)> {
        let mut found: Vec<(i32, &Path)> = self
            .files
            .iter()
            .filter_map(|path| {
                let relative = project_root
                    .and_then(|root| path.strip_prefix(root).ok())
                    .unwrap_or(path.as_path());
                let label = relative.to_string_lossy();
                fuzzy_score(&self.query, &label).map(|score| (score, path.as_path()))
            })
            .collect();
        // Prima i punteggi più alti; a parità, i percorsi più corti.
        found.sort_by(|a, b| {
            b.0.cmp(&a.0)
                .then_with(|| a.1.as_os_str().len().cmp(&b.1.as_os_str().len()))
        });
        found.truncate(200);
        found
    }
}

/// Punteggio di corrispondenza "fuzzy": `None` se `needle` non è una
/// sottosequenza di `haystack`.
///
/// Premia i caratteri consecutivi e quelli che iniziano una parola o un
/// componente del percorso: è il comportamento che rende utile una lista come
/// quella di "vai al file".
pub fn fuzzy_score(needle: &str, haystack: &str) -> Option<i32> {
    if needle.is_empty() {
        return Some(0);
    }
    let needle: Vec<char> = needle.to_lowercase().chars().collect();
    let hay: Vec<char> = haystack.to_lowercase().chars().collect();
    let orig: Vec<char> = haystack.chars().collect();

    let mut score = 0i32;
    let mut hi = 0usize;
    let mut previous_match: Option<usize> = None;

    for (ni, &nc) in needle.iter().enumerate() {
        if nc == ' ' {
            continue;
        }
        let mut found = None;
        while hi < hay.len() {
            if hay[hi] == nc {
                found = Some(hi);
                break;
            }
            hi += 1;
        }
        let index = found?;
        score += 10;
        if let Some(prev) = previous_match {
            if prev + 1 == index {
                score += 15; // caratteri consecutivi
            }
        }
        // Inizio parola o di componente del percorso.
        if index == 0
            || matches!(
                orig.get(index.wrapping_sub(1)),
                Some('/' | '\\' | '_' | '-' | '.' | ' ')
            )
        {
            score += 20;
        }
        // La prima lettera della query che cade all'inizio è un segnale forte.
        if ni == 0 && index == 0 {
            score += 10;
        }
        // Penalizza i buchi "lontani" dalla fine del nome del file.
        score -= (index as i32) / 8;
        previous_match = Some(index);
        hi = index + 1;
    }
    Some(score)
}

// ---------------------------------------------------------------------------
// Dialoghi
// ---------------------------------------------------------------------------

/// Cosa fare quando l'utente conferma il dialogo di percorso.
///
/// Con i dialoghi nativi (`native-dialogs`, attivo di default) le varianti
/// `OpenFolder`/`OpenFile` non servono: restano per la modalità senza finestre
/// di sistema.
#[cfg_attr(feature = "native-dialogs", allow(dead_code))]
#[derive(Debug, Clone, PartialEq)]
pub enum PromptAction {
    /// Apri una cartella come progetto.
    OpenFolder,
    /// Apri un file.
    OpenFile,
    /// Salva il documento con un nuovo percorso.
    SaveAs(DocumentId),
    /// Crea un file dentro la cartella indicata.
    NewFile(PathBuf),
    /// Crea una cartella dentro la cartella indicata.
    NewFolder(PathBuf),
    /// Rinomina un file o una cartella.
    Rename(PathBuf),
    /// Esegue gli argomenti di build indicati (goal e opzioni).
    RunBuildArgs(ride_core::build::BuildTool),
}

/// Un dialogo che chiede un percorso o un nome.
#[derive(Debug, Clone, PartialEq)]
pub struct Prompt {
    pub title: String,
    pub hint: String,
    /// Testo suggerito nel campo vuoto.
    pub placeholder: String,
    pub value: String,
    pub action: PromptAction,
    pub focus: bool,
}

impl Prompt {
    pub fn new(title: impl Into<String>, hint: impl Into<String>, value: impl Into<String>, action: PromptAction) -> Self {
        Prompt {
            title: title.into(),
            hint: hint.into(),
            placeholder: "Percorso o nome".to_string(),
            value: value.into(),
            action,
            focus: true,
        }
    }

    /// Cambia il testo suggerito nel campo vuoto.
    pub fn with_placeholder(mut self, placeholder: impl Into<String>) -> Self {
        self.placeholder = placeholder.into();
        self
    }
}

/// Richiesta di chiusura di documenti con modifiche non salvate.
#[derive(Debug, Clone)]
pub struct CloseRequest {
    pub docs: Vec<DocumentId>,
    /// `true` se dopo la chiusura si deve uscire dall'editor.
    pub quit: bool,
    /// Progetto da chiudere dopo i documenti (le sue tab se ne vanno con lui).
    pub close_project: Option<std::path::PathBuf>,
}

/// Stato dei dialoghi aperti.
#[derive(Debug, Default)]
pub struct DialogState {
    pub prompt: Option<Prompt>,
    pub confirm_close: Option<CloseRequest>,
    pub confirm_delete: Option<PathBuf>,
    pub shortcuts: bool,
    pub about: bool,
    pub lsp_info: bool,
    pub goto_line: bool,
    pub goto_line_value: String,
    pub quick_open: QuickOpen,
    /// Popup dei problemi segnalati dal language server.
    pub problems: bool,
    /// Impronta dei problemi che l'utente ha già visto: finché non cambia, il
    /// popup non si riapre da solo (vedi [`crate::lsp::signature`]).
    pub problems_ack: u64,
}

impl DialogState {
    /// C'è un dialogo che si sta prendendo il focus?
    ///
    /// Il popup dei problemi è di proposito **fuori** da questo elenco: compare
    /// da solo e non deve rubare la tastiera né bloccare le scorciatoie.
    pub fn any_open(&self) -> bool {
        self.prompt.is_some()
            || self.confirm_close.is_some()
            || self.confirm_delete.is_some()
            || self.shortcuts
            || self.about
            || self.lsp_info
            || self.goto_line
            || self.quick_open.open
    }
}

// ---------------------------------------------------------------------------
// Messaggi di stato
// ---------------------------------------------------------------------------

/// Gravità di un messaggio.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Level {
    #[default]
    Info,
    Warn,
    Error,
}

/// Messaggio temporaneo mostrato nella barra di stato.
#[derive(Debug, Default)]
pub struct Status {
    message: Option<String>,
    level: Level,
    until: Option<Instant>,
}

impl Status {
    /// Mostra un messaggio che sparisce da solo.
    pub fn show(&mut self, level: Level, message: impl Into<String>) {
        let duration = match level {
            Level::Info => Duration::from_secs(5),
            Level::Warn => Duration::from_secs(8),
            Level::Error => Duration::from_secs(12),
        };
        self.message = Some(message.into());
        self.level = level;
        self.until = Some(Instant::now() + duration);
    }

    pub fn info(&mut self, message: impl Into<String>) {
        self.show(Level::Info, message);
    }

    pub fn warn(&mut self, message: impl Into<String>) {
        self.show(Level::Warn, message);
    }

    pub fn error(&mut self, message: impl Into<String>) {
        self.show(Level::Error, message);
    }

    /// Il messaggio ancora valido.
    pub fn current(&self, now: Instant) -> Option<(Level, &str)> {
        let until = self.until?;
        if now >= until {
            return None;
        }
        self.message.as_deref().map(|m| (self.level, m))
    }

    /// `true` se c'è un messaggio da far scadere (per chiedere un repaint).
    pub fn is_alive(&self, now: Instant) -> bool {
        self.until.is_some_and(|until| now < until)
    }
}

// ---------------------------------------------------------------------------
// Lavori in background
// ---------------------------------------------------------------------------

/// Esito di una ricerca nel progetto.
#[derive(Debug)]
pub struct SearchMessage {
    pub generation: u64,
    pub outcome: Result<SearchResult, String>,
}

/// Esito di una formattazione.
#[derive(Debug)]
pub struct FormatMessage {
    pub doc: DocumentId,
    pub outcome: Result<String, String>,
}

/// Esito dell'elenco file per l'apertura rapida.
#[derive(Debug)]
pub struct FilesMessage {
    pub generation: u64,
    pub files: Vec<PathBuf>,
}

/// Un evento della console di build (Maven/Gradle).
#[derive(Debug)]
pub enum BuildEvent {
    /// Una riga di output (stdout o stderr, mescolati).
    Line(String),
    /// Il comando è terminato.
    Finished {
        success: bool,
        cancelled: bool,
    },
}

/// Stato della console di build.
///
/// L'output arriva dal thread che esegue il comando e viene accumulato qui: la
/// finestra si limita a mostrarlo.
#[derive(Debug)]
pub struct BuildState {
    /// Un comando è in corso.
    pub running: bool,
    /// Etichetta del comando (es. `mvnw -B compile`).
    pub label: String,
    /// Righe di output, dalla più vecchia alla più recente.
    pub lines: Vec<String>,
    /// La finestra della console è aperta.
    pub open: bool,
    /// Esito dell'ultimo comando concluso (`None` finché è in corso).
    pub last: Option<bool>,
    /// Richiesta di interruzione per il comando in corso.
    pub cancel: Arc<AtomicBool>,
}

impl Default for BuildState {
    fn default() -> Self {
        BuildState {
            running: false,
            label: String::new(),
            lines: Vec::new(),
            open: false,
            last: None,
            cancel: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl BuildState {
    /// Quante righe tenere in memoria (l'output di una build può essere enorme).
    const MAX_LINES: usize = 4000;

    /// Prepara la console per un nuovo comando.
    pub fn start(&mut self, label: String) {
        self.lines.clear();
        self.label = label;
        self.running = true;
        self.last = None;
        self.open = true;
        self.cancel.store(false, Ordering::Relaxed);
    }

    /// Aggiunge una riga, scartando le più vecchie oltre il limite.
    pub fn push(&mut self, line: impl Into<String>) {
        self.lines.push(line.into());
        if self.lines.len() > Self::MAX_LINES {
            let excess = self.lines.len() - Self::MAX_LINES;
            self.lines.drain(..excess);
        }
    }

    /// Chiede al comando in corso di terminare.
    pub fn request_stop(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// Canali verso i thread di lavoro.
///
/// Regola: le operazioni lente (scandire il progetto, formattare) **non** devono
/// mai girare nel thread dell'interfaccia, altrimenti l'editor si blocca.
#[derive(Debug)]
pub struct Jobs {
    search_tx: Sender<SearchMessage>,
    search_rx: Receiver<SearchMessage>,
    format_tx: Sender<FormatMessage>,
    format_rx: Receiver<FormatMessage>,
    files_tx: Sender<FilesMessage>,
    files_rx: Receiver<FilesMessage>,
    build_tx: Sender<BuildEvent>,
    build_rx: Receiver<BuildEvent>,
}

impl Default for Jobs {
    fn default() -> Self {
        Self::new()
    }
}

impl Jobs {
    pub fn new() -> Self {
        let (search_tx, search_rx) = mpsc::channel();
        let (format_tx, format_rx) = mpsc::channel();
        let (files_tx, files_rx) = mpsc::channel();
        let (build_tx, build_rx) = mpsc::channel();
        Jobs {
            search_tx,
            search_rx,
            format_tx,
            format_rx,
            files_tx,
            files_rx,
            build_tx,
            build_rx,
        }
    }

    /// Lancia la ricerca nel progetto su un thread separato.
    pub fn start_search(
        &self,
        generation: u64,
        root: PathBuf,
        options: TreeOptions,
        query: SearchQuery,
    ) {
        let tx = self.search_tx.clone();
        std::thread::Builder::new()
            .name("ride-search".into())
            .spawn(move || {
                let outcome = search::search_project(&root, &options, &query)
                    .map_err(|e| e.to_string());
                let _ = tx.send(SearchMessage { generation, outcome });
            })
            .ok();
    }

    /// Lancia la formattazione del testo su un thread separato.
    pub fn start_format(&self, doc: DocumentId, language: Language, text: String) {
        let tx = self.format_tx.clone();
        std::thread::Builder::new()
            .name("ride-format".into())
            .spawn(move || {
                let outcome =
                    ride_core::format::format_text(language, &text).map_err(|e| e.to_string());
                let _ = tx.send(FormatMessage { doc, outcome });
            })
            .ok();
    }

    /// Lancia l'elenco dei file del progetto (apertura rapida).
    pub fn start_list_files(&self, generation: u64, root: PathBuf, options: TreeOptions) {
        let tx = self.files_tx.clone();
        std::thread::Builder::new()
            .name("ride-files".into())
            .spawn(move || {
                let files = match ride_core::Project::open(&root) {
                    Ok(project) => project.list_files(&options),
                    Err(_) => Vec::new(),
                };
                let _ = tx.send(FilesMessage { generation, files });
            })
            .ok();
    }

    /// Esegue un comando di build nella radice del progetto, su un thread
    /// separato.
    ///
    /// L'output di stdout e stderr viene letto riga per riga e rispedito alla
    /// GUI; il processo si può interrompere alzando il flag `cancel`.
    pub fn start_build(
        &self,
        root: PathBuf,
        program: PathBuf,
        args: Vec<String>,
        cancel: Arc<AtomicBool>,
    ) {
        let tx = self.build_tx.clone();
        std::thread::Builder::new()
            .name("ride-build".into())
            .spawn(move || run_build_command(&tx, &root, &program, &args, &cancel))
            .ok();
    }

    /// Ritira i risultati pronti (non bloccante).
    pub fn try_search(&self) -> Option<SearchMessage> {
        self.search_rx.try_recv().ok()
    }

    pub fn try_format(&self) -> Option<FormatMessage> {
        self.format_rx.try_recv().ok()
    }

    pub fn try_files(&self) -> Option<FilesMessage> {
        self.files_rx.try_recv().ok()
    }

    pub fn try_build(&self) -> Option<BuildEvent> {
        self.build_rx.try_recv().ok()
    }
}

/// Corpo del thread di build: avvia il processo, ne riversa l'output nel canale
/// e attende la fine (interrompibile).
fn run_build_command(
    tx: &Sender<BuildEvent>,
    root: &Path,
    program: &Path,
    args: &[String],
    cancel: &AtomicBool,
) {
    let spawned = Command::new(program)
        .args(args)
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let mut child = match spawned {
        Ok(child) => child,
        Err(e) => {
            let _ = tx.send(BuildEvent::Line(format!(
                "✖ impossibile avviare «{}»: {e}",
                program.display()
            )));
            let _ = tx.send(BuildEvent::Finished { success: false, cancelled: false });
            return;
        }
    };

    // stdout e stderr vanno letti da thread distinti: un solo lettore si
    // bloccherebbe sul primo flusso, senza mai accorgersi dell'interruzione.
    let stdout = child
        .stdout
        .take()
        .and_then(|out| pump("ride-build-out", tx.clone(), out));
    let stderr = child
        .stderr
        .take()
        .and_then(|err| pump("ride-build-err", tx.clone(), err));

    // Attesa interrompibile: `wait()` sarebbe bloccante e non reagirebbe al
    // pulsante "Interrompi".
    let mut cancelled = false;
    let status = loop {
        if cancel.load(Ordering::Relaxed) && !cancelled {
            cancelled = true;
            let _ = child.kill();
        }
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(_) => break None,
        }
    };

    // Il verdetto lo diamo subito, prima di aspettare i lettori: se il
    // comando ha avviato un processo nipote che tiene aperti gli stessi
    // descrittori, i lettori possono restare bloccati a lungo (o per sempre) e
    // la console non deve sembrare ancora occupata.
    let success = status.is_some_and(|s| s.success()) && !cancelled;
    let _ = tx.send(BuildEvent::Finished { success, cancelled });

    if let Some(handle) = stdout {
        let _ = handle.join();
    }
    if let Some(handle) = stderr {
        let _ = handle.join();
    }
}

/// Thread che copia un flusso di output nel canale, riga per riga.
fn pump(
    name: &str,
    tx: Sender<BuildEvent>,
    stream: impl std::io::Read + Send + 'static,
) -> Option<std::thread::JoinHandle<()>> {
    std::thread::Builder::new()
        .name(name.to_string())
        .spawn(move || {
            for line in BufReader::new(stream).lines().map_while(Result::ok) {
                let _ = tx.send(BuildEvent::Line(line));
            }
        })
        .ok()
}

// ---------------------------------------------------------------------------
// Istantanea dell'editor
// ---------------------------------------------------------------------------

/// Posizione e selezione dell'editor, aggiornate a ogni frame.
///
/// Serve alle azioni che devono sapere "dove sta lavorando l'utente" senza
/// dipendere dall'ordine con cui egui disegna i pannelli.
#[derive(Debug, Clone, Copy)]
pub struct EditorSnapshot {
    pub doc: DocumentId,
    /// Intervallo di caratteri selezionato (inizio ≤ fine).
    pub selection: (usize, usize),
    pub line: usize,
    pub column: usize,
}

impl EditorSnapshot {
    /// Intervallo di righe (0-based, inclusivo) coperto dalla selezione.
    pub fn line_range(&self, text: &str) -> (usize, usize) {
        let lines = ride_core::editor::Lines::new(text);
        let (from, to) = self.selection;
        let (first, _) = lines.line_col_of_char(text, from);
        let (last, _) = lines.line_col_of_char(text, to);
        (first, last)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzzy_premia_l_inizio_e_i_caratteri_consecutivi() {
        let a = fuzzy_score("main", "src/main.rs").unwrap();
        let b = fuzzy_score("main", "src/domain.rs").unwrap();
        assert!(a > b);
        // Non è una sottosequenza: nessun risultato.
        assert!(fuzzy_score("zzz", "src/main.rs").is_none());
        // Query vuota: tutto corrisponde.
        assert_eq!(fuzzy_score("", "qualsiasi"), Some(0));
    }

    #[test]
    fn fuzzy_ignora_maiuscole_minuscole() {
        assert!(fuzzy_score("MAIN", "src/main.rs").is_some());
    }

    #[test]
    fn il_popup_dei_problemi_non_blocca_le_scorciatoie() {
        // Non è un dialogo modale: `any_open` resta falso.
        let dialogs = DialogState {
            problems: true,
            ..Default::default()
        };
        assert!(!dialogs.any_open());
        // Mentre una finestra vera lo è.
        let dialogs = DialogState {
            about: true,
            ..dialogs
        };
        assert!(dialogs.any_open());
    }

    #[test]
    fn la_ricerca_in_file_si_ricalcola_solo_se_cambia() {
        let mut find = FindState {
            open: true,
            query: "fn".into(),
            ..Default::default()
        };
        find.recompute("fn a() {}\nfn b() {}\n");
        assert_eq!(find.matches.len(), 2);
        assert_eq!(find.current, 0);
        assert_eq!(find.step(true), Some(find.matches[1]));
        assert_eq!(find.step(true), Some(find.matches[0]));
        assert_eq!(find.step(false), Some(find.matches[1]));

        // Con una regex sbagliata si segnala l'errore e non si sblocca nulla.
        find.use_regex = true;
        find.query = "(".into();
        find.recompute("fn a() {}");
        assert!(find.error.is_some());
        assert!(find.matches.is_empty());
    }

    #[test]
    fn la_barra_chiusa_non_evidenzia_nulla() {
        let find = FindState {
            query: "x".into(),
            ..Default::default()
        };
        assert_eq!(find.stamp(), 0);
    }

    #[test]
    fn intervallo_di_righe_della_selezione() {
        let text = "a\nb\nc\n";
        let snapshot = EditorSnapshot {
            doc: DocumentId(1),
            selection: (2, 4), // dalla riga 1 al centro della riga 2
            line: 2,
            column: 0,
        };
        assert_eq!(snapshot.line_range(text), (1, 2));
    }

    #[test]
    fn la_console_scarta_le_righe_piu_vecchie() {
        let mut build = BuildState::default();
        for i in 0..BuildState::MAX_LINES + 10 {
            build.push(format!("riga {i}"));
        }
        assert_eq!(build.lines.len(), BuildState::MAX_LINES);
        // La prima riga superstite è la decima.
        assert_eq!(build.lines[0], "riga 10");
    }

    /// Attende la fine di un comando, raccogliendo l'output (con un limite di
    /// tempo per non bloccare i test).
    ///
    /// Dopo il messaggio di fine si concede una breve finestra per le ultime
    /// righe: possono essere ancora in volo mentre il processo è già terminato.
    fn attendi_build(jobs: &Jobs) -> (Vec<String>, bool, bool) {
        let mut lines = Vec::new();
        let mut finished = None;
        let mut quiet_until = None;
        let deadline = Instant::now() + Duration::from_secs(15);
        while Instant::now() < deadline {
            while let Some(event) = jobs.try_build() {
                match event {
                    BuildEvent::Line(line) => lines.push(line),
                    BuildEvent::Finished { success, cancelled } => {
                        finished = Some((success, cancelled));
                        quiet_until = Some(Instant::now() + Duration::from_millis(300));
                    }
                }
            }
            if quiet_until.is_some_and(|until| Instant::now() >= until) {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let (success, cancelled) = finished.expect("il comando non è terminato");
        (lines, success, cancelled)
    }

    #[cfg(unix)]
    #[test]
    fn un_comando_di_build_riporta_stdout_e_stderr() {
        let jobs = Jobs::new();
        jobs.start_build(
            std::env::temp_dir(),
            PathBuf::from("sh"),
            vec!["-c".into(), "echo ciao; echo errore >&2".into()],
            Arc::new(AtomicBool::new(false)),
        );
        let (lines, success, cancelled) = attendi_build(&jobs);
        assert!(success);
        assert!(!cancelled);
        assert!(lines.iter().any(|l| l == "ciao"));
        assert!(lines.iter().any(|l| l == "errore"));
    }

    #[cfg(unix)]
    #[test]
    fn un_comando_lungo_si_puo_interrompere() {
        let jobs = Jobs::new();
        let build = BuildState::default();
        jobs.start_build(
            std::env::temp_dir(),
            PathBuf::from("sh"),
            vec!["-c".into(), "sleep 30".into()],
            build.cancel.clone(),
        );
        // Lascia partire il processo, poi chiedi l'interruzione.
        std::thread::sleep(Duration::from_millis(200));
        build.request_stop();
        let (_, success, cancelled) = attendi_build(&jobs);
        assert!(cancelled);
        assert!(!success);
    }

    #[test]
    fn un_programma_mancante_chiude_la_console_con_un_errore() {
        let jobs = Jobs::new();
        jobs.start_build(
            std::env::temp_dir(),
            PathBuf::from("ride-comando-che-non-esiste"),
            Vec::new(),
            Arc::new(AtomicBool::new(false)),
        );
        let (lines, success, cancelled) = attendi_build(&jobs);
        assert!(!success);
        assert!(!cancelled);
        assert!(lines.iter().any(|l| l.contains("impossibile avviare")));
    }
}
