//! Il progetto: una cartella radice e l'albero dei file contenuti.

use std::path::{Path, PathBuf};

use ignore::WalkBuilder;

/// Cartelle che quasi mai interessano e che è meglio non mostrare, a meno che
/// l'utente non le apra esplicitamente (o non sia attivo `.gitignore`).
pub const DEFAULT_SKIP_DIRS: &[&str] = &[
    ".git",
    ".hg",
    ".svn",
    "target",
    "node_modules",
    "__pycache__",
    ".venv",
    "venv",
    ".cache",
    ".gradle",
    ".tox",
    ".idea",
    ".DS_Store",
];

/// Come esplorare le cartelle del progetto.
#[derive(Debug, Clone)]
pub struct TreeOptions {
    /// Mostra i file e le cartelle che iniziano con `.`.
    pub show_hidden: bool,
    /// Rispetta `.gitignore`, `.ignore` e le esclusioni globali di git.
    pub respect_gitignore: bool,
    /// Cartelle da non mostrare.
    pub skip_dirs: &'static [&'static str],
    /// Limite di elementi letti per singola cartella.
    pub max_entries_per_dir: usize,
}

impl Default for TreeOptions {
    fn default() -> Self {
        TreeOptions {
            show_hidden: false,
            respect_gitignore: true,
            skip_dirs: DEFAULT_SKIP_DIRS,
            max_entries_per_dir: 5_000,
        }
    }
}

impl TreeOptions {
    /// Crea un walker configurato come richiesto, con profondità massima `depth`.
    fn walker(&self, dir: &Path, depth: Option<usize>) -> ignore::Walk {
        let mut builder = WalkBuilder::new(dir);
        builder
            .max_depth(depth)
            .hidden(!self.show_hidden)
            .follow_links(false)
            .sort_by_file_name(|a, b| a.cmp(b));
        if self.respect_gitignore {
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
        let skip = self.skip_dirs;
        builder.filter_entry(move |entry| {
            // Il filtro non deve mai escludere la radice della scansione.
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
        builder.build()
    }

    /// `true` se la cartella va nascosta/ignorata.
    pub fn skips_dir_name(&self, name: &str) -> bool {
        self.skip_dirs.contains(&name)
    }
}

/// Progetto aperto.
#[derive(Debug, Clone)]
pub struct Project {
    root: PathBuf,
    name: String,
}

impl Project {
    /// Apre una cartella come progetto.
    pub fn open(root: impl AsRef<Path>) -> std::io::Result<Project> {
        let root = root.as_ref().canonicalize()?;
        if !root.is_dir() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("{} non è una cartella", root.display()),
            ));
        }
        let name = root
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("progetto")
            .to_string();
        Ok(Project { root, name })
    }

    /// Cartella radice (percorso assoluto e normalizzato).
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Nome del progetto (ultimo componente della cartella).
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Percorso relativo al progetto, se il file ne fa parte.
    pub fn relative(&self, path: &Path) -> Option<PathBuf> {
        path.strip_prefix(&self.root).ok().map(|p| p.to_path_buf())
    }

    /// Radice dell'albero, non ancora espansa.
    pub fn root_node(&self) -> FileNode {
        FileNode::dir(self.root.clone())
    }

    /// Tutti i file del progetto, ordinati: usato dalla "apertura rapida"
    /// (Ctrl+P).
    pub fn list_files(&self, opts: &TreeOptions) -> Vec<PathBuf> {
        let mut files = Vec::new();
        for entry in opts.walker(&self.root, None) {
            let Ok(entry) = entry else { continue };
            if entry.depth() == 0 {
                continue;
            }
            if entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                files.push(entry.path().to_path_buf());
            }
            if files.len() >= 100_000 {
                break;
            }
        }
        files
    }
}

/// Un elemento dell'albero (cartella o file).
///
/// I figli vengono caricati **pigramente**: all'apertura del progetto si legge
/// solo la radice, e ogni cartella viene letta la prima volta che la si espande.
/// Così aprire un progetto grande resta istantaneo.
#[derive(Debug, Clone)]
pub struct FileNode {
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
    pub expanded: bool,
    /// `None` = figli non ancora letti.
    pub children: Option<Vec<FileNode>>,
    /// Errore di lettura della cartella (permessi, …).
    pub error: Option<String>,
}

impl FileNode {
    /// Nodo di tipo cartella, non espanso.
    pub fn dir(path: PathBuf) -> Self {
        let name = node_name(&path);
        FileNode {
            path,
            name,
            is_dir: true,
            expanded: false,
            children: None,
            error: None,
        }
    }

    /// Nodo foglia di tipo file.
    pub fn file(path: PathBuf) -> Self {
        let name = node_name(&path);
        FileNode {
            path,
            name,
            is_dir: false,
            expanded: false,
            children: None,
            error: None,
        }
    }

