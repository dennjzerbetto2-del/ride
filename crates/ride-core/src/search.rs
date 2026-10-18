//! Ricerca nel progetto (stile "cerca in tutti i file").
//!
//! La scansione riusa la stessa logica dell'albero ([`crate::project::TreeOptions`]),
//! quindi rispetta `.gitignore` e salta le cartelle di servizio.

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use regex::{Regex, RegexBuilder};

use crate::project::TreeOptions;

/// Oltre questa dimensione un file viene saltato dalla ricerca.
pub const MAX_SEARCH_FILE_SIZE: u64 = 2 * 1024 * 1024;

/// Cosa cercare e come.
#[derive(Debug, Clone)]
pub struct SearchQuery {
    pub text: String,
    pub case_sensitive: bool,
    /// Solo parole intere.
    pub whole_word: bool,
    /// Interpreta `text` come espressione regolare.
    pub use_regex: bool,
    /// Filtro glob sui percorsi (es. `*.rs`), applicato dal server di ricerca.
    pub include: Option<String>,
    /// Numero massimo di risultati.
    pub max_results: usize,
}

impl Default for SearchQuery {
    fn default() -> Self {
        SearchQuery {
            text: String::new(),
            case_sensitive: false,
            whole_word: false,
            use_regex: false,
            include: None,
            max_results: 2_000,
        }
    }
}

impl SearchQuery {
    /// `true` se non c'è nulla da cercare.
    pub fn is_empty(&self) -> bool {
        self.text.trim().is_empty()
    }

    /// Compila l'espressione regolare corrispondente alla query.
    pub fn compile(&self) -> Result<Regex, RegexError> {
        let pattern = if self.use_regex {
            self.text.clone()
        } else {
            regex::escape(&self.text)
        };
        let pattern = if self.whole_word {
            format!(r"\b(?:{pattern})\b")
        } else {
            pattern
        };
        let re = RegexBuilder::new(&pattern)
            .case_insensitive(!self.case_sensitive)
            .multi_line(true)
            .build()?;
        Ok(re)
    }
}

/// Errore di compilazione della query.
#[derive(Debug)]
pub struct RegexError(regex::Error);

impl fmt::Display for RegexError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "espressione regolare non valida: {}", self.0)
    }
}

impl std::error::Error for RegexError {}

impl From<regex::Error> for RegexError {
    fn from(e: regex::Error) -> Self {
        RegexError(e)
    }
}

/// Una riga che contiene una corrispondenza.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchHit {
    pub path: PathBuf,
    /// Numero di riga, 0-based.
    pub line: usize,
    /// Colonna, in caratteri, 0-based.
    pub column: usize,
    /// Lunghezza della corrispondenza, in caratteri.
    pub len: usize,
    /// Testo completo della riga, senza terminatore.
    pub line_text: String,
}

/// Esito di una ricerca: risultati più informazioni sullo stato.
#[derive(Debug, Clone, Default)]
pub struct SearchResult {
    pub hits: Vec<SearchHit>,
    /// File effettivamente letti.
    pub files_scanned: usize,
    /// File saltati perché troppo grandi o binari.
    pub files_skipped: usize,
    /// `true` se è stato raggiunto `max_results`.
    pub truncated: bool,
}

impl SearchResult {
    /// Raggruppa i risultati per file, mantenendo l'ordine di scoperta.
    pub fn by_file(&self) -> Vec<(PathBuf, Vec<&SearchHit>)> {
        let mut groups: Vec<(PathBuf, Vec<&SearchHit>)> = Vec::new();
        for hit in &self.hits {
            match groups.last_mut() {
                Some((path, hits)) if *path == hit.path => hits.push(hit),
                _ => groups.push((hit.path.clone(), vec![hit])),
            }
        }
        groups
    }
}

