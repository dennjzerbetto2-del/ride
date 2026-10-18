//! Client LSP (Language Server Protocol) via JSON-RPC su stdio.
//!
//! # Come è fatto
//!
//! Il protocollo è semplice: messaggi JSON preceduti da un'intestazione
//! `Content-Length`. Il "processo" è perciò:
//!
//! 1. **Avvio** del server del linguaggio (`rust-analyzer`, `gopls`, `jdtls`,
//!    `typescript-language-server`, `pyright`) con la cartella del progetto
//!    come directory di lavoro.
//! 2. **Handshake**: `initialize` → (risposta) → `initialized`.
//! 3. **Sincronizzazione**: `textDocument/didOpen` / `didChange` / `didClose`
//!    con sincronizzazione *full* (mandiamo tutto il testo: più semplice di
//!    quella incrementale e sufficiente per un editor di questa taglia).
//! 4. **Risposte utili**: `textDocument/publishDiagnostics` (errori e warning,
//!    disegnati sotto il codice) e `textDocument/definition` (vai alla
//!    definizione).
//!
//! # Perché non blocca l'interfaccia
//!
//! Un thread dedicato legge dallo `stdout` del server e mette i messaggi in un
//! canale `mpsc`; la GUI li raccoglie una volta per frame con
//! [`LspManager::poll`]. La scrittura (poche decine di byte per volta) avviene
//! dal thread dell'interfaccia: è veloce e non richiede un secondo thread.
//!
//! # Posizioni
//!
//! LSP conta righe e colonne in **UTF-16**; `egui` (e questo editor) in
//! **caratteri**. La conversione è in [`char_offset`] e [`utf16_position`]:
//! senza di essa le sottolineature finirebbero nel posto sbagliato appena
//! compare un carattere non ASCII (un'emoji vale due unità UTF-16).

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};

use ride_core::{DocumentId, Language};
use serde_json::{json, Value};

/// Stato della connessione al server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LspStatus {
    /// Nessun server avviato.
    #[default]
    Off,
    /// Handshake in corso.
    Starting,
    /// Pronto: accetta richieste.
    Ready,
    /// Avvio o comunicazione fallita.
    Failed,
}

impl LspStatus {
    pub const fn label(self) -> &'static str {
        match self {
            LspStatus::Off => "LSP: non attivo",
            LspStatus::Starting => "LSP: avvio…",
            LspStatus::Ready => "LSP: pronto",
            LspStatus::Failed => "LSP: errore",
        }
    }

    /// `true` se il server è in grado di ricevere notifiche.
    pub const fn is_ready(self) -> bool {
        matches!(self, LspStatus::Ready)
    }
}

/// Gravità di una diagnostica, come definita dal protocollo.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    Error,
    Warning,
    Information,
    Hint,
}

impl Severity {
    /// I valori numerici del protocollo (1 = errore … 4 = suggerimento).
    fn from_lsp(value: u64) -> Severity {
        match value {
            1 => Severity::Error,
            2 => Severity::Warning,
            3 => Severity::Information,
            _ => Severity::Hint,
        }
    }

    /// Colore associato (brutto ma esplicito: la GUI lo converte in `Color32`).
    pub const fn rgb(self) -> (u8, u8, u8) {
        match self {
            Severity::Error => (220, 90, 90),
            Severity::Warning => (220, 180, 90),
            Severity::Information => (110, 170, 230),
            Severity::Hint => (150, 150, 150),
        }
    }
}

/// Un errore/warning segnalato dal server.
///
/// Gli intervalli sono in **indici di carattere** del documento, pronti per
/// essere disegnati dalla GUI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    /// Primo carattere interessato.
    pub start: usize,
    /// Primo carattere *dopo* l'intervallo.
    pub end: usize,
    /// Riga (0-based) di `start`, comoda per gli elenchi.
    pub line: usize,
    pub severity: Severity,
    pub message: String,
}

/// Impronta dei *problemi* segnalati per un documento (0 se non ce ne sono).
///
/// Serve a decidere se un insieme di diagnostiche è "nuovo" e merita di
/// riaprire il popup. Volutamente **ignora le righe**: spostare del testo (che
/// fa scivolare gli errori su e giù) non è un problema nuovo, mentre un
/// messaggio diverso sì. I messaggi vengono ordinati prima di contarli, così
/// non conta in che ordine li manda il server.
pub fn signature(diagnostics: &[Diagnostic]) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    if diagnostics.is_empty() {
        return 0;
    }
    let mut keys: Vec<(Severity, &str)> = diagnostics
        .iter()
        .map(|diagnostic| (diagnostic.severity, diagnostic.message.as_str()))
        .collect();
    keys.sort_unstable();
    let mut hasher = DefaultHasher::new();
    keys.hash(&mut hasher);
    hasher.finish()
}

/// Come avviare il language server di un linguaggio.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LanguageServerConfig {
    pub name: &'static str,
    pub program: &'static str,
    pub args: &'static [&'static str],
    /// L'identificativo `languageId` usato nelle notifiche `didOpen`.
    pub language_id: &'static str,
    /// `true` se il server sa anche **formattare** il documento
    /// (`textDocument/formatting`). Non tutti lo fanno: in quel caso la
    /// formattazione resta affidata a uno strumento esterno.
    pub formatting: bool,
}

/// Server noto per un linguaggio (scelta "di bandiera": sono quelli usati dalla
/// maggior parte degli editor).
pub fn server_for(language: Language) -> Option<LanguageServerConfig> {
    Some(match language {
        Language::Rust => LanguageServerConfig {
            name: "rust-analyzer",
            program: "rust-analyzer",
            args: &[],
            language_id: "rust",
            formatting: true,
        },
        Language::Java => LanguageServerConfig {
            name: "Eclipse JDT (jdtls)",
            program: "jdtls",
            args: &[],
            language_id: "java",
            formatting: true,
        },
        Language::Go => LanguageServerConfig {
            name: "gopls",
            program: "gopls",
            args: &[],
            language_id: "go",
            formatting: true,
        },
        Language::TypeScript => LanguageServerConfig {
            name: "typescript-language-server",
            program: "typescript-language-server",
            args: &["--stdio"],
            language_id: "typescript",
            formatting: true,
        },
        Language::JavaScript => LanguageServerConfig {
            name: "typescript-language-server",
            program: "typescript-language-server",
            args: &["--stdio"],
            language_id: "javascript",
            formatting: true,
        },
        Language::Python => LanguageServerConfig {
            name: "pyright",
            program: "pyright-langserver",
            args: &["--stdio"],
            language_id: "python",
            // `pyright` non implementa `textDocument/formatting`: per Python
            // resta il formatter esterno (`black`).
            formatting: false,
        },
        _ => return None,
    })
}