    /// `true` se i figli sono già stati letti.
    pub const fn is_loaded(&self) -> bool {
        self.children.is_some()
    }

    /// Legge i figli di una cartella (una sola volta, se non è già fatto).
    pub fn load(&mut self, opts: &TreeOptions) {
        if self.children.is_some() || !self.is_dir {
            return;
        }
        match read_children(&self.path, opts) {
            Ok(children) => {
                self.children = Some(children);
                self.error = None;
            }
            Err(e) => {
                self.children = Some(Vec::new());
                self.error = Some(e.to_string());
            }
        }
    }

    /// Espande o richiude una cartella; ritorna il nuovo stato di espansione.
    pub fn toggle(&mut self, opts: &TreeOptions) -> bool {
        if !self.is_dir {
            return false;
        }
        if self.expanded {
            self.expanded = false;
        } else {
            self.load(opts);
            self.expanded = true;
        }
        self.expanded
    }

    /// Rilegge i figli da disco mantenendo l'espansione, per quanto possibile.
    pub fn refresh(&mut self, opts: &TreeOptions) {
        let open_paths = self.expanded_paths();
        self.children = None;
        self.load(opts);
        self.restore_expansion(&open_paths, opts);
    }

    /// Percorsi delle cartelle attualmente espanse.
    pub fn expanded_paths(&self) -> Vec<PathBuf> {
        let mut out = Vec::new();
        self.collect_expanded(&mut out);
        out
    }

    fn collect_expanded(&self, out: &mut Vec<PathBuf>) {
        if self.expanded && self.is_dir {
            out.push(self.path.clone());
        }
        for child in self.children.as_deref().unwrap_or(&[]) {
            child.collect_expanded(out);
        }
    }

    /// Espande le cartelle elencate in `open`, leggendo i livelli che servono.
    ///
    /// È il modo per ritrovare l'albero com'era: dopo aver letto la radice si
    /// riaprono le cartelle che erano aperte.
    pub fn restore_expansion(&mut self, open: &[PathBuf], opts: &TreeOptions) {
        if !self.is_dir {
            return;
        }
        if self.expanded || open.contains(&self.path) {
            self.expanded = true;
            self.load(opts);
            if let Some(children) = self.children.as_mut() {
                for child in children {
                    child.restore_expansion(open, opts);
                }
            }
        }
    }

    /// Cerca un nodo per percorso.
    pub fn find(&self, path: &Path) -> Option<&FileNode> {
        if self.path == path {
            return Some(self);
        }
        self.children
            .as_deref()?
            .iter()
            .find_map(|child| child.find(path))
    }

    /// Cerca un nodo per percorso, mutabile.
    pub fn find_mut(&mut self, path: &Path) -> Option<&mut FileNode> {
        if self.path == path {
            return Some(self);
        }
        self.children
            .as_deref_mut()?
            .iter_mut()
            .find_map(|child| child.find_mut(path))
    }

    /// Percorso dell'antenato (cartella) che contiene `path`, caricando
    /// i livelli necessari e riespandendo il cammino. Ritorna `true` se il
    /// percorso esiste nell'albero.
    pub fn reveal(&mut self, path: &Path, opts: &TreeOptions) -> bool {
        self.reveal_inner(path, opts, true)
    }

    fn reveal_inner(&mut self, path: &Path, opts: &TreeOptions, allow_reload: bool) -> bool {
        if self.path == path {
            return true;
        }
        if !self.is_dir || !path.starts_with(&self.path) {
            return false;
        }
        self.load(opts);
        self.expanded = true;
        let found = self
            .children
            .as_mut()
            .map(|children| {
                children
                    .iter_mut()
                    .any(|child| child.reveal_inner(path, opts, allow_reload))
            })
            .unwrap_or(false);
        if found {
            return true;
        }
        // Il file potrebbe essere stato creato dopo l'ultima lettura.
        if allow_reload {
            self.children = None;
            self.load(opts);
            return self
                .children
                .as_mut()
                .map(|children| {
                    children
                        .iter_mut()
                        .any(|child| child.reveal_inner(path, opts, false))
                })
                .unwrap_or(false);
        }
        false
    }

    /// Tutti i file dell'albero già letto.
    pub fn files(&self, out: &mut Vec<PathBuf>) {
        if !self.is_dir {
            out.push(self.path.clone());
            return;
        }
        for child in self.children.as_deref().unwrap_or(&[]) {
            child.files(out);
        }
    }
}

fn node_name(path: &Path) -> String {
    path.file_name()
        .and_then(|n| n.to_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| path.display().to_string())
}

