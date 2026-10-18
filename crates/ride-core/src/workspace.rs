//! Il workspace: i progetti aperti e i documenti aperti (le tab).
//!
//! Ogni progetto ha le **sue** tab: cambiando progetto cambiano i file mostrati.
//! I documenti aperti quando non c'è nessun progetto stanno in un gruppo a
//! parte, e passano al primo progetto che si apre (altrimenti resterebbero
//! invisibili dietro a un progetto attivo).

use std::fmt;
use std::path::{Path, PathBuf};

use crate::document::{Document, DocumentError};
use crate::project::Project;

/// Identificatore stabile di un documento aperto.
///
/// La GUI usa questo valore come `Id` di egui per la tab: così lo stato interno
/// (cursore, undo, scroll) resta agganciato al documento giusto anche quando le
/// tab cambiano ordine o ne vengono chiuse altre.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DocumentId(pub u64);

impl fmt::Display for DocumentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "doc{}", self.0)
    }
}

/// Errori a livello di workspace.
#[derive(Debug)]
pub enum WorkspaceError {
    Document(DocumentError),
    Io(std::io::Error),
}

impl fmt::Display for WorkspaceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WorkspaceError::Document(e) => write!(f, "{e}"),
            WorkspaceError::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for WorkspaceError {}

impl From<DocumentError> for WorkspaceError {
    fn from(e: DocumentError) -> Self {
        WorkspaceError::Document(e)
    }
}

impl From<std::io::Error> for WorkspaceError {
    fn from(e: std::io::Error) -> Self {
        WorkspaceError::Io(e)
    }
}

/// Le tab di una cartella: i documenti aperti e quello in primo piano.
#[derive(Debug, Default)]
struct Tabs {
    docs: Vec<(DocumentId, Document)>,
    active: Option<DocumentId>,
}

impl Tabs {
    fn get(&self, id: DocumentId) -> Option<&Document> {
        self.docs.iter().find(|(i, _)| *i == id).map(|(_, d)| d)
    }

    fn get_mut(&mut self, id: DocumentId) -> Option<&mut Document> {
        self.docs
            .iter_mut()
            .find(|(i, _)| *i == id)
            .map(|(_, d)| d)
    }

    fn index_of(&self, id: DocumentId) -> Option<usize> {
        self.docs.iter().position(|(i, _)| *i == id)
    }

    fn push(&mut self, id: DocumentId, doc: Document) {
        self.docs.push((id, doc));
        self.active = Some(id);
    }

    /// Chiude una tab. Se era quella attiva, attiva la vicina.
    fn close(&mut self, id: DocumentId) -> bool {
        let Some(index) = self.index_of(id) else {
            return false;
        };
        self.docs.remove(index);
        if self.active == Some(id) {
            self.active = if self.docs.is_empty() {
                None
            } else {
                // Preferisci la tab a destra, altrimenti quella a sinistra.
                Some(self.docs[index.min(self.docs.len() - 1)].0)
            };
        }
        true
    }
}

/// Un progetto aperto: la cartella e le sue tab.
#[derive(Debug)]
struct ProjectEntry {
    project: Project,
    tabs: Tabs,
}

/// Progetti aperti + documenti aperti.
///
/// Ogni progetto tiene le sue tab; il progetto *attivo* è quello di cui si
/// vedono albero, ricerca, build e file aperti — [`Self::root`] e
/// [`Self::project`] si riferiscono sempre a quello. I documenti aperti fuori
/// da ogni progetto stanno in un gruppo a parte.
#[derive(Debug, Default)]
pub struct Workspace {
    /// I progetti aperti, nell'ordine in cui sono stati aperti.
    projects: Vec<ProjectEntry>,
    /// Indice, in `projects`, del progetto attivo.
    active_project: Option<usize>,
    /// Tab dei documenti aperti quando non c'è nessun progetto: passano al
    /// primo progetto che si apre.
    loose: Tabs,
    next_id: u64,
    /// Contatore per i nomi dei documenti nuovi ("senza nome 1", "senza nome 2", …).
    untitled_counter: u64,
}