/// `true` se il language server di questo linguaggio sa formattare.
pub fn server_can_format(language: Language) -> bool {
    server_for(language).is_some_and(|config| config.formatting)
}

/// Eventi che il client consegna alla GUI a ogni frame.
#[derive(Debug, Clone, PartialEq)]
pub enum LspEvent {
    /// La diagnostica di un documento è cambiata: ridisegnare.
    Diagnostics(DocumentId),
    /// Il server vuole portarci su una posizione (vai alla definizione).
    Definition {
        path: PathBuf,
        /// Riga 0-based.
        line: usize,
        /// Colonna in unità UTF-16.
        character: usize,
    },
    /// Il server ha formattato un documento: il testo va sostituito.
    Formatted {
        doc: DocumentId,
        text: String,
    },
    /// Messaggio da mostrare (log del server o errore).
    Log(String),
    /// Lo stato della connessione è cambiato.
    Status,
}

/// Messaggi in arrivo dai thread di lettura.
enum Incoming {
    Message(Value),
    Stderr(String),
    Closed,
}

/// Documento aperto presso il server.
#[derive(Debug)]
struct OpenDoc {
    uri: String,
    version: i64,
    text: String,
}

/// A quale richiesta corrisponde un id in attesa di risposta.
#[derive(Debug)]
enum Pending {
    Initialize,
    Definition,
    /// Formattazione richiesta per un documento.
    Formatting(DocumentId),
}

/// Il client LSP. Un solo server attivo alla volta, quello del linguaggio del
/// documento in primo piano.
#[derive(Debug, Default)]
pub struct LspManager {
    status: LspStatus,
    config: Option<LanguageServerConfig>,
    language: Option<Language>,
    project_root: Option<PathBuf>,
    error: Option<String>,
    /// Diagnostica per documento, in indici di carattere.
    diagnostics: HashMap<DocumentId, Vec<Diagnostic>>,
    /// Documenti aperti presso il server.
    docs: HashMap<DocumentId, OpenDoc>,
    uri_to_doc: HashMap<String, DocumentId>,
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    incoming: Option<Receiver<Incoming>>,
    next_id: i64,
    pending: HashMap<i64, Pending>,
    /// Contesto di `egui`: i thread di lettura lo usano per svegliare la GUI
    /// appena arriva un messaggio (altrimenti resterebbe a dormire finché
    /// l'utente non tocca qualcosa).
    repaint: Option<egui::Context>,
}

