//! La sessione: i progetti aperti e, per ciascuno, i file aperti.
//!
//! Serve a ritrovare al prossimo avvio la stessa scrivania: le cartelle aperte,
//! le loro tab e quale era in primo piano. Non è una configurazione (quella sta
//! nelle preferenze della GUI): è una fotografia di com'era il lavoro, e se il
//! file non c'è più si salta senza drammi.

use std::path::{Path, PathBuf};

use crate::workspace::Workspace;

/// I file aperti di un progetto.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProjectSession {
    /// Cartella radice del progetto.
    pub root: PathBuf,
    /// Colore personalizzato del progetto (RGBA), se impostato.
    pub color: Option<[u8; 4]>,
    /// File aperti, nell'ordine delle tab.
    pub files: Vec<PathBuf>,
    /// Tab in primo piano, se c'era.
    pub active: Option<PathBuf>,
    /// Cartelle che erano espanse nell'albero.
    ///
    /// Le riempie la GUI (l'albero è roba sua); a [`Session::restore`] non
    /// servono, le usa il livello che disegna l'albero per ritrovarlo com'era.
    pub expanded: Vec<PathBuf>,
    /// Voce selezionata nell'albero.
    pub selected: Option<PathBuf>,
}

/// Com'era l'editor: i progetti aperti e i loro file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Session {
    /// I progetti aperti, nell'ordine in cui erano.
    pub projects: Vec<ProjectSession>,
    /// Radice del progetto attivo.
    pub active_project: Option<PathBuf>,
    /// File aperti quando non c'era nessun progetto.
    pub loose_files: Vec<PathBuf>,
    /// Tab in primo piano fra i file senza progetto.
    pub loose_active: Option<PathBuf>,
}

/// Etichette delle righe del formato: una lettera, poi il percorso (o il colore RGBA per `C`).
///
/// `P` progetto, `F` file aperto, `A` tab in primo piano, `E` cartella espansa,
/// `V` voce selezionata; poi le righe di chi non ha un progetto: `L` file,
/// `S` tab in primo piano. `C` salva il colore del progetto e `T` il progetto
/// attivo.
const PROGETTO: char = 'P';
const FILE: char = 'F';
const ATTIVO: char = 'A';
const ESPANSO: char = 'E';
const SELEZIONATO: char = 'V';
const SCIOLTO: char = 'L';
const SCIOLTO_ATTIVO: char = 'S';
const PROGETTO_ATTIVO: char = 'T';
const COLORE_PROGETTO: char = 'C';

impl Session {
    /// Una sessione vuota: niente da riaprire.
    pub fn is_empty(&self) -> bool {
        self.projects.is_empty() && self.loose_files.is_empty()
    }

    /// Quanti file (o progetti) la sessione descrive.
    pub fn len(&self) -> usize {
        self.projects.len()
            + self.projects.iter().map(|p| p.files.len()).sum::<usize>()
            + self.loose_files.len()
    }

    /// Scrive la sessione in una stringa: una riga per voce.
    ///
    /// Il formato è volutamente banale (`P<percorso>`, `F<percorso>`, …) perché
    /// si possa leggere e correggere a mano: i caratteri che confonderebbero la
    /// lettura (a capo, ritorno carrello, barra rovesciata) si scrivono
    /// preceduti da `\`.
    pub fn encode(&self) -> String {
        let mut lines: Vec<String> = Vec::new();
        for file in &self.loose_files {
            lines.push(riga(SCIOLTO, file));
        }
        if let Some(active) = &self.loose_active {
            lines.push(riga(SCIOLTO_ATTIVO, active));
        }
        for project in &self.projects {
            lines.push(riga(PROGETTO, &project.root));
            if let Some([red, green, blue, alpha]) = project.color {
                lines.push(format!(
                    "{COLORE_PROGETTO}{red:02x}{green:02x}{blue:02x}{alpha:02x}"
                ));
            }
            for file in &project.files {
                lines.push(riga(FILE, file));
            }
            for folder in &project.expanded {
                lines.push(riga(ESPANSO, folder));
            }
            if let Some(active) = &project.active {
                lines.push(riga(ATTIVO, active));
            }
            if let Some(selected) = &project.selected {
                lines.push(riga(SELEZIONATO, selected));
            }
        }
        if let Some(root) = &self.active_project {
            lines.push(riga(PROGETTO_ATTIVO, root));
        }
        lines.join("\n")
    }