/// Cerca in tutti i file del progetto.
pub fn search_project(root: &Path, opts: &TreeOptions, query: &SearchQuery) -> Result<SearchResult, RegexError> {
    if query.is_empty() {
        return Ok(SearchResult::default());
    }
    let re = query.compile()?;
    let mut result = SearchResult::default();

    let mut builder = ignore::WalkBuilder::new(root);
    builder
        .max_depth(None)
        .hidden(!opts.show_hidden)
        .follow_links(false)
        .sort_by_file_name(|a, b| a.cmp(b));
    if opts.respect_gitignore {
        builder
            .git_ignore(true)
            .git_global(true)
            .git_exclude(true)
            .parents(true);
    } else {
        builder
            .git_ignore(false)
            .git_global(false)
            .git_exclude(false)
            .parents(false);
    }
    let skip = opts.skip_dirs;
    builder.filter_entry(move |entry| {
        if entry.depth() == 0 {
            return true;
        }
        if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            if let Some(name) = entry.file_name().to_str() {
                return !skip.contains(&name);
            }
        }
        true
    });
    if let Some(glob) = query.include.as_deref().filter(|g| !g.trim().is_empty()) {
        let mut overrides = ignore::overrides::OverrideBuilder::new(root);
        if overrides.add(glob).is_ok() {
            if let Ok(overrides) = overrides.build() {
                builder.overrides(overrides);
            }
        }
    }

    for entry in builder.build() {
        if result.truncated {
            break;
        }
        let Ok(entry) = entry else { continue };
        if entry.depth() == 0 || !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }
        let path = entry.path();
        match search_file(path, &re, query.max_results - result.hits.len()) {
            Ok(FileOutcome::Hits(hits)) => {
                result.files_scanned += 1;
                result.hits.extend(hits);
                if result.hits.len() >= query.max_results {
                    result.truncated = true;
                }
            }
            Ok(FileOutcome::Skipped) => result.files_skipped += 1,
            Err(_) => result.files_skipped += 1,
        }
    }

    Ok(result)
}

enum FileOutcome {
    Hits(Vec<SearchHit>),
    Skipped,
}

fn search_file(path: &Path, re: &Regex, limit: usize) -> std::io::Result<FileOutcome> {
    let meta = fs::metadata(path)?;
    if meta.len() > MAX_SEARCH_FILE_SIZE {
        return Ok(FileOutcome::Skipped);
    }
    let bytes = fs::read(path)?;
    if bytes[..bytes.len().min(4096)].contains(&0) {
        return Ok(FileOutcome::Skipped);
    }
    let Ok(text) = String::from_utf8(bytes) else {
        return Ok(FileOutcome::Skipped);
    };
    Ok(FileOutcome::Hits(search_text(&text, re, path, limit)))
}

/// Cerca `re` nel testo, restituendo un risultato per riga (il primo match).
///
/// Fermarsi alla prima corrispondenza per riga è quello che fa anche `ripgrep`
/// nel formato "righe": evita di riempire l'elenco con mille hit identiche.
pub fn search_text(text: &str, re: &Regex, path: &Path, limit: usize) -> Vec<SearchHit> {
    let mut hits = Vec::new();
    if limit == 0 {
        return hits;
    }
    for (line, chunk) in text.split_inclusive('\n').enumerate() {
        let body = chunk.trim_end_matches('\n').trim_end_matches('\r');
        if let Some(m) = re.find(body) {
            hits.push(SearchHit {
                path: path.to_path_buf(),
                line,
                column: body[..m.start()].chars().count(),
                len: m.as_str().chars().count(),
                line_text: body.to_string(),
            });
            if hits.len() >= limit {
                break;
            }
        }
    }
    hits
}