impl LspManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registra il contesto `egui` da risvegliare quando arrivano messaggi.
    pub fn set_repaint_context(&mut self, ctx: egui::Context) {
        self.repaint = Some(ctx);
    }

    pub const fn status(&self) -> LspStatus {
        self.status
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Il linguaggio del server attivo.
    pub const fn language(&self) -> Option<Language> {
        self.language
    }

    /// La cartella di progetto su cui gira il server.
    pub fn project_root(&self) -> Option<&Path> {
        self.project_root.as_deref()
    }

    /// Diagnostica di un documento (vuota se il server non ha ancora risposto).
    pub fn diagnostics(&self, id: DocumentId) -> &[Diagnostic] {
        self.diagnostics.get(&id).map(|v| v.as_slice()).unwrap_or(&[])
    }

    /// Inietta una diagnostica senza passare da un server: solo per i test.
    #[cfg(test)]
    pub(crate) fn set_diagnostics(&mut self, id: DocumentId, diagnostics: Vec<Diagnostic>) {
        self.diagnostics.insert(id, diagnostics);
    }

    /// `true` se il documento è già stato comunicato al server con `didOpen`.
    pub fn is_open(&self, id: DocumentId) -> bool {
        self.docs.contains_key(&id)
    }

    /// `true` se il server del linguaggio è installato nel `PATH`.
    pub fn server_available(language: Language) -> bool {
        server_for(language).is_some_and(|c| ride_core::format::is_available(c.program))
    }

    // -----------------------------------------------------------------
    // Ciclo di vita
    // -----------------------------------------------------------------

    /// Avvia il server per il progetto corrente.
    pub fn start(&mut self, project_root: &Path, language: Language) -> Result<&'static str, String> {
        let config = server_for(language)
            .ok_or_else(|| format!("nessun language server noto per {}", language.name()))?;
        if !ride_core::format::is_available(config.program) {
            self.status = LspStatus::Failed;
            self.error = Some(format!("{} non è installato", config.program));
            return Err(self.error.clone().unwrap());
        }
        let args: Vec<String> = config.args.iter().map(|a| (*a).to_string()).collect();
        self.start_command(config, language, project_root, config.program.to_string(), args)
    }

    /// Variante con comando esplicito: serve ai test (e a chi vuole puntare a un
    /// server in un percorso diverso dal `PATH`).
    pub fn start_command(
        &mut self,
        config: LanguageServerConfig,
        language: Language,
        project_root: &Path,
        program: String,
        args: Vec<String>,
    ) -> Result<&'static str, String> {
        self.shutdown();

        let mut child = match Command::new(&program)
            .args(&args)
            .current_dir(project_root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(child) => child,
            Err(e) => {
                let message = format!("avvio di {program} fallito: {e}");
                self.status = LspStatus::Failed;
                self.error = Some(message.clone());
                return Err(message);
            }
        };

        let stdin = child.stdin.take();
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let (tx, rx) = mpsc::channel();

        if let Some(stdout) = stdout {
            let tx = tx.clone();
            let repaint = self.repaint.clone();
            std::thread::Builder::new()
                .name("ride-lsp-reader".into())
                .spawn(move || {
                    let mut reader = BufReader::new(stdout);
                    while let Ok(Some(value)) = read_message(&mut reader) {
                        if tx.send(Incoming::Message(value)).is_err() {
                            break;
                        }
                        if let Some(ctx) = &repaint {
                            ctx.request_repaint();
                        }
                    }
                    let _ = tx.send(Incoming::Closed);
                    if let Some(ctx) = &repaint {
                        ctx.request_repaint();
                    }
                })
                .ok();
        }
        if let Some(stderr) = stderr {
            let tx = tx.clone();
            std::thread::Builder::new()
                .name("ride-lsp-stderr".into())
                .spawn(move || {
                    let reader = BufReader::new(stderr);
                    for line in reader.lines().map_while(Result::ok) {
                        if tx.send(Incoming::Stderr(line)).is_err() {
                            break;
                        }
                    }
                })
                .ok();
        }

        self.child = Some(child);
        self.stdin = stdin;
        self.incoming = Some(rx);
        self.config = Some(config);
        self.language = Some(language);
        self.project_root = Some(project_root.to_path_buf());
        self.error = None;
        self.status = LspStatus::Starting;

        let root_uri = path_to_uri(project_root);
        let name = project_root
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("progetto")
            .to_string();
        let id = self.take_id();
        self.pending.insert(id, Pending::Initialize);
        let params = json!({
            "processId": std::process::id(),
            "clientInfo": { "name": "ride", "version": env!("CARGO_PKG_VERSION") },
            "rootUri": root_uri,
            "workspaceFolders": [ { "uri": root_uri, "name": name } ],
            "capabilities": {
                "textDocument": {
                    "synchronization": { "dynamicRegistration": false },
                    "publishDiagnostics": { "relatedInformation": false },
                    "definition": { "dynamicRegistration": false, "linkSupport": true },
                    "documentFormatting": { "dynamicRegistration": false },
                    "hover": { "dynamicRegistration": false, "contentFormat": ["plaintext", "markdown"] }
                },
                "workspace": { "workspaceFolders": true, "configuration": true }
            }
        });
        let init = json!({ "jsonrpc": "2.0", "id": id, "method": "initialize", "params": params });
        if self.send(&init).is_err() {
            // `send` ha già aggiornato stato ed errore.
        }
        Ok(config.name)
    }

    /// Chiude la connessione e termina il processo.
    pub fn shutdown(&mut self) {
        if let Some(mut stdin) = self.stdin.take() {
            // Saluto gentile: `shutdown` + `exit` come vuole il protocollo.
            let id = self.take_id();
            let shutdown = json!({ "jsonrpc": "2.0", "id": id, "method": "shutdown", "params": null });
            let _ = write_message(&mut stdin, &shutdown);
            let exit = json!({ "jsonrpc": "2.0", "method": "exit", "params": null });
            let _ = write_message(&mut stdin, &exit);
        }
        if let Some(mut child) = self.child.take() {
            // `kill` è la garanzia: se il server ignora `exit`, non resta vivo.
            let _ = child.kill();
            let _ = child.wait();
        }
        self.incoming = None;
        self.status = LspStatus::Off;
        self.docs.clear();
        self.uri_to_doc.clear();
        self.pending.clear();
        self.diagnostics.clear();
    }

    // -----------------------------------------------------------------
    // Sincronizzazione dei documenti
    // -----------------------------------------------------------------

    /// Segnala al server che un documento è aperto (idempotente).
    pub fn did_open(&mut self, id: DocumentId, path: &Path, _language: Language, text: &str) {
        if !self.status.is_ready() || self.docs.contains_key(&id) {
            return;
        }
        let language_id = self.config.map(|c| c.language_id).unwrap_or("plaintext");
        let uri = path_to_uri(path);
        let params = json!({
            "textDocument": {
                "uri": uri,
                "languageId": language_id,
                "version": 1,
                "text": text,
            }
        });
        self.docs.insert(
            id,
            OpenDoc {
                uri: uri.clone(),
                version: 1,
                text: text.to_string(),
            },
        );
        self.uri_to_doc.insert(uri, id);
        self.send_notification("textDocument/didOpen", params);
    }

    /// Segnala una modifica (sincronizzazione *full*).
    pub fn did_change(&mut self, id: DocumentId, text: &str) {
        let Some(doc) = self.docs.get_mut(&id) else {
            return;
        };
        if doc.text == text {
            return;
        }
        doc.text = text.to_string();
        doc.version += 1;
        let uri = doc.uri.clone();
        let version = doc.version;
        self.send_notification(
            "textDocument/didChange",
            json!({
                "textDocument": { "uri": uri, "version": version },
                "contentChanges": [ { "text": text } ]
            }),
        );
    }

    /// Segnala che un documento è stato chiuso.
    pub fn did_close(&mut self, id: DocumentId) {
        if let Some(doc) = self.docs.remove(&id) {
            self.uri_to_doc.remove(&doc.uri);
            if self.status.is_ready() {
                self.send_notification(
                    "textDocument/didClose",
                    json!({ "textDocument": { "uri": doc.uri } }),
                );
            }
        }
        self.diagnostics.remove(&id);
    }

    /// Chiede la definizione alla posizione (indice di carattere) indicata.
    pub fn request_definition(&mut self, id: DocumentId, char_position: usize) {
        if !self.status.is_ready() {
            return;
        }
        let Some(doc) = self.docs.get(&id) else {
            return;
        };
        let (line, character) = utf16_position(&doc.text, char_position);
        let uri = doc.uri.clone();
        let request_id = self.take_id();
        self.pending.insert(request_id, Pending::Definition);
        self.send(&json!({
            "jsonrpc": "2.0",
            "id": request_id,
            "method": "textDocument/definition",
            "params": {
                "textDocument": { "uri": uri },
                "position": { "line": line, "character": character }
            }
        }))
        .ok();
    }

    /// Chiede al server di formattare un documento.
    ///
    /// Restituisce `false` se la richiesta non è partita (server non pronto,
    /// documento sconosciuto o formattazione già in corso): la GUI lo usa per
    /// spiegare il perché senza attendere invano.
    pub fn request_formatting(
        &mut self,
        id: DocumentId,
        tab_size: usize,
        insert_spaces: bool,
    ) -> bool {
        if !self.status.is_ready() {
            return false;
        }
        if self
            .pending
            .values()
            .any(|pending| matches!(pending, Pending::Formatting(doc) if *doc == id))
        {
            return false;
        }
        let Some(doc) = self.docs.get(&id) else {
            return false;
        };
        let uri = doc.uri.clone();
        let request_id = self.take_id();
        self.pending.insert(request_id, Pending::Formatting(id));
        self.send(&json!({
            "jsonrpc": "2.0",
            "id": request_id,
            "method": "textDocument/formatting",
            "params": {
                "textDocument": { "uri": uri },
                "options": { "tabSize": tab_size, "insertSpaces": insert_spaces }
            }
        }))
        .is_ok()
    }

    // -----------------------------------------------------------------
    // Ciclo di raccolta dei messaggi
    // -----------------------------------------------------------------

    /// Raccoglie i messaggi pronti dal server senza bloccare. Va chiamata una
    /// volta per frame.
    pub fn poll(&mut self) -> Vec<LspEvent> {
        let mut events = Vec::new();
        let Some(receiver) = &self.incoming else {
            return events;
        };
        let mut queue = Vec::new();
        while let Ok(message) = receiver.try_recv() {
            queue.push(message);
        }
        for message in queue {
            match message {
                Incoming::Message(value) => self.handle_message(value, &mut events),
                Incoming::Stderr(line) => {
                    let line = line.trim();
                    if !line.is_empty() {
                        self.error = Some(line.to_string());
                    }
                }
                Incoming::Closed => {
                    if self.status != LspStatus::Off {
                        self.status = LspStatus::Failed;
                        if self.error.is_none() {
                            self.error = Some("il server ha chiuso la connessione".into());
                        }
                        events.push(LspEvent::Status);
                    }
                }
            }
        }
        events
    }

    fn handle_message(&mut self, value: Value, events: &mut Vec<LspEvent>) {
        // Richiesta o notifica dal server.
        if let Some(method) = value.get("method").and_then(Value::as_str) {
            if value.get("id").is_some() {
                self.answer_request(&value);
            } else {
                let method = method.to_string();
                let params = value.get("params").cloned().unwrap_or(Value::Null);
                self.handle_notification(&method, params, events);
            }
            return;
        }

        // Risposta a una nostra richiesta.
        let Some(id) = value.get("id").and_then(Value::as_i64) else {
            return;
        };
        let Some(kind) = self.pending.remove(&id) else {
            return;
        };
        match kind {
            Pending::Initialize => {
                if let Some(error) = value.get("error") {
                    self.status = LspStatus::Failed;
                    self.error = Some(format!("initialize rifiutato: {error}"));
                } else {
                    self.status = LspStatus::Ready;
                    self.error = None;
                    self.send_notification("initialized", json!({}));
                }
                events.push(LspEvent::Status);
            }
            Pending::Definition => {
                if let Some((path, line, character)) = parse_location(value.get("result")) {
                    events.push(LspEvent::Definition {
                        path,
                        line,
                        character,
                    });
                }
            }
            Pending::Formatting(doc) => {
                if let Some(error) = value.get("error") {
                    events.push(LspEvent::Log(format!("formattazione rifiutata: {error}")));
                    return;
                }
                // `null` (o un elenco vuoto) significa "niente da cambiare": lo
                // comunichiamo lo stesso, così la GUI può dirlo all'utente.
                let Some(text) = self.docs.get(&doc).map(|d| d.text.clone()) else {
                    return;
                };
                let edits = parse_text_edits(value.get("result"), &text);
                let formatted = if edits.is_empty() {
                    text
                } else {
                    apply_text_edits(&text, &edits)
                };
                events.push(LspEvent::Formatted {
                    doc,
                    text: formatted,
                });
            }
        }
    }

    fn handle_notification(&mut self, method: &str, params: Value, events: &mut Vec<LspEvent>) {
        match method {
            "textDocument/publishDiagnostics" => {
                let Some(uri) = params.get("uri").and_then(Value::as_str) else {
                    return;
                };
                let Some(id) = self.uri_to_doc.get(uri).copied() else {
                    return;
                };
                let text = self.docs.get(&id).map(|d| d.text.as_str()).unwrap_or("");
                let list = diagnostics_from(&params, text);
                self.diagnostics.insert(id, list);
                events.push(LspEvent::Diagnostics(id));
            }
            "window/logMessage" | "window/showMessage" => {
                if let Some(message) = params.get("message").and_then(Value::as_str) {
                    events.push(LspEvent::Log(message.to_string()));
                }
            }
            _ => {}
        }
    }

    /// Risponde a una richiesta del server: senza risposta il server può
    /// bloccarsi (es. `workspace/configuration`).
    fn answer_request(&mut self, value: &Value) {
        let Some(id) = value.get("id").cloned() else {
            return;
        };
        let method = value.get("method").and_then(Value::as_str).unwrap_or("");
        let result = match method {
            // Il server chiede la configurazione di N sezioni: rispondiamo con N
            // valori nulli (nessuna configurazione personalizzata).
            "workspace/configuration" => {
                let count = value
                    .pointer("/params/items")
                    .and_then(Value::as_array)
                    .map(|items| items.len())
                    .unwrap_or(0);
                Value::Array(vec![Value::Null; count])
            }
            _ => Value::Null,
        };
        self.send(&json!({ "jsonrpc": "2.0", "id": id, "result": result }))
            .ok();
    }

    // -----------------------------------------------------------------
    // Invio
    // -----------------------------------------------------------------

    fn send_notification(&mut self, method: &str, params: Value) {
        self.send(&json!({ "jsonrpc": "2.0", "method": method, "params": params }))
            .ok();
    }

    fn send(&mut self, value: &Value) -> Result<(), String> {
        let result = match self.stdin.as_mut() {
            Some(stdin) => write_message(stdin, value),
            None => return Err("nessun server avviato".into()),
        };
        match result {
            Ok(()) => Ok(()),
            Err(e) => {
                let message = e.to_string();
                self.status = LspStatus::Failed;
                self.error = Some(message.clone());
                Err(message)
            }
        }
    }

    fn take_id(&mut self) -> i64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }
}