impl Workspace {
    pub fn new() -> Self {
        Self::default()
    }

    // ---- progetti ---------------------------------------------------------

    /// Apre una cartella come progetto e la rende attiva.
    ///
    /// Se la cartella è già aperta non se ne fa una seconda copia: la si
    /// riattiva e basta.
    pub fn open_project(&mut self, root: impl AsRef<Path>) -> Result<(), WorkspaceError> {
        let project = Project::open(root)?;
        if let Some(index) = self.index_of_project(project.root()) {
            self.active_project = Some(index);
            return Ok(());
        }
        // I documenti aperti "senza progetto" diventano le tab del progetto.
        let tabs = std::mem::take(&mut self.loose);
        self.projects.push(ProjectEntry { project, tabs });
        self.active_project = Some(self.projects.len() - 1);
        Ok(())
    }

    /// Chiude il progetto **attivo**: le sue tab se ne vanno con lui.
    pub fn close_project(&mut self) {
        if let Some(index) = self.active_project {
            self.close_project_at(index);
        }
    }

    /// Chiude un progetto. Se era quello attivo, l'attivo passa al vicino di
    /// destra e — se non c'è — a quello di sinistra.
    ///
    /// Le tab del progetto vengono chiuse: la GUI chiede prima conferma per
    /// quelle modificate.
    pub fn close_project_at(&mut self, index: usize) {
        if index >= self.projects.len() {
            return;
        }
        self.projects.remove(index);
        self.active_project = match self.active_project {
            Some(active) if active == index => {
                if self.projects.is_empty() {
                    None
                } else {
                    Some(index.min(self.projects.len() - 1))
                }
            }
            Some(active) if active > index => Some(active - 1),
            other => other,
        };
    }

    /// Il progetto attivo.
    pub fn project(&self) -> Option<&Project> {
        self.active_project.and_then(|index| self.projects.get(index))
            .map(|entry| &entry.project)
    }

    /// Cartella radice del progetto attivo, se c'è.
    pub fn root(&self) -> Option<&Path> {
        self.project().map(|p| p.root())
    }

    /// I progetti aperti, nell'ordine in cui sono stati aperti.
    pub fn projects(&self) -> impl Iterator<Item = &Project> {
        self.projects.iter().map(|entry| &entry.project)
    }

    /// Numero di progetti aperti.
    pub fn project_count(&self) -> usize {
        self.projects.len()
    }

    /// Il progetto in posizione `index`.
    pub fn project_at(&self, index: usize) -> Option<&Project> {
        self.projects.get(index).map(|entry| &entry.project)
    }

    /// Indice del progetto attivo.
    pub fn active_project_index(&self) -> Option<usize> {
        self.active_project
    }

    /// Indice del progetto con quella radice, se è aperto.
    ///
    /// La radice di un [`Project`] è già normalizzata, quindi il confronto è
    /// esatto: `/a/./b` e `/a/b` sono lo stesso progetto.
    pub fn index_of_project(&self, root: &Path) -> Option<usize> {
        self.projects.iter().position(|e| e.project.root() == root)
    }

    /// Rende attivo il progetto indicato; `false` se l'indice non esiste.
    pub fn activate_project(&mut self, index: usize) -> bool {
        if self.projects.get(index).is_some() {
            self.active_project = Some(index);
            true
        } else {
            false
        }
    }

    // ---- documenti --------------------------------------------------------

    /// Le tab del progetto attivo (o i documenti senza progetto).
    fn tabs(&self) -> &Tabs {
        match self.active_project {
            Some(index) => &self.projects[index].tabs,
            None => &self.loose,
        }
    }

    fn tabs_mut(&mut self) -> &mut Tabs {
        match self.active_project {
            Some(index) => &mut self.projects[index].tabs,
            None => &mut self.loose,
        }
    }

    /// Le tab in cui sta un documento: `Some(progetto)`, oppure `None` per i
    /// documenti fuori da ogni progetto. L'"esterno" `None` significa "non c'è".
    fn owner_of(&self, id: DocumentId) -> Option<Option<usize>> {
        if self.loose.index_of(id).is_some() {
            return Some(None);
        }
        self.projects
            .iter()
            .position(|e| e.tabs.index_of(id).is_some())
            .map(Some)
    }

