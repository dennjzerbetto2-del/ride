//! Un file aperto nell'editor.
//!
//! Un [`Document`] è proprietario del testo, ricorda da dove viene, se è stato
//! modificato e con quale stile di fine riga va riscritto su disco.

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::syntax::Language;

/// Oltre questa dimensione il file non viene aperto (i grandi binari o i log
/// giganti mandano in crisi qualsiasi editor di testo).
pub const MAX_FILE_SIZE: u64 = 64 * 1024 * 1024;

/// Terminatore di riga del file su disco.
///
/// Internamente il testo è **sempre** normalizzato a `\n`: così tutta la logica
/// di editing ha un solo caso da gestire, e la conversione avviene solo al
/// salvataggio.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Eol {
    #[default]
    Lf,
    Crlf,
}

impl Eol {
    /// La sequenza da scrivere su disco.
    pub const fn as_str(self) -> &'static str {
        match self {
            Eol::Lf => "\n",
            Eol::Crlf => "\r\n",
        }
    }

    /// Etichetta per la status bar.
    pub const fn label(self) -> &'static str {
        match self {
            Eol::Lf => "LF",
            Eol::Crlf => "CRLF",
        }
    }
}

/// Posizione del cursore, 0-based, con la colonna espressa in caratteri.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Cursor {
    pub line: usize,
    pub column: usize,
}

impl Cursor {
    pub const fn new(line: usize, column: usize) -> Self {
        Cursor { line, column }
    }
}

/// Errori nel lavoro su un singolo documento.
#[derive(Debug)]
pub enum DocumentError {
    Io(io::Error),
    /// Il file è troppo grande per essere aperto.
    TooLarge { size: u64, limit: u64 },
    /// File binario (contiene byte NUL nei primi KB).
    Binary(PathBuf),
    /// Non è UTF-8 valido.
    NotUtf8(PathBuf),
    /// Operazione che richiede un percorso (es. salvare un file "senza nome").
    NoPath,
}

impl fmt::Display for DocumentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DocumentError::Io(e) => write!(f, "{e}"),
            DocumentError::TooLarge { size, limit } => write!(
                f,
                "file troppo grande ({:.1} MB, limite {:.0} MB)",
                *size as f64 / 1_048_576.0,
                *limit as f64 / 1_048_576.0
            ),
            DocumentError::Binary(p) => write!(f, "{} sembra un file binario", p.display()),
            DocumentError::NotUtf8(p) => write!(f, "{} non è testo UTF-8 valido", p.display()),
            DocumentError::NoPath => write!(f, "il documento non ha ancora un percorso su disco"),
        }
    }
}

impl std::error::Error for DocumentError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            DocumentError::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<io::Error> for DocumentError {
    fn from(e: io::Error) -> Self {
        DocumentError::Io(e)
    }
}

/// Un file aperto.
#[derive(Debug, Clone)]
pub struct Document {
    /// Percorso su disco; `None` per un documento nuovo mai salvato.
    path: Option<PathBuf>,
    /// Nome mostrato nella tab finché non esiste un percorso.
    scratch_name: String,
    text: String,
    language: Language,
    eol: Eol,
    dirty: bool,
    read_only: bool,
    cursor: Cursor,
    /// Offset in byte delle `{` dei blocchi piegati (ordinati).
    ///
    /// È stato di *vista*, come il cursore: non entra nel testo e non rende il
    /// documento "modificato".
    folds: Vec<usize>,
    /// Incrementata a ogni modifica: la GUI la usa per invalidare le cache
    /// (evidenziazione, numeri di riga).
    revision: u64,
}

impl Document {
    /// Crea un documento vuoto non ancora legato a un file.
    pub fn new_untitled(name: impl Into<String>) -> Self {
        Document {
            path: None,
            scratch_name: name.into(),
            text: String::new(),
            language: Language::Text,
            eol: Eol::Lf,
            dirty: false,
            read_only: false,
            cursor: Cursor::default(),
            folds: Vec::new(),
            revision: 0,
        }
    }