// ---------------------------------------------------------------------------
// Protocollo: framing e conversione
// ---------------------------------------------------------------------------

/// Scrive un messaggio JSON-RPC con l'intestazione `Content-Length`.
fn write_message(writer: &mut impl Write, value: &Value) -> std::io::Result<()> {
    let body = serde_json::to_vec(value)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    write!(writer, "Content-Length: {}\r\n\r\n", body.len())?;
    writer.write_all(&body)?;
    writer.flush()
}

/// Legge un messaggio JSON-RPC; `Ok(None)` a stream chiuso.
fn read_message(reader: &mut impl BufRead) -> std::io::Result<Option<Value>> {
    let mut length: Option<usize> = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            return Ok(None);
        }
        let header = line.trim_end_matches(['\r', '\n']);
        if header.is_empty() {
            break;
        }
        if let Some(rest) = header.strip_prefix("Content-Length:") {
            length = rest.trim().parse().ok();
        }
    }
    let Some(length) = length else {
        return Ok(None);
    };
    let mut body = vec![0u8; length];
    reader.read_exact(&mut body)?;
    let value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    Ok(Some(value))
}

/// Costruisce la diagnostica di un documento a partire dai parametri
/// `publishDiagnostics`, convertendo le posizioni UTF-16 in indici di carattere.
fn diagnostics_from(params: &Value, text: &str) -> Vec<Diagnostic> {
    let Some(items) = params.get("diagnostics").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut list = Vec::new();
    for item in items {
        let Some(range) = item.get("range") else {
            continue;
        };
        let line = range.pointer("/start/line").and_then(Value::as_u64).unwrap_or(0) as usize;
        let start_col = range
            .pointer("/start/character")
            .and_then(Value::as_u64)
            .unwrap_or(0) as usize;
        let end_line = range.pointer("/end/line").and_then(Value::as_u64).unwrap_or(line as u64) as usize;
        let end_col = range
            .pointer("/end/character")
            .and_then(Value::as_u64)
            .unwrap_or(start_col as u64) as usize;
        let severity = item.get("severity").and_then(Value::as_u64).unwrap_or(1);
        let message = item
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let start = char_offset(text, line, start_col);
        let end = char_offset(text, end_line, end_col).max(start);
        list.push(Diagnostic {
            start,
            end,
            line,
            severity: Severity::from_lsp(severity),
            message,
        });
    }
    list
}