    fn bucket_mut(&mut self, owner: Option<usize>) -> &mut Tabs {
        match owner {
            Some(index) => &mut self.projects[index].tabs,
            None => &mut self.loose,
        }
    }

    /// Documenti aperti (le tab del progetto attivo), nell'ordine delle tab.
    pub fn docs(&self) -> impl Iterator<Item = (DocumentId, &Document)> {
        self.tabs().docs.iter().map(|(id, doc)| (*id, doc))
    }

    /// Documenti aperti (le tab del progetto attivo), mutabili.
    pub fn docs_mut(&mut self) -> impl Iterator<Item = (DocumentId, &mut Document)> {
        self.tabs_mut().docs.iter_mut().map(|(id, doc)| (*id, doc))
    }

    /// *Tutti* i documenti aperti, di ogni progetto (per salvare o uscire).
    pub fn all_docs(&self) -> impl Iterator<Item = (DocumentId, &Document)> {
        self.projects
            .iter()
            .flat_map(|e| e.tabs.docs.iter())
            .chain(self.loose.docs.iter())
            .map(|(id, doc)| (*id, doc))
    }

    /// Numero di tab aperte (del progetto attivo).
    pub fn len(&self) -> usize {
        self.tabs().docs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tabs().docs.is_empty()
    }

    pub fn get(&self, id: DocumentId) -> Option<&Document> {
        match self.owner_of(id)? {
            Some(index) => self.projects[index].tabs.get(id),
            None => self.loose.get(id),
        }
    }

    pub fn get_mut(&mut self, id: DocumentId) -> Option<&mut Document> {
        let owner = self.owner_of(id)?;
        self.bucket_mut(owner).get_mut(id)
    }

    /// Indice della tab, fra quelle visibili (del progetto attivo).
    pub fn index_of(&self, id: DocumentId) -> Option<usize> {
        self.tabs().index_of(id)
    }

    /// Id del documento attivo.
    pub fn active_id(&self) -> Option<DocumentId> {
        self.tabs().active
    }

    pub fn active(&self) -> Option<&Document> {
        self.active_id().and_then(|id| self.get(id))
    }

    pub fn active_mut(&mut self) -> Option<&mut Document> {
        let id = self.active_id()?;
        self.get_mut(id)
    }

    /// Attiva una tab; ritorna `false` se l'id non esiste.
    ///
    /// Se la tab è di un altro progetto, quel progetto diventa quello attivo:
    /// altrimenti la tab non si vedrebbe.
    pub fn set_active(&mut self, id: DocumentId) -> bool {
        let Some(owner) = self.owner_of(id) else {
            return false;
        };
        if let Some(index) = owner {
            self.active_project = Some(index);
        }
        self.bucket_mut(owner).active = Some(id);
        true
    }

    /// Documento già aperto per quel percorso, se c'è (anche in altri progetti).
    pub fn find_by_path(&self, path: &Path) -> Option<DocumentId> {
        self.all_docs()
            .find(|(_, doc)| doc.path() == Some(path))
            .map(|(id, _)| id)
    }

    /// Apre un file, o attiva la tab che lo contiene già.
    pub fn open_path(&mut self, path: impl AsRef<Path>) -> Result<DocumentId, WorkspaceError> {
        let path = path.as_ref();
        let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        if let Some(id) = self.find_by_path(&canonical) {
            self.set_active(id);
            return Ok(id);
        }
        let doc = Document::open(&canonical)?;
        let owner = self.owner_for(&canonical);
        Ok(self.push(owner, doc))
    }

    /// In quali tab finisce un documento nuovo: il progetto che *contiene* il
    /// percorso (il più interno, se i progetti sono annidati), altrimenti il
    /// progetto attivo, altrimenti nessuno.
    fn owner_for(&self, path: &Path) -> Option<usize> {
        self.projects
            .iter()
            .enumerate()
            .filter(|(_, e)| path.starts_with(e.project.root()))
            .max_by_key(|(_, e)| e.project.root().as_os_str().len())
            .map(|(index, _)| index)
            .or(self.active_project)
    }