/// Legge i figli diretti di una cartella, cartelle prima e ordine alfabetico.
fn read_children(dir: &Path, opts: &TreeOptions) -> std::io::Result<Vec<FileNode>> {
    let mut dirs = Vec::new();
    let mut files = Vec::new();
    let mut errors = Vec::new();

    for entry in opts.walker(dir, Some(1)) {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                errors.push(e.to_string());
                continue;
            }
        };
        if entry.depth() == 0 {
            continue; // la cartella stessa
        }
        let path = entry.path().to_path_buf();
        match entry.file_type() {
            Some(t) if t.is_dir() => dirs.push(FileNode::dir(path)),
            Some(t) if t.is_file() => files.push(FileNode::file(path)),
            // Symlink e casi strani: se punta a una cartella lo mostro come cartella.
            _ => {
                if path.is_dir() {
                    dirs.push(FileNode::dir(path));
                } else {
                    files.push(FileNode::file(path));
                }
            }
        }
        if dirs.len() + files.len() >= opts.max_entries_per_dir {
            break;
        }
    }

    let sort = |a: &FileNode, b: &FileNode| {
        a.name
            .to_ascii_lowercase()
            .cmp(&b.name.to_ascii_lowercase())
            .then_with(|| a.name.cmp(&b.name))
    };
    dirs.sort_by(sort);
    files.sort_by(sort);

    if dirs.is_empty() && files.is_empty() && !errors.is_empty() {
        return Err(std::io::Error::other(errors.join("; ")));
    }

    dirs.extend(files);
    Ok(dirs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_project(nome: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ride-core-proj-{}-{}",
            std::process::id(),
            nome
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("src/nested")).unwrap();
        fs::create_dir_all(dir.join("target/debug")).unwrap();
        fs::write(dir.join("Cargo.toml"), "[package]\n").unwrap();
        fs::write(dir.join("README.md"), "# progetto\n").unwrap();
        fs::write(dir.join("src/main.rs"), "fn main() {}\n").unwrap();
        fs::write(dir.join("src/lib.rs"), "\n").unwrap();
        fs::write(dir.join("src/nested/deep.txt"), "x\n").unwrap();
        fs::write(dir.join("target/debug/junk"), "x\n").unwrap();
        dir
    }

    #[test]
    fn apre_il_progetto_e_legge_i_figli() {
        let root = temp_project("apri");
        let project = Project::open(&root).unwrap();
        let opts = TreeOptions::default();
        let mut node = project.root_node();
        node.load(&opts);
        let names: Vec<_> = node
            .children
            .as_ref()
            .unwrap()
            .iter()
            .map(|c| c.name.as_str())
            .collect();
        // Cartelle prima dei file, `target` escluso.
        assert_eq!(names, vec!["src", "Cargo.toml", "README.md"]);
        assert!(node.children.as_ref().unwrap()[0].children.is_none());

        node.toggle(&opts);
        assert!(node.expanded);
        let _ = fs::remove_dir_all(&root);
    }

    /// Ritrovare l'albero com'era: le cartelle che erano aperte si riaprono
    /// (anche quelle annidate, che si leggono solo se servono), le altre no.
    #[test]
    fn si_riaprono_le_cartelle_che_erano_espanse() {
        let root = temp_project("riespandi");
        let project = Project::open(&root).unwrap();
        let opts = TreeOptions::default();

        // Come se si fossero aperte `src` e poi `src/nested`.
        let mut node = project.root_node();
        node.load(&opts);
        node.expanded = true;
        let src = node.path.join("src");
        let nested = src.join("nested");
        node.find_mut(&src).unwrap().toggle(&opts);
        node.find_mut(&nested).unwrap().toggle(&opts);
        let aperti = node.expanded_paths();
        assert_eq!(aperti, vec![node.path.clone(), src.clone(), nested.clone()]);

        // Un albero letto da zero, con le stesse cartelle riaperte.
        let mut nuovo = project.root_node();
        nuovo.load(&opts);
        nuovo.expanded = true;
        assert!(nuovo.find(&nested).is_none(), "nested non è ancora letto");
        nuovo.restore_expansion(&aperti, &opts);
        assert_eq!(nuovo.expanded_paths(), aperti);

        // `target` non compare nemmeno (lo escludono le regole di default).
        let target = nuovo.path.join("target");
        assert!(nuovo.find(&target).is_none());

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn elenca_i_file_del_progetto() {
        let root = temp_project("lista");
        let project = Project::open(&root).unwrap();
        let mut files = project.list_files(&TreeOptions::default());
        files.sort();
        let names: Vec<String> = files
            .iter()
            .map(|p| project.relative(p).unwrap().display().to_string())
            .collect();
        assert!(names.contains(&"Cargo.toml".to_string()));
        assert!(names.contains(&"src/nested/deep.txt".to_string()));
        assert!(!names.iter().any(|n| n.starts_with("target")));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn rivela_un_file_annidato() {
        let root = temp_project("rivela");
        let project = Project::open(&root).unwrap();
        let opts = TreeOptions::default();
        let mut node = project.root_node();
        let target = root.join("src/nested/deep.txt");
        assert!(node.reveal(&target, &opts));
        assert!(node.find(&target).is_some());
        let _ = fs::remove_dir_all(&root);
    }
}