    /// Rilegge ciò che ha scritto [`Self::encode`].
    ///
    /// Le righe che non si capiscono vengono ignorate: una sessione scritta da
    /// una versione più nuova (o rovinata) non deve impedire l'avvio.
    pub fn decode(text: &str) -> Session {
        let mut session = Session::default();
        for line in text.lines() {
            let mut chars = line.chars();
            let Some(tag) = chars.next() else {
                continue;
            };
            let path = leggi(chars.as_str());
            match tag {
                PROGETTO => session.projects.push(ProjectSession {
                    root: path,
                    color: None,
                    files: Vec::new(),
                    active: None,
                    expanded: Vec::new(),
                    selected: None,
                }),
                COLORE_PROGETTO => {
                    if let Some(project) = session.projects.last_mut() {
                        if let Some(color) = decode_color(chars.as_str()) {
                            project.color = Some(color);
                        }
                    }
                }
                FILE => {
                    if let Some(project) = session.projects.last_mut() {
                        project.files.push(path);
                    }
                }
                ESPANSO => {
                    if let Some(project) = session.projects.last_mut() {
                        project.expanded.push(path);
                    }
                }
                ATTIVO => {
                    if let Some(project) = session.projects.last_mut() {
                        project.active = Some(path);
                    }
                }
                SELEZIONATO => {
                    if let Some(project) = session.projects.last_mut() {
                        project.selected = Some(path);
                    }
                }
                SCIOLTO => session.loose_files.push(path),
                SCIOLTO_ATTIVO => session.loose_active = Some(path),
                PROGETTO_ATTIVO => session.active_project = Some(path),
                _ => {}
            }
        }
        session
    }

    /// Riapre progetti e file in `workspace`.
    ///
    /// I percorsi che non esistono più (o che non si riescono ad aprire) vengono
    /// saltati: ritorna quanti ne sono rimasti fuori.
    pub fn restore(&self, workspace: &mut Workspace) -> usize {
        let mut failed = 0;
        if self.projects.is_empty() {
            // Senza progetti i file stanno in un gruppo a parte: vanno riaperti
            // adesso, prima che un progetto se li porti via.
            for file in &self.loose_files {
                if workspace.open_path(file).is_err() {
                    failed += 1;
                }
            }
            if let Some(active) = &self.loose_active {
                if let Ok(id) = workspace.open_path(active) {
                    workspace.set_active(id);
                }
            }
            return failed;
        }
        for project in &self.projects {
            if workspace.open_project(&project.root).is_err() {
                failed += 1;
            }
        }
        // I file si aprono un progetto alla volta: così ognuno finisce fra le
        // tab della cartella che lo conteneva.
        for project in &self.projects {
            let Some(index) = workspace.index_of_project(&project.root) else {
                // La cartella non si è aperta (sparita?): con lei se ne vanno i suoi file.
                failed += project.files.len();
                continue;
            };
            workspace.activate_project(index);
            for file in &project.files {
                if workspace.open_path(file).is_err() {
                    failed += 1;
                }
            }
            if let Some(active) = &project.active {
                if let Ok(id) = workspace.open_path(active) {
                    workspace.set_active(id);
                }
            }
        }
        if let Some(root) = &self.active_project {
            if let Some(index) = workspace.index_of_project(root) {
                workspace.activate_project(index);
            }
        }
        failed
    }
}

impl Workspace {
    /// La sessione da ritrovare al prossimo avvio.
    ///
    /// I documenti nuovi (senza percorso) non ci sono: al prossimo avvio non
    /// avrebbero dove tornare. Lo stato dell'albero (`expanded`, `selected`)
    /// resta vuoto: lo riempie chi disegna l'albero, che è l'unico a saperlo.
    pub fn session(&self) -> Session {
        let projects = self
            .projects()
            .enumerate()
            .map(|(index, project)| {
                let (files, active) = self.project_files(index);
                ProjectSession {
                    root: project.root().to_path_buf(),
                    color: None,
                    files,
                    active,
                    expanded: Vec::new(),
                    selected: None,
                }
            })
            .collect();
        let (loose_files, loose_active) = self.loose_files();
        Session {
            projects,
            active_project: self.root().map(|r| r.to_path_buf()),
            loose_files,
            loose_active,
        }
    }
}

/// Una riga del formato: l'etichetta e il percorso protetto.
fn riga(tag: char, path: &Path) -> String {
    let mut out = String::with_capacity(path.as_os_str().len() + 1);
    out.push(tag);
    for c in path.to_string_lossy().chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            other => out.push(other),
        }
    }
    out
}

/// L'inverso di [`riga`]: da `\n`, `\r`, `\\` al carattere che rappresentano.
fn leggi(text: &str) -> PathBuf {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('\\') => out.push('\\'),
            // Una barra rovesciata seguita da altro resta com'era.
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    PathBuf::from(out)
}