    /// Crea un documento nuovo, non ancora salvato.
    pub fn new_document(&mut self) -> DocumentId {
        self.untitled_counter += 1;
        let doc = Document::new_untitled(format!("senza nome {}", self.untitled_counter));
        let owner = self.active_project;
        self.push(owner, doc)
    }

    fn push(&mut self, owner: Option<usize>, doc: Document) -> DocumentId {
        self.next_id += 1;
        let id = DocumentId(self.next_id);
        match owner {
            Some(index) => {
                // La tab deve essere visibile: il progetto che la ospita diventa attivo.
                self.active_project = Some(index);
                self.projects[index].tabs.push(id, doc);
            }
            None => self.loose.push(id, doc),
        }
        id
    }

    /// Chiude una tab. Se era quella attiva, attiva la vicina.
    ///
    /// Ritorna `false` se l'id non esisteva. Le modifiche non salvate vengono
    /// **scartate**: è la GUI a chiedere conferma prima di chiamare questa.
    pub fn close(&mut self, id: DocumentId) -> bool {
        let Some(owner) = self.owner_of(id) else {
            return false;
        };
        self.bucket_mut(owner).close(id)
    }

    /// Chiude tutte le tab tranne una.
    pub fn close_others(&mut self, keep: DocumentId) {
        let tabs = self.tabs_mut();
        tabs.docs.retain(|(id, _)| *id == keep);
        tabs.active = tabs.docs.first().map(|(id, _)| *id);
    }

    /// Chiude tutte le tab visibili (del progetto attivo).
    pub fn close_all(&mut self) {
        let tabs = self.tabs_mut();
        tabs.docs.clear();
        tabs.active = None;
    }

    /// Sposta una tab (drag & drop).
    pub fn move_tab(&mut self, id: DocumentId, to: usize) {
        let tabs = self.tabs_mut();
        let Some(from) = tabs.index_of(id) else {
            return;
        };
        let to = to.min(tabs.docs.len().saturating_sub(1));
        if from != to {
            let item = tabs.docs.remove(from);
            tabs.docs.insert(to, item);
        }
    }

    /// Tab successiva (ciclica) rispetto a quella attiva.
    pub fn activate_next(&mut self) {
        self.activate_offset(1);
    }

    /// Tab precedente (ciclica).
    pub fn activate_previous(&mut self) {
        self.activate_offset(-1);
    }

    fn activate_offset(&mut self, delta: isize) {
        let tabs = self.tabs_mut();
        let n = tabs.docs.len();
        if n == 0 {
            return;
        }
        let current = tabs.active.and_then(|id| tabs.index_of(id)).unwrap_or(0);
        let next = (current as isize + delta).rem_euclid(n as isize) as usize;
        tabs.active = Some(tabs.docs[next].0);
    }

    // ---- salvataggio ------------------------------------------------------

    /// Salva un documento. Ritorna l'errore, se c'è stato.
    pub fn save(&mut self, id: DocumentId) -> Result<(), WorkspaceError> {
        let Some(doc) = self.get_mut(id) else {
            return Ok(());
        };
        doc.save()?;
        Ok(())
    }

    /// Salva il documento attivo.
    pub fn save_active(&mut self) -> Result<(), WorkspaceError> {
        match self.active_id() {
            Some(id) => self.save(id),
            None => Ok(()),
        }
    }

    /// Salva tutti i documenti modificati che hanno già un percorso, anche
    /// quelli dei progetti non attivi.
    ///
    /// Ritorna la lista degli errori: `(nome del documento, messaggio)`.
    pub fn save_all(&mut self) -> Vec<(String, String)> {
        let mut errors = Vec::new();
        for entry in self.projects.iter_mut() {
            Self::save_dirty(&mut entry.tabs.docs, &mut errors);
        }
        Self::save_dirty(&mut self.loose.docs, &mut errors);
        errors
    }