/// Tutte le corrispondenze di `re` nel testo, come intervalli di **caratteri**
/// (indici di char, non di byte: è la convenzione dei cursori di egui).
///
/// Le corrispondenze vuote (possibili con espressioni come `a*`) vengono
/// scartate: evidenziare "nulla" non serve a nessuno.
pub fn find_all(text: &str, re: &Regex, limit: usize) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut char_pos = 0usize;
    let mut byte_pos = 0usize;
    for m in re.find_iter(text) {
        if ranges.len() >= limit {
            break;
        }
        if m.start() < byte_pos {
            continue; // corrispondenze sovrapposte: la prima vince
        }
        char_pos += text[byte_pos..m.start()].chars().count();
        let len = m.as_str().chars().count();
        if len > 0 {
            ranges.push((char_pos, char_pos + len));
            char_pos += len;
        }
        byte_pos = m.end();
    }
    ranges
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ricerca_semplice() {
        let q = SearchQuery {
            text: "main".into(),
            ..Default::default()
        };
        let re = q.compile().unwrap();
        let hits = search_text("fn main() {}\n// main\n", &re, Path::new("a.rs"), 10);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].line, 0);
        assert_eq!(hits[0].column, 3);
        assert_eq!(hits[0].len, 4);
    }

    #[test]
    fn case_insensitive_per_default() {
        let q = SearchQuery {
            text: "MAIN".into(),
            ..Default::default()
        };
        let re = q.compile().unwrap();
        assert_eq!(search_text("fn main()", &re, Path::new("a"), 10).len(), 1);

        let q = SearchQuery {
            text: "MAIN".into(),
            case_sensitive: true,
            ..Default::default()
        };
        let re = q.compile().unwrap();
        assert_eq!(search_text("fn main()", &re, Path::new("a"), 10).len(), 0);
    }

    #[test]
    fn parola_intera() {
        let q = SearchQuery {
            text: "main".into(),
            whole_word: true,
            ..Default::default()
        };
        let re = q.compile().unwrap();
        assert_eq!(search_text("main()", &re, Path::new("a"), 10).len(), 1);
        assert_eq!(search_text("domain()", &re, Path::new("a"), 10).len(), 0);
    }

    #[test]
    fn regex_e_ordine_dei_risultati() {
        let q = SearchQuery {
            text: r"fn\s+\w+".into(),
            use_regex: true,
            ..Default::default()
        };
        let re = q.compile().unwrap();
        let hits = search_text("fn a() {}\nfn b() {}\n", &re, Path::new("a.rs"), 10);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[1].line, 1);
    }

    #[test]
    fn regex_non_valida_da_errore() {
        let q = SearchQuery {
            text: "(".into(),
            use_regex: true,
            ..Default::default()
        };
        assert!(q.compile().is_err());
    }

    #[test]
    fn trova_tutte_le_corrispondenze_in_caratteri() {
        let re = Regex::new("ab").unwrap();
        // `è` occupa un solo carattere ma due byte: gli indici sono di carattere.
        assert_eq!(find_all("èab ab", &re, 10), vec![(1, 3), (4, 6)]);
        // Più corrispondenze sulla stessa riga.
        assert_eq!(find_all("abab", &re, 10), vec![(0, 2), (2, 4)]);
        // Il limite viene rispettato.
        assert_eq!(find_all("abab", &re, 1), vec![(0, 2)]);
        // Corrispondenze vuote scartate.
        let re = Regex::new("x*").unwrap();
        assert!(find_all("abc", &re, 10).is_empty());
    }

    #[test]
    fn cerca_in_un_progetto() {
        let root = std::env::temp_dir().join(format!("ride-search-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/a.rs"), "fn main() {}\nlet x = 1;\n").unwrap();
        fs::write(root.join("src/b.rs"), "let y = 2;\n").unwrap();
        fs::write(root.join("notes.md"), "main\n").unwrap();

        let q = SearchQuery {
            text: "main".into(),
            ..Default::default()
        };
        let res = search_project(&root, &TreeOptions::default(), &q).unwrap();
        assert_eq!(res.hits.len(), 2);
        assert!(res.hits.iter().any(|h| h.path.ends_with("a.rs")));
        assert!(res.hits.iter().any(|h| h.path.ends_with("notes.md")));
        let _ = fs::remove_dir_all(&root);
    }
}