    /// Legge un file da disco.
    ///
    /// Il percorso viene normalizzato (`canonicalize`): così due modi di
    /// indicare lo stesso file non aprono due tab.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, DocumentError> {
        let path = path.as_ref().canonicalize()?;
        let meta = fs::metadata(&path)?;
        if meta.len() > MAX_FILE_SIZE {
            return Err(DocumentError::TooLarge {
                size: meta.len(),
                limit: MAX_FILE_SIZE,
            });
        }
        let bytes = fs::read(&path)?;
        let head = &bytes[..bytes.len().min(8192)];
        if head.contains(&0) {
            return Err(DocumentError::Binary(path));
        }
        let raw = String::from_utf8(bytes).map_err(|_| DocumentError::NotUtf8(path.clone()))?;
        // Il BOM non fa parte del testo.
        let raw = raw.strip_prefix('\u{feff}').unwrap_or(&raw).to_string();

        let eol = if raw.contains("\r\n") { Eol::Crlf } else { Eol::Lf };
        let text = if eol == Eol::Crlf {
            raw.replace("\r\n", "\n")
        } else {
            raw
        };

        Ok(Document {
            path: Some(path.clone()),
            scratch_name: file_label(&path),
            text,
            language: Language::from_path(&path),
            eol,
            dirty: false,
            read_only: meta.permissions().readonly(),
            cursor: Cursor::default(),
            folds: Vec::new(),
            revision: 0,
        })
    }

    /// Percorso su disco, se esiste.
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Nome da mostrare (nome file, o nome provvisorio per i documenti nuovi).
    pub fn display_name(&self) -> &str {
        match &self.path {
            Some(p) => p.file_name().and_then(|n| n.to_str()).unwrap_or(&self.scratch_name),
            None => &self.scratch_name,
        }
    }

    /// Percorso completo per la status bar / tooltip.
    pub fn display_path(&self) -> String {
        match &self.path {
            Some(p) => p.display().to_string(),
            None => format!("{} (non salvato)", self.scratch_name),
        }
    }

    /// Percorso relativo alla radice del progetto, se il file è dentro il progetto.
    pub fn relative_to(&self, root: &Path) -> Option<PathBuf> {
        self.path
            .as_deref()
            .and_then(|p| p.strip_prefix(root).ok())
            .map(|p| p.to_path_buf())
    }

    /// Il testo corrente.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Accesso mutabile al testo, per `TextEdit` di egui.
    ///
    /// Segna il documento come modificato. Va usato per le modifiche fatte
    /// **dall'editor** (operazioni sulle righe, sostituzioni programmatiche).
    pub fn text_mut(&mut self) -> &mut String {
        self.touch();
        &mut self.text
    }

    /// Accesso grezzo al buffer, **senza** segnare modifiche.
    ///
    /// È quello che serve a `TextEdit` di egui, che riscrive il buffer anche
    /// quando l'utente non ha cambiato nulla. Chi lo usa deve chiamare
    /// [`Document::mark_edited`] quando l'utente ha davvero modificato il testo
    /// (nella GUI questo lo dice `Response::changed()`).
    pub fn buffer_mut(&mut self) -> &mut String {
        &mut self.text
    }

    /// Segnala che il testo è stato modificato dall'utente.
    pub fn mark_edited(&mut self) {
        self.touch();
    }

    /// Sostituisce il testo (annulla la cronologia concettualmente: la GUI
    /// ricostruisce le sue cache sulla nuova revisione).
    pub fn set_text(&mut self, text: impl Into<String>) {
        let text = text.into();
        if text != self.text {
            self.text = text;
            self.touch();
        }
    }

    /// Segna il documento come modificato (usato anche da `text_mut`).
    ///
    /// Modificare il testo riapre anche tutti i blocchi piegati: le posizioni
    /// delle graffe cambierebbero e la piegatura non avrebbe più senso.
    pub fn touch(&mut self) {
        self.dirty = true;
        self.folds.clear();
        self.revision = self.revision.wrapping_add(1);
    }

    /// Versione del testo: cambia a ogni modifica.
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    /// `true` se ci sono modifiche non salvate.
    pub const fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// `true` se il file su disco è di sola lettura.
    pub const fn is_read_only(&self) -> bool {
        self.read_only
    }

    /// Scrive sul percorso corrente. Errore se il documento è "senza nome".
    pub fn save(&mut self) -> Result<(), DocumentError> {
        match self.path.clone() {
            Some(path) => self.write_to(&path),
            None => Err(DocumentError::NoPath),
        }
    }

    /// Salva con un nuovo percorso: da qui in poi il documento "è" quel file.
    pub fn save_as(&mut self, path: impl AsRef<Path>) -> Result<(), DocumentError> {
        let path = path.as_ref().to_path_buf();
        self.write_to(&path)?;
        self.language = Language::from_path(&path);
        self.scratch_name = file_label(&path);
        self.path = Some(path.canonicalize().unwrap_or(path));
        Ok(())
    }

    fn write_to(&mut self, path: &Path) -> Result<(), DocumentError> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() && !parent.exists() {
                fs::create_dir_all(parent)?;
            }
        }
        let bytes = match self.eol {
            Eol::Lf => self.text.as_bytes().to_vec(),
            Eol::Crlf => self.text.replace('\n', "\r\n").into_bytes(),
        };
        fs::write(path, bytes)?;
        self.dirty = false;
        self.read_only = false;
        Ok(())
    }

    /// Rilegge il file da disco, **scartando** le modifiche non salvate.
    pub fn reload(&mut self) -> Result<(), DocumentError> {
        let Some(path) = self.path.clone() else {
            return Err(DocumentError::NoPath);
        };
        let fresh = Document::open(&path)?;
        self.text = fresh.text;
        self.eol = fresh.eol;
        self.read_only = fresh.read_only;
        self.dirty = false;
        self.folds.clear();
        self.revision = self.revision.wrapping_add(1);
        Ok(())
    }

    /// Adotta un nuovo percorso **senza scrivere su disco**.
    ///
    /// Serve quando il file viene rinominato o spostato dall'esterno: la tab
    /// deve continuare a puntare al file giusto.
    pub fn adopt_path(&mut self, path: impl AsRef<Path>) {
        let path = path.as_ref().to_path_buf();
        self.language = Language::from_path(&path);
        self.scratch_name = file_label(&path);
        self.path = Some(path.canonicalize().unwrap_or(path));
    }

    /// Imposta il terminatore di riga (applicato al prossimo salvataggio).
    pub fn set_eol(&mut self, eol: Eol) {
        if self.eol != eol {
            self.eol = eol;
            self.touch();
        }
    }

    pub const fn eol(&self) -> Eol {
        self.eol
    }

    pub const fn language(&self) -> Language {
        self.language
    }

    pub fn set_language(&mut self, language: Language) {
        self.language = language;
    }

    pub const fn cursor(&self) -> Cursor {
        self.cursor
    }

    /// Aggiorna la posizione del cursore (chiamata dalla GUI a ogni frame).
    pub fn set_cursor(&mut self, cursor: Cursor) {
        self.cursor = cursor;
    }

    /// Gli offset (in byte) delle `{` dei blocchi piegati, in ordine.
    pub fn folds(&self) -> &[usize] {
        &self.folds
    }

    /// Sostituisce l'insieme dei blocchi piegati.
    pub fn set_folds(&mut self, folds: Vec<usize>) {
        self.folds = folds;
    }

    /// `true` se il blocco che si apre a `open_byte` è piegato.
    pub fn is_folded(&self, open_byte: usize) -> bool {
        self.folds.binary_search(&open_byte).is_ok()
    }

    /// Piega il blocco indicato, o lo riapre se era già piegato.
    pub fn toggle_fold(&mut self, open_byte: usize) {
        match self.folds.binary_search(&open_byte) {
            Ok(index) => {
                self.folds.remove(index);
            }
            Err(index) => self.folds.insert(index, open_byte),
        }
    }

    /// Riapre il blocco indicato (se era piegato).
    pub fn unfold(&mut self, open_byte: usize) {
        if let Ok(index) = self.folds.binary_search(&open_byte) {
            self.folds.remove(index);
        }
    }

    /// Numero di righe del testo (≥ 1).
    pub fn line_count(&self) -> usize {
        crate::editor::line_count(&self.text)
    }

    /// Offset in byte dell'inizio di ogni riga.
    pub fn lines(&self) -> crate::editor::Lines {
        crate::editor::Lines::new(&self.text)
    }
}