    fn save_dirty(docs: &mut [(DocumentId, Document)], errors: &mut Vec<(String, String)>) {
        for (_, doc) in docs.iter_mut() {
            if doc.is_dirty() && doc.path().is_some() {
                if let Err(e) = doc.save() {
                    errors.push((doc.display_name().to_string(), e.to_string()));
                }
            }
        }
    }

    /// Documenti con modifiche non salvate, in *tutti* i progetti: è la lista
    /// che serve per uscire senza perdere niente.
    pub fn dirty_docs(&self) -> Vec<DocumentId> {
        self.all_docs()
            .filter(|(_, doc)| doc.is_dirty())
            .map(|(id, _)| id)
            .collect()
    }

    /// Documenti con modifiche non salvate fra le tab visibili.
    pub fn dirty_tabs(&self) -> Vec<DocumentId> {
        self.docs()
            .filter(|(_, doc)| doc.is_dirty())
            .map(|(id, _)| id)
            .collect()
    }

    /// Numero di documenti con modifiche non salvate (tutti i progetti).
    pub fn dirty_count(&self) -> usize {
        self.all_docs().filter(|(_, doc)| doc.is_dirty()).count()
    }

    /// Id delle tab di un progetto (per chiuderle quando il progetto se ne va).
    pub fn tabs_of(&self, index: usize) -> Vec<DocumentId> {
        self.projects
            .get(index)
            .map(|e| e.tabs.docs.iter().map(|(id, _)| *id).collect())
            .unwrap_or_default()
    }

    /// Percorsi dei file aperti in un progetto, e quello della tab in primo
    /// piano. I documenti nuovi (senza percorso) restano fuori.
    pub fn project_files(&self, index: usize) -> (Vec<PathBuf>, Option<PathBuf>) {
        let Some(entry) = self.projects.get(index) else {
            return (Vec::new(), None);
        };
        percorsi(&entry.tabs)
    }

    /// Percorsi dei file aperti senza nessun progetto, e quello in primo piano.
    pub fn loose_files(&self) -> (Vec<PathBuf>, Option<PathBuf>) {
        percorsi(&self.loose)
    }

    /// Percorsi dei documenti aperti (per riaprirli al prossimo avvio).
    pub fn open_paths(&self) -> Vec<PathBuf> {
        self.all_docs()
            .filter_map(|(_, doc)| doc.path().map(|p| p.to_path_buf()))
            .collect()
    }
}