fn decode_color(value: &str) -> Option<[u8; 4]> {
    if value.len() != 8 || !value.is_ascii() {
        return None;
    }
    let mut color = [0; 4];
    for (index, channel) in color.iter_mut().enumerate() {
        *channel = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16).ok()?;
    }
    Some(color)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_dir(nome: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ride-session-{}-{nome}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Un progetto temporaneo con i file indicati, e il suo percorso canonico.
    fn progetto(nome: &str, file: &[&str]) -> (PathBuf, PathBuf) {
        let dir = temp_dir(nome);
        let root = fs::canonicalize(&dir).unwrap();
        for nome_file in file {
            fs::write(root.join(nome_file), nome_file).unwrap();
        }
        (dir, root)
    }

    /// Percorso del documento in primo piano (leggerli è più chiaro che
    /// confrontare gli id, che cambiano da un workspace all'altro).
    fn percorso_attivo(ws: &Workspace) -> Option<PathBuf> {
        ws.active().and_then(|doc| doc.path().map(|p| p.to_path_buf()))
    }

    #[test]
    fn la_sessione_si_scrive_e_si_rilegge() {
        let session = Session {
            projects: vec![
                ProjectSession {
                    root: PathBuf::from("/p/a"),
                    color: None,
                    files: vec![PathBuf::from("/p/a/x.rs"), PathBuf::from("/p/a/y.rs")],
                    active: Some(PathBuf::from("/p/a/y.rs")),
                    expanded: vec![PathBuf::from("/p/a"), PathBuf::from("/p/a/src")],
                    selected: Some(PathBuf::from("/p/a/x.rs")),
                },
                ProjectSession {
                    root: PathBuf::from("/p/b"),
                    color: None,
                    files: Vec::new(),
                    active: None,
                    expanded: Vec::new(),
                    selected: None,
                },
            ],
            active_project: Some(PathBuf::from("/p/a")),
            loose_files: Vec::new(),
            loose_active: None,
        };
        assert_eq!(Session::decode(&session.encode()), session);
        // Le righe restano leggibili a mano: una per voce (due progetti, due
        // file, due cartelle espanse, la tab attiva, la voce scelta, l'attivo).
        assert_eq!(session.encode().lines().count(), 9, "{}", session.encode());
        // E senza niente da salvare non si scrive nemmeno una riga.
        assert!(Session::default().is_empty());
        assert_eq!(Session::default().encode(), "");
        assert_eq!(Session::decode(""), Session::default());
    }

    #[test]
    fn il_colore_del_progetto_si_salva_e_si_rilegge() {
        let session = Session {
            projects: vec![ProjectSession {
                root: PathBuf::from("/p/colorato"),
                color: Some([0x12, 0xab, 0xff, 0xff]),
                files: Vec::new(),
                active: None,
                expanded: Vec::new(),
                selected: None,
            }],
            ..Session::default()
        };
        assert_eq!(Session::decode(&session.encode()), session);
        assert_eq!(
            Session::decode("P/p\nC12abffff").projects[0].color,
            Some([0x12, 0xab, 0xff, 0xff])
        );
        assert_eq!(Session::decode("P/p\nCnotacol").projects[0].color, None);
    }

    #[test]
    fn percorsi_bislacchi_non_rompono_la_sessione() {
        // Un a capo o una barra rovesciata in un nome di file sono legali su
        // Linux: non devono spezzare le righe del formato.
        let session = Session {
            projects: vec![ProjectSession {
                root: PathBuf::from("/p/a\nb"),
                color: None,
                files: vec![PathBuf::from("/p/a\\c.txt"), PathBuf::from("/p/d\re")],
                active: Some(PathBuf::from("/p/a\\c.txt")),
                expanded: vec![PathBuf::from("/p/a\nb/src")],
                selected: Some(PathBuf::from("/p/d\re")),
            }],
            active_project: Some(PathBuf::from("/p/a\nb")),
            loose_files: Vec::new(),
            loose_active: None,
        };
        assert_eq!(Session::decode(&session.encode()), session);
    }

    #[test]
    fn righe_che_non_si_capiscono_vengono_ignorate() {
        let session = Session::decode("Xstrano\nFsenza-progetto\nP/p\nF/p/x\n?boh");
        assert_eq!(
            session.projects,
            vec![ProjectSession {
                root: PathBuf::from("/p"),
                color: None,
                files: vec![PathBuf::from("/p/x")],
                active: None,
                expanded: Vec::new(),
                selected: None,
            }]
        );
        // La `F` prima di ogni `P` non ha un progetto a cui appartenere.
        assert!(session.loose_files.is_empty());
    }

    #[test]
    fn espansioni_e_selezione_senza_progetto_vengono_ignorate() {
        // `E` e `V` descrivono l'albero di un progetto: senza un `P` prima non
        // si sa a chi appartengano.
        let session = Session::decode("E/p/espansa\nV/p/scelta\nP/p\nE/p/dentro");
        assert_eq!(session.projects.len(), 1);
        assert_eq!(session.projects[0].expanded, vec![PathBuf::from("/p/dentro")]);
        assert!(session.projects[0].selected.is_none());
    }

    #[test]
    fn la_sessione_riapre_progetti_e_file() {
        let (a, ra) = progetto("riapri-a", &["a1.rs", "a2.rs"]);
        let (b, rb) = progetto("riapri-b", &["b1.rs"]);

        let mut ws = Workspace::new();
        ws.open_project(&a).unwrap();
        ws.open_path(ra.join("a1.rs")).unwrap();
        ws.open_path(ra.join("a2.rs")).unwrap();
        ws.open_project(&b).unwrap();
        ws.open_path(rb.join("b1.rs")).unwrap();
        // Si chiude tornando al primo progetto, per vedere che anche l'attivo
        // viene ritrovato.
        ws.activate_project(0);
        let prima = ws.docs().next().unwrap().0;
        ws.set_active(prima);

        let session = ws.session();
        assert_eq!(session.projects.len(), 2);
        assert_eq!(session.active_project.as_deref(), Some(ra.as_path()));

        let mut nuovo = Workspace::new();
        assert_eq!(session.restore(&mut nuovo), 0);
        assert_eq!(nuovo.project_count(), 2);
        assert_eq!(nuovo.active_project_index(), Some(0));
        assert_eq!(nuovo.root(), Some(ra.as_path()));
        assert_eq!(nuovo.len(), 2);
        assert_eq!(nuovo.all_docs().count(), 3);
        assert_eq!(percorso_attivo(&nuovo), Some(ra.join("a1.rs")));

        // L'altro progetto ha la sua tab in primo piano.
        nuovo.activate_project(1);
        assert_eq!(nuovo.len(), 1);
        assert_eq!(percorso_attivo(&nuovo), Some(rb.join("b1.rs")));

        let _ = (fs::remove_dir_all(&a), fs::remove_dir_all(&b));
    }

    #[test]
    fn la_sessione_riapre_i_file_senza_progetto() {
        let (a, ra) = progetto("riapri-sciolti", &["x.rs", "y.rs"]);

        let mut ws = Workspace::new();
        ws.open_path(ra.join("x.rs")).unwrap();
        ws.open_path(ra.join("y.rs")).unwrap();

        let session = ws.session();
        // Nessun progetto aperto: i file sono "sciolti".
        assert!(session.projects.is_empty());
        assert_eq!(session.loose_files.len(), 2);
        assert!(session.active_project.is_none());

        let mut nuovo = Workspace::new();
        assert_eq!(session.restore(&mut nuovo), 0);
        assert_eq!(nuovo.project_count(), 0);
        assert_eq!(nuovo.len(), 2);
        assert_eq!(percorso_attivo(&nuovo), Some(ra.join("y.rs")));

        let _ = fs::remove_dir_all(&a);
    }

    #[test]
    fn i_file_spariti_si_contano_e_non_fermano_il_ripristino() {
        let (a, ra) = progetto("riapri-mancanti", &["c1.rs", "c2.rs"]);

        let mut ws = Workspace::new();
        ws.open_project(&a).unwrap();
        ws.open_path(ra.join("c1.rs")).unwrap();
        ws.open_path(ra.join("c2.rs")).unwrap();
        let session = ws.session();

        // Il progetto resta, ma uno dei due file sparisce.
        fs::remove_file(ra.join("c2.rs")).unwrap();
        let mut nuovo = Workspace::new();
        assert_eq!(session.restore(&mut nuovo), 1);
        assert_eq!(nuovo.project_count(), 1);
        assert_eq!(nuovo.len(), 1);
        assert_eq!(percorso_attivo(&nuovo), Some(ra.join("c1.rs")));

        // Se sparisce la cartella intera si salta tutto senza panico.
        fs::remove_dir_all(&a).unwrap();
        let mut vuoto = Workspace::new();
        assert_eq!(session.restore(&mut vuoto), 3);
        assert_eq!(vuoto.project_count(), 0);
        assert!(vuoto.is_empty());
    }
}