/// Estrae la prima posizione da un `textDocument/definition` (accetta
/// `Location`, `Location[]` e `LocationLink[]`).
fn parse_location(result: Option<&Value>) -> Option<(PathBuf, usize, usize)> {
    let result = result?;
    let location = if result.is_array() {
        result.as_array()?.first()?
    } else {
        result
    };
    let uri = location
        .get("uri")
        .and_then(Value::as_str)
        .or_else(|| location.pointer("/targetUri").and_then(Value::as_str))?;
    let range = location
        .get("range")
        .or_else(|| location.get("targetSelectionRange"))?;
    let line = range.pointer("/start/line").and_then(Value::as_u64)? as usize;
    let character = range.pointer("/start/character").and_then(Value::as_u64)? as usize;
    Some((uri_to_path(uri)?, line, character))
}

/// Un `TextEdit` del protocollo, con gli offset **già in caratteri** del
/// documento (pronti per il modello).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextEdit {
    /// Primo carattere sostituito.
    pub start: usize,
    /// Primo carattere *dopo* la parte sostituita.
    pub end: usize,
    /// Testo nuovo.
    pub new_text: String,
}

/// Legge l'elenco di `TextEdit` restituito da `textDocument/formatting`.
///
/// `null` (o un valore inatteso) significa "nessuna modifica".
fn parse_text_edits(result: Option<&Value>, text: &str) -> Vec<TextEdit> {
    let Some(items) = result.and_then(Value::as_array) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            let range = item.get("range")?;
            let start_line = range.pointer("/start/line").and_then(Value::as_u64)? as usize;
            let start_col = range
                .pointer("/start/character")
                .and_then(Value::as_u64)
                .unwrap_or(0) as usize;
            let end_line = range.pointer("/end/line").and_then(Value::as_u64)? as usize;
            let end_col = range
                .pointer("/end/character")
                .and_then(Value::as_u64)
                .unwrap_or(0) as usize;
            let start = char_offset(text, start_line, start_col);
            let end = char_offset(text, end_line, end_col);
            Some(TextEdit {
                start,
                end: end.max(start),
                new_text: item
                    .get("newText")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
            })
        })
        .collect()
}

/// Indice di byte del carattere numero `char_index` (la lunghezza se è oltre).
fn char_to_byte(text: &str, char_index: usize) -> usize {
    text.char_indices()
        .nth(char_index)
        .map_or(text.len(), |(byte, _)| byte)
}

/// Applica una lista di `TextEdit` al testo e restituisce il risultato.
///
/// Gli intervalli del protocollo non si sovrappongono; si applicano dall'ultimo
/// al primo, così le posizioni precedenti restano valide senza doverle
/// ricalcolare a ogni modifica. A parità di posizione vale l'ordine
/// dell'elenco: applicando al contrario, il risultato è nell'ordine ricevuto
/// (è il caso di più inserimenti nello stesso punto).
pub fn apply_text_edits(text: &str, edits: &[TextEdit]) -> String {
    // (inizio, fine, ordine) in byte; l'ordine entra nella chiave per rendere
    // deterministico il caso di posizioni identiche.
    let mut spans: Vec<((usize, usize, usize), &str)> = edits
        .iter()
        .enumerate()
        .map(|(index, edit)| {
            (
                (
                    char_to_byte(text, edit.start),
                    char_to_byte(text, edit.end),
                    index,
                ),
                edit.new_text.as_str(),
            )
        })
        .collect();
    spans.sort_by_key(|(key, _)| std::cmp::Reverse(*key));

    let mut result = text.to_string();
    for ((start, end, _), new_text) in spans {
        if start > end || end > result.len() {
            continue;
        }
        if result.is_char_boundary(start) && result.is_char_boundary(end) {
            result.replace_range(start..end, new_text);
        }
    }
    result
}

/// Indice di carattere corrispondente a (riga, colonna UTF-16).
///
/// Se la posizione cade oltre la fine della riga, restituisce la fine della
/// riga: meglio una posizione leggermente spostata che un indice fuori dal
/// testo.
pub fn char_offset(text: &str, line: usize, utf16_col: usize) -> usize {
    let mut offset = 0usize;
    for (index, raw) in text.split_inclusive('\n').enumerate() {
        let body = raw.strip_suffix('\n').unwrap_or(raw);
        if index == line {
            let mut column = 0usize;
            for (chars, ch) in body.chars().enumerate() {
                if column >= utf16_col {
                    return offset + chars;
                }
                column += ch.len_utf16();
            }
            return offset + body.chars().count();
        }
        offset += raw.chars().count();
    }
    offset
}

/// Inverso di [`char_offset`]: da indice di carattere a (riga, colonna UTF-16).
pub fn utf16_position(text: &str, char_position: usize) -> (usize, usize) {
    let mut line = 0usize;
    let mut column = 0usize;
    for (index, ch) in text.chars().enumerate() {
        if index == char_position {
            return (line, column);
        }
        if ch == '\n' {
            line += 1;
            column = 0;
        } else {
            column += ch.len_utf16();
        }
    }
    (line, column)
}