/// I percorsi dei file aperti in un gruppo di tab: tutti, e quello in primo
/// piano. I documenti senza percorso (i nuovi, ancora senza nome) sono esclusi.
fn percorsi(tabs: &Tabs) -> (Vec<PathBuf>, Option<PathBuf>) {
    let mut files = Vec::new();
    let mut active = None;
    for (id, doc) in &tabs.docs {
        let Some(path) = doc.path() else {
            continue;
        };
        files.push(path.to_path_buf());
        if tabs.active == Some(*id) {
            active = Some(path.to_path_buf());
        }
    }
    (files, active)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_dir(nome: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ride-ws-{}-{nome}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Un progetto temporaneo con dentro i file indicati.
    fn progetto(nome: &str, file: &[&str]) -> (PathBuf, PathBuf) {
        let dir = temp_dir(nome);
        let root = fs::canonicalize(&dir).unwrap();
        for nome_file in file {
            fs::write(root.join(nome_file), nome_file).unwrap();
        }
        (dir, root)
    }

    #[test]
    fn apri_due_tab_e_riusala() {
        let dir = temp_dir("tab");
        let a = dir.join("a.txt");
        let b = dir.join("b.txt");
        fs::write(&a, "a").unwrap();
        fs::write(&b, "b").unwrap();

        let mut ws = Workspace::new();
        let ia = ws.open_path(&a).unwrap();
        let ib = ws.open_path(&b).unwrap();
        assert_eq!(ws.len(), 2);
        assert_eq!(ws.active_id(), Some(ib));

        // Riaprire lo stesso file non crea una seconda tab.
        assert_eq!(ws.open_path(&a).unwrap(), ia);
        assert_eq!(ws.len(), 2);
        assert_eq!(ws.active_id(), Some(ia));

        // Chiudendo la tab attiva si passa a quella vicina.
        assert!(ws.close(ia));
        assert_eq!(ws.active_id(), Some(ib));
        assert!(ws.close(ib));
        assert_eq!(ws.active_id(), None);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn navigazione_ciclica() {
        let mut ws = Workspace::new();
        let a = ws.new_document();
        let b = ws.new_document();
        let c = ws.new_document();
        assert_eq!(ws.active_id(), Some(c));
        ws.activate_next();
        assert_eq!(ws.active_id(), Some(a));
        ws.activate_previous();
        assert_eq!(ws.active_id(), Some(c));
        ws.close(b);
        assert_eq!(ws.len(), 2);
    }

    #[test]
    fn piu_progetti_aperti_con_uno_attivo() {
        let (a, ra) = progetto("multi-a", &[]);
        let (b, rb) = progetto("multi-b", &[]);

        let mut ws = Workspace::new();
        assert!(ws.root().is_none());
        assert_eq!(ws.project_count(), 0);

        ws.open_project(&a).unwrap();
        assert_eq!(ws.root(), Some(ra.as_path()));
        ws.open_project(&b).unwrap();
        assert_eq!(ws.project_count(), 2);
        // L'ultimo aperto è quello attivo.
        assert_eq!(ws.active_project_index(), Some(1));
        assert_eq!(ws.root(), Some(rb.as_path()));

        // Si può tornare al primo.
        assert!(ws.activate_project(0));
        assert_eq!(ws.root(), Some(ra.as_path()));

        // Riaprire la stessa cartella non duplica il progetto: lo riattiva.
        ws.open_project(&a).unwrap();
        assert_eq!(ws.project_count(), 2);
        assert_eq!(ws.active_project_index(), Some(0));

        // Un indice inesistente non cambia niente.
        assert!(!ws.activate_project(9));
        assert_eq!(ws.root(), Some(ra.as_path()));
        assert!(ws.project_at(9).is_none());

        let _ = (fs::remove_dir_all(&a), fs::remove_dir_all(&b));
    }

    #[test]
    fn ogni_progetto_ha_le_sue_tab() {
        let (a, ra) = progetto("tab-a", &["a1.txt", "a2.txt"]);
        let (b, rb) = progetto("tab-b", &["b1.txt"]);

        let mut ws = Workspace::new();
        ws.open_project(&a).unwrap();
        let a1 = ws.open_path(ra.join("a1.txt")).unwrap();
        let a2 = ws.open_path(ra.join("a2.txt")).unwrap();
        assert_eq!(ws.len(), 2);

        // Aprendo il secondo progetto si vedono le *sue* tab, non quelle del primo.
        ws.open_project(&b).unwrap();
        let b1 = ws.open_path(rb.join("b1.txt")).unwrap();
        assert_eq!(ws.len(), 1);
        assert_eq!(ws.active_id(), Some(b1));
        assert_eq!(ws.all_docs().count(), 3);

        // Ogni progetto ricorda la tab in primo piano.
        ws.activate_project(0);
        assert_eq!(ws.active_id(), Some(a2));
        ws.activate_project(1);
        assert_eq!(ws.active_id(), Some(b1));

        // Attivare una tab di un altro progetto passa a quel progetto.
        assert!(ws.set_active(a1));
        assert_eq!(ws.active_project_index(), Some(0));
        assert_eq!(ws.active_id(), Some(a1));

        // "Chiudi le altre" agisce solo sulle tab del progetto attivo.
        ws.close_others(a1);
        assert_eq!(ws.len(), 1);
        assert_eq!(ws.all_docs().count(), 2);

        let _ = (fs::remove_dir_all(&a), fs::remove_dir_all(&b));
    }

    #[test]
    fn un_file_finisce_nel_progetto_che_lo_contiene() {
        let (a, ra) = progetto("proprietario-a", &["a.txt"]);
        let (b, _rb) = progetto("proprietario-b", &["b.txt"]);
        let (fuori, rf) = progetto("proprietario-fuori", &["f.txt"]);

        let mut ws = Workspace::new();
        ws.open_project(&a).unwrap();
        ws.open_project(&b).unwrap();
        assert_eq!(ws.active_project_index(), Some(1));

        // Un file di A aperto mentre è attivo B: va fra le tab di A (che torna
        // attivo), non fra quelle di B.
        let ta = ws.open_path(ra.join("a.txt")).unwrap();
        assert_eq!(ws.active_project_index(), Some(0));
        assert_eq!(ws.active_id(), Some(ta));
        assert_eq!(ws.tabs_of(1).len(), 0);

        // Riaprire lo stesso file lo riporta in primo piano senza duplicarlo.
        ws.activate_project(1);
        assert_eq!(ws.open_path(ra.join("a.txt")).unwrap(), ta);
        assert_eq!(ws.active_project_index(), Some(0));
        assert_eq!(ws.all_docs().count(), 1);

        // Un file fuori da ogni progetto resta nel progetto attivo.
        ws.activate_project(1);
        let tf = ws.open_path(rf.join("f.txt")).unwrap();
        assert_eq!(ws.active_project_index(), Some(1));
        assert_eq!(ws.tabs_of(1), vec![tf]);

        let _ = (fs::remove_dir_all(&a), fs::remove_dir_all(&b), fs::remove_dir_all(&fuori));
    }

    #[test]
    fn i_documenti_senza_progetto_passano_al_primo_progetto() {
        let (a, ra) = progetto("adozione", &["x.txt"]);

        let mut ws = Workspace::new();
        // Senza progetti i file aperti stanno in un gruppo a parte.
        let id = ws.open_path(ra.join("x.txt")).unwrap();
        assert_eq!(ws.project_count(), 0);
        assert_eq!(ws.len(), 1);

        // Aprendo un progetto diventano le sue tab: altrimenti sparirebbero
        // dietro al progetto attivo.
        ws.open_project(&a).unwrap();
        assert_eq!(ws.project_count(), 1);
        assert_eq!(ws.len(), 1);
        assert_eq!(ws.active_id(), Some(id));

        let _ = fs::remove_dir_all(&a);
    }

    #[test]
    fn chiudere_un_progetto_chiude_le_sue_tab() {
        let (a, ra) = progetto("chiudi-a", &["a.txt"]);
        let (b, rb) = progetto("chiudi-b", &["b.txt"]);

        let mut ws = Workspace::new();
        ws.open_project(&a).unwrap();
        let ta = ws.open_path(ra.join("a.txt")).unwrap();
        ws.open_project(&b).unwrap();
        let tb = ws.open_path(rb.join("b.txt")).unwrap();

        // Tornando ad A si rivede la sua tab.
        ws.activate_project(0);
        assert_eq!(ws.active_id(), Some(ta));

        // Chiudendo A se ne vanno le sue tab, non quelle di B.
        ws.close_project();
        assert_eq!(ws.project_count(), 1);
        assert_eq!(ws.root(), Some(rb.as_path()));
        assert_eq!(ws.active_id(), Some(tb));
        assert!(ws.get(ta).is_none());
        assert_eq!(ws.all_docs().count(), 1);

        // Chiudendo anche l'ultimo non resta nessun progetto né tab.
        ws.close_project();
        assert_eq!(ws.project_count(), 0);
        assert!(ws.root().is_none());
        assert!(ws.project().is_none());
        assert!(ws.active_project_index().is_none());
        assert!(ws.is_empty());
        assert!(ws.active_id().is_none());
        assert_eq!(ws.all_docs().count(), 0);

        // Chiudere senza progetti aperti non fa niente.
        ws.close_project();
        assert_eq!(ws.project_count(), 0);

        let _ = (fs::remove_dir_all(&a), fs::remove_dir_all(&b));
    }

    #[test]
    fn salva_tutto_riporta_gli_errori_dei_senza_nome() {
        let mut ws = Workspace::new();
        ws.new_document();
        ws.active_mut().unwrap().text_mut().push('x');
        let errors = ws.save_all();
        // Il documento senza percorso non viene nemmeno tentato.
        assert!(errors.is_empty());
        assert_eq!(ws.dirty_count(), 1);
    }
}