fn file_label(path: &Path) -> String {
    path.file_name()
        .and_then(|n| n.to_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp_dir(nome: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ride-core-doc-{}-{}",
            std::process::id(),
            nome
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn nuovo_documento_non_e_sporco() {
        let mut doc = Document::new_untitled("1");
        assert!(!doc.is_dirty());
        doc.text_mut().push_str("ciao");
        assert!(doc.is_dirty());
        assert_eq!(doc.revision(), 1);
        assert!(matches!(doc.save(), Err(DocumentError::NoPath)));
    }

    #[test]
    fn salva_e_riapri() {
        let dir = temp_dir("salva");
        let path = dir.join("esempio.rs");
        {
            let mut doc = Document::new_untitled("1");
            doc.text_mut().push_str("fn main() {}\n");
            doc.save_as(&path).unwrap();
            assert!(!doc.is_dirty());
            assert_eq!(doc.display_name(), "esempio.rs");
            assert_eq!(doc.language(), Language::Rust);
        }
        let doc = Document::open(&path).unwrap();
        assert_eq!(doc.text(), "fn main() {}\n");
        assert_eq!(doc.eol(), Eol::Lf);
        assert!(!doc.is_dirty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn normalizza_crlf_e_lo_riscrive() {
        let dir = temp_dir("crlf");
        let path = dir.join("win.txt");
        let mut f = fs::File::create(&path).unwrap();
        f.write_all(b"a\r\nb\r\n").unwrap();
        drop(f);

        let mut doc = Document::open(&path).unwrap();
        assert_eq!(doc.eol(), Eol::Crlf);
        // Internamente solo \n.
        assert_eq!(doc.text(), "a\nb\n");
        doc.text_mut().push_str("c\n");
        doc.save().unwrap();
        let raw = fs::read(&path).unwrap();
        assert_eq!(raw, b"a\r\nb\r\nc\r\n");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn rifiuta_i_binari() {
        let dir = temp_dir("bin");
        let path = dir.join("bin.dat");
        fs::write(&path, [0u8, 1, 2, 3, 0, 9]).unwrap();
        assert!(matches!(
            Document::open(&path),
            Err(DocumentError::Binary(_))
        ));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn le_piegature_si_aprono_quando_il_testo_cambia() {
        let mut doc = Document::new_untitled("1");
        doc.set_text("fn main() {\n    x\n}\n");
        doc.toggle_fold(10);
        doc.toggle_fold(3);
        // Ordinate e interrogabili.
        assert_eq!(doc.folds(), &[3, 10]);
        assert!(doc.is_folded(10));
        doc.toggle_fold(3);
        assert_eq!(doc.folds(), &[10]);
        doc.unfold(10);
        assert!(doc.folds().is_empty());

        // Una modifica riapre tutto e non "sporca" la revisione a sproposito.
        doc.toggle_fold(10);
        doc.mark_edited();
        assert!(doc.folds().is_empty());
    }
}