/// Converte un percorso in URI `file://` (con la codifica percentuale delle
/// parti che non sono ASCII sicuro).
pub fn path_to_uri(path: &Path) -> String {
    let absolute = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let mut uri = String::from("file://");
    for component in absolute.to_string_lossy().split('/') {
        if component.is_empty() {
            continue;
        }
        uri.push('/');
        for byte in component.bytes() {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~' | b'+' | b'@') {
                uri.push(byte as char);
            } else {
                uri.push_str(&format!("%{byte:02X}"));
            }
        }
    }
    uri
}

/// Inverso di [`path_to_uri`].
pub fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let mut bytes = Vec::with_capacity(rest.len());
    let mut chars = rest.bytes();
    while let Some(byte) = chars.next() {
        if byte == b'%' {
            let hex = [chars.next()?, chars.next()?];
            let hex = std::str::from_utf8(&hex).ok()?;
            bytes.push(u8::from_str_radix(hex, 16).ok()?);
        } else {
            bytes.push(byte);
        }
    }
    let text = String::from_utf8(bytes).ok()?;
    Some(PathBuf::from(text))
}

/// Un server LSP finto: risponde a `initialize`, manda una diagnostica quando si
/// apre un documento, risponde a `definition` e a `formatting`.
///
/// Sta fuori dal modulo dei test perché lo usano anche i test dell'applicazione.
#[cfg(test)]
pub(crate) const FAKE_SERVER: &str = r#"
import sys, json

def read_exact(n):
    data = b""
    while len(data) < n:
        chunk = sys.stdin.buffer.read(n - len(data))
        if not chunk:
            raise EOFError
        data += chunk
    return data

def read_message():
    length = None
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            return None
        line = line.strip()
        if line == b"":
            break
        if line.lower().startswith(b"content-length:"):
            length = int(line.split(b":")[1])
    return json.loads(read_exact(length))

def send(obj):
    data = json.dumps(obj).encode()
    sys.stdout.buffer.write(b"Content-Length: %d\r\n\r\n" % len(data))
    sys.stdout.buffer.write(data)
    sys.stdout.buffer.flush()

while True:
    msg = read_message()
    if msg is None:
        break
    method = msg.get("method")
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": msg["id"], "result": {"capabilities": {}}})
    elif method == "initialized":
        pass
    elif method == "textDocument/didOpen":
        uri = msg["params"]["textDocument"]["uri"]
        send({"jsonrpc": "2.0", "method": "textDocument/publishDiagnostics", "params": {
            "uri": uri,
            "diagnostics": [{"range": {"start": {"line": 0, "character": 0},
                                       "end": {"line": 0, "character": 3}},
                             "severity": 1, "message": "finto errore"}]}})
    elif method == "textDocument/definition":
        params = msg["params"]
        send({"jsonrpc": "2.0", "id": msg["id"], "result": [
            {"uri": params["textDocument"]["uri"],
             "range": {"start": {"line": 2, "character": 0},
                       "end": {"line": 2, "character": 1}}}]})
    elif method == "textDocument/formatting":
        # Sostituisce le prime due lettere: "fn" -> "FN".
        send({"jsonrpc": "2.0", "id": msg["id"], "result": [
            {"range": {"start": {"line": 0, "character": 0},
                       "end": {"line": 0, "character": 2}},
             "newText": "FN"}]})
    elif method == "shutdown":
        send({"jsonrpc": "2.0", "id": msg["id"], "result": None})
    elif method == "exit":
        break
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ogni_linguaggio_richiesto_ha_un_server() {
        for language in [
            Language::Rust,
            Language::Java,
            Language::Go,
            Language::TypeScript,
            Language::Python,
        ] {
            let config = server_for(language).unwrap_or_else(|| panic!("{language:?} senza server"));
            assert!(!config.program.is_empty());
            assert!(!config.language_id.is_empty());
        }
    }

    #[test]
    fn i_server_hanno_i_comandi_attesi() {
        assert_eq!(server_for(Language::Rust).unwrap().program, "rust-analyzer");
        assert_eq!(server_for(Language::Java).unwrap().program, "jdtls");
        assert_eq!(server_for(Language::Go).unwrap().program, "gopls");
        assert_eq!(
            server_for(Language::TypeScript).unwrap().program,
            "typescript-language-server"
        );
        assert_eq!(
            server_for(Language::Python).unwrap().program,
            "pyright-langserver"
        );
        assert!(server_for(Language::Markdown).is_none());
    }

    #[test]
    fn la_firma_dei_problemi_ignora_ordine_e_righe() {
        let make = |severity, message: &str, line| Diagnostic {
            start: 0,
            end: 1,
            line,
            severity,
            message: message.to_string(),
        };

        // Nessun problema: firma nulla.
        assert_eq!(signature(&[]), 0);

        let a = make(Severity::Error, "boom", 3);
        let b = make(Severity::Warning, "attenzione", 7);
        // L'ordine con cui arrivano non conta...
        assert_eq!(
            signature(&[a.clone(), b.clone()]),
            signature(&[b.clone(), a.clone()])
        );
        // ...e nemmeno la riga (spostare il testo non è un problema nuovo)...
        assert_eq!(
            signature(std::slice::from_ref(&a)),
            signature(std::slice::from_ref(&make(Severity::Error, "boom", 99)))
        );
        // ...ma un messaggio diverso sì.
        assert_ne!(
            signature(std::slice::from_ref(&a)),
            signature(std::slice::from_ref(&make(Severity::Error, "bang", 3)))
        );
        // E nemmeno due problemi sono uno solo.
        assert_ne!(signature(std::slice::from_ref(&a)), signature(&[a, b]));
    }

    /// Ogni server dichiara se sa formattare: `pyright` no, gli altri sì.
    #[test]
    fn i_server_dichiarano_se_sanno_formattare() {
        assert!(server_can_format(Language::Rust));
        assert!(server_can_format(Language::Java));
        assert!(server_can_format(Language::Go));
        assert!(server_can_format(Language::TypeScript));
        assert!(server_can_format(Language::JavaScript));
        // `pyright` non implementa `textDocument/formatting`.
        assert!(!server_can_format(Language::Python));
        // Nessun server noto: niente formattazione via LSP.
        assert!(!server_can_format(Language::Markdown));
    }

    #[test]
    fn gli_edit_si_applicano_dall_ultimo_al_primo() {
        let text = "uno due tre\n";
        let edits = vec![
            TextEdit { start: 0, end: 3, new_text: "1".into() },
            TextEdit { start: 8, end: 11, new_text: "3".into() },
            TextEdit { start: 4, end: 7, new_text: "2".into() },
        ];
        assert_eq!(apply_text_edits(text, &edits), "1 2 3\n");
    }

    #[test]
    fn un_edit_che_inserisce_non_mangia_il_resto() {
        let text = "ac\n";
        let edits = vec![TextEdit { start: 1, end: 1, new_text: "b".into() }];
        assert_eq!(apply_text_edits(text, &edits), "abc\n");
    }

    /// Il caso reale di `jdtls`: tanti inserimenti puntuali sulla stessa riga.
    #[test]
    fn piu_inserimenti_sulla_stessa_riga() {
        let text = "class A{int x;}";
        let edits = vec![
            TextEdit { start: 8, end: 8, new_text: "\n    ".into() },
            TextEdit { start: 14, end: 14, new_text: "\n".into() },
        ];
        assert_eq!(apply_text_edits(text, &edits), "class A{\n    int x;\n}");
    }

    /// A parità di posizione l'ordine dell'elenco è l'ordine del risultato.
    #[test]
    fn due_inserimenti_nello_stesso_punto_rispettano_l_ordine() {
        let edits = vec![
            TextEdit { start: 1, end: 1, new_text: "X".into() },
            TextEdit { start: 1, end: 1, new_text: "Y".into() },
        ];
        assert_eq!(apply_text_edits("ab", &edits), "aXYb");
    }

    #[test]
    fn senza_edit_il_testo_non_cambia() {
        assert_eq!(apply_text_edits("ciao\n", &[]), "ciao\n");
    }

    #[test]
    fn le_posizioni_degli_edit_sono_in_utf16() {
        // L'emoji occupa 2 unità UTF-16: la colonna 3 è l'indice di carattere 2.
        let text = "a😀b\n";
        let result = json!([{
            "range": { "start": { "line": 0, "character": 3 },
                       "end":   { "line": 0, "character": 4 } },
            "newText": "B"
        }]);
        let edits = parse_text_edits(Some(&result), text);
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].start, 2);
        assert_eq!(edits[0].end, 3);
        assert_eq!(apply_text_edits(text, &edits), "a😀B\n");
    }

    #[test]
    fn un_risultato_nullo_non_produce_edit() {
        assert!(parse_text_edits(None, "x").is_empty());
        assert!(parse_text_edits(Some(&Value::Null), "x").is_empty());
        assert!(parse_text_edits(Some(&json!({})), "x").is_empty());
    }

    #[test]
    fn la_formattazione_vuole_un_server_pronto() {
        let mut lsp = LspManager::new();
        assert!(!lsp.request_formatting(DocumentId(0), 4, true));
    }

    #[test]
    fn il_framing_si_rilegge() {
        let mut buffer = Vec::new();
        write_message(&mut buffer, &json!({ "jsonrpc": "2.0", "id": 1 })).unwrap();
        let mut reader = std::io::BufReader::new(&buffer[..]);
        let value = read_message(&mut reader).unwrap().unwrap();
        assert_eq!(value["id"], 1);

        // Un secondo messaggio nello stesso stream non si perde.
        let mut two = Vec::new();
        write_message(&mut two, &json!({ "id": 1 })).unwrap();
        write_message(&mut two, &json!({ "id": 2 })).unwrap();
        let mut reader = std::io::BufReader::new(&two[..]);
        assert_eq!(read_message(&mut reader).unwrap().unwrap()["id"], 1);
        assert_eq!(read_message(&mut reader).unwrap().unwrap()["id"], 2);
        assert!(read_message(&mut reader).unwrap().is_none());
    }

    #[test]
    fn le_posizioni_utf16_tornano_da_caratteri() {
        let text = "a\n😀 b\nc";
        // La riga 1 contiene l'emoji (2 unità UTF-16) e uno spazio.
        assert_eq!(char_offset(text, 0, 0), 0);
        assert_eq!(char_offset(text, 0, 1), 1); // fine della riga 0 (dopo 'a')
        assert_eq!(char_offset(text, 1, 0), 2); // inizio riga 1
        assert_eq!(char_offset(text, 1, 2), 3); // dopo l'emoji
        assert_eq!(char_offset(text, 2, 0), 6); // riga 2
        // Oltre la fine della riga: si torna alla fine della riga.
        assert_eq!(char_offset(text, 0, 99), 1);

        assert_eq!(utf16_position(text, 0), (0, 0));
        assert_eq!(utf16_position(text, 3), (1, 2));
        assert_eq!(utf16_position(text, 6), (2, 0));
    }

    #[test]
    fn gli_uri_file_si_convertono() {
        let uri = path_to_uri(Path::new("/tmp/una cartella/a.rs"));
        assert!(uri.starts_with("file:///tmp/"));
        assert!(uri.contains("%20"));
        assert_eq!(uri_to_path(&uri).unwrap(), PathBuf::from("/tmp/una cartella/a.rs"));
    }

    #[test]
    fn la_diagnostica_si_converte_in_caratteri() {
        let text = "let x = 1;\nlet y = 2;\n";
        let params = json!({
            "uri": "file:///tmp/a.rs",
            "diagnostics": [
                { "range": { "start": { "line": 1, "character": 4 }, "end": { "line": 1, "character": 5 } },
                  "severity": 1, "message": "errore" }
            ]
        });
        let list = diagnostics_from(&params, text);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].severity, Severity::Error);
        // Riga 1, colonna 4 → indice di carattere 11 + 4 = 15.
        assert_eq!(list[0].start, 15);
        assert_eq!(list[0].end, 16);
        assert_eq!(list[0].line, 1);
    }

    #[test]
    #[cfg(unix)]
    fn dialogo_con_un_server_finto() {
        use std::time::{Duration, Instant};

        if !ride_core::format::is_available("python3") {
            return; // niente Python: il test si salta.
        }
        let dir = std::env::temp_dir().join(format!("ride-lsp-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("fake_lsp.py");
        std::fs::write(&script, FAKE_SERVER).unwrap();
        let file = dir.join("main.rs");
        std::fs::write(&file, "fn main() {}\n").unwrap();

        let config = LanguageServerConfig {
            name: "finto",
            program: "python3",
            args: &[],
            language_id: "rust",
            formatting: true,
        };
        let mut lsp = LspManager::new();
        lsp.start_command(
            config,
            Language::Rust,
            &dir,
            "python3".into(),
            vec![script.display().to_string()],
        )
        .unwrap();

        // Attende l'handshake.
        let deadline = Instant::now() + Duration::from_secs(10);
        while lsp.status() != LspStatus::Ready && Instant::now() < deadline {
            lsp.poll();
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(lsp.status(), LspStatus::Ready, "errore: {:?}", lsp.error());

        // didOpen → il server manda una diagnostica.
        lsp.did_open(DocumentId(0), &file, Language::Rust, "fn main() {}\n");
        let mut got_diagnostics = false;
        let deadline = Instant::now() + Duration::from_secs(10);
        while !got_diagnostics && Instant::now() < deadline {
            for event in lsp.poll() {
                if matches!(event, LspEvent::Diagnostics(_)) {
                    got_diagnostics = true;
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(got_diagnostics, "nessuna diagnostica ricevuta");
        let diagnostics = lsp.diagnostics(DocumentId(0));
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].severity, Severity::Error);
        assert_eq!(diagnostics[0].message, "finto errore");
        assert_eq!(diagnostics[0].start, 0);
        // definition → il server risponde con una posizione.
        lsp.request_definition(DocumentId(0), 0);
        let mut definition = None;
        let deadline = Instant::now() + Duration::from_secs(10);
        while definition.is_none() && Instant::now() < deadline {
            for event in lsp.poll() {
                if let LspEvent::Definition { path, line, .. } = event {
                    definition = Some((path, line));
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let (path, line) = definition.expect("nessuna definizione ricevuta");
        assert!(path.ends_with("main.rs"));
        assert_eq!(line, 2);

        // formatting → il server risponde con una sostituzione ("fn " → "FN ").
        assert!(lsp.request_formatting(DocumentId(0), 4, true));
        // Una seconda richiesta per lo stesso documento viene rifiutata: è
        // ancora in corso.
        assert!(!lsp.request_formatting(DocumentId(0), 4, true));
        let mut formatted = None;
        let deadline = Instant::now() + Duration::from_secs(10);
        while formatted.is_none() && Instant::now() < deadline {
            for event in lsp.poll() {
                if let LspEvent::Formatted { doc, text } = event {
                    assert_eq!(doc, DocumentId(0));
                    formatted = Some(text);
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(formatted.as_deref(), Some("FN main() {}\n"));

        // A richiesta conclusa se ne può fare un'altra.
        assert!(lsp.request_formatting(DocumentId(0), 4, true));

        lsp.shutdown();
        assert_eq!(lsp.status(), LspStatus::Off);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Prova il client contro un server vero. È `#[ignore]` perché dipende da
    /// `rust-analyzer` installato e dall'analisi del progetto: si lancia a mano
    /// con `cargo test -p ride -- --ignored`.
    #[test]
    #[ignore = "richiede rust-analyzer installato e l'analisi del progetto"]
    fn handshake_con_rust_analyzer() {
        use std::time::{Duration, Instant};

        if !ride_core::format::is_available("rust-analyzer") {
            return;
        }
        let root = std::env::current_dir().unwrap();
        let file = root.join("src/lsp/mod.rs");
        if !file.exists() {
            return;
        }

        let mut lsp = LspManager::new();
        lsp.start(&root, Language::Rust).ok();

        let deadline = Instant::now() + Duration::from_secs(60);
        while lsp.status() != LspStatus::Ready && Instant::now() < deadline {
            lsp.poll();
            std::thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(lsp.status(), LspStatus::Ready, "errore: {:?}", lsp.error());

        lsp.did_open(DocumentId(0), &file, Language::Rust, "fn main() {}\n");
        lsp.shutdown();
    }

    /// Prova la formattazione via LSP contro un server vero (`jdtls`). È
    /// `#[ignore]` perché dipende da `jdtls` installato e dall'importazione del
    /// progetto Java: si lancia a mano con `cargo test -p ride -- --ignored`.
    #[test]
    #[ignore = "richiede jdtls installato e l'importazione del progetto Java"]
    fn la_formattazione_java_funziona_con_jdtls() {
        use std::time::{Duration, Instant};

        if !ride_core::format::is_available("jdtls") {
            return;
        }
        let dir = std::env::temp_dir().join(format!("ride-jdtls-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("Main.java");
        let ugly = "public class Main {public static void main(String[]a){System.out.println(\"ciao\");}}\n";
        std::fs::write(&file, ugly).unwrap();

        let mut lsp = LspManager::new();
        lsp.start(&dir, Language::Java).ok();

        let deadline = Instant::now() + Duration::from_secs(180);
        while lsp.status() != LspStatus::Ready && Instant::now() < deadline {
            lsp.poll();
            std::thread::sleep(Duration::from_millis(100));
        }
        assert_eq!(lsp.status(), LspStatus::Ready, "errore: {:?}", lsp.error());

        lsp.did_open(DocumentId(0), &file, Language::Java, ugly);
        assert!(lsp.request_formatting(DocumentId(0), 4, true));

        let mut formatted = None;
        let deadline = Instant::now() + Duration::from_secs(180);
        while formatted.is_none() && Instant::now() < deadline {
            for event in lsp.poll() {
                if let LspEvent::Formatted { text, .. } = event {
                    formatted = Some(text);
                }
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        lsp.shutdown();

        let formatted = formatted.expect("jdtls non ha risposto alla formattazione");
        assert_ne!(formatted, ugly, "il testo è rimasto com'era");
        assert!(formatted.contains("\n    public static void main"), "{formatted}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn senza_binario_lo_stato_e_errore() {
        let mut lsp = LspManager::new();
        lsp.config = None;
        // Un programma che non esiste non può essere avviato.
        let config = LanguageServerConfig {
            name: "finto",
            program: "ride-server-inesistente",
            args: &[],
            language_id: "text",
            formatting: false,
        };
        let result = lsp.start_command(config, Language::Text, Path::new("/tmp"), "ride-server-inesistente".into(), vec![]);
        assert!(result.is_err());
        assert_eq!(lsp.status(), LspStatus::Failed);
    }
}
