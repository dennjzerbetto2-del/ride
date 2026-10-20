//! Azioni dell'editor e scorciatoie da tastiera.
//!
//! Le azioni sono l'unico modo in cui menu, tastiera e pulsanti chiedono
//! qualcosa all'applicazione: così ogni comando è definito una volta sola e la
//! tabella delle scorciatoie è anche la finestra di aiuto (vedi [`SHORTCUTS`]).

use std::path::PathBuf;

use egui::{Context, Key, KeyboardShortcut, Modifiers};

/// Un comando dell'editor.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    // File e progetto
    NewFile,
    OpenFileDialog,
    OpenFolderDialog,
    OpenPath(PathBuf),
    OpenFolder(PathBuf),
    /// Passa al progetto aperto in posizione `index` (le "linguette" a sinistra).
    SwitchProject(usize),
    CloseProject,
    Save,
    SaveAs,
    SaveAll,
    ReloadFile,
    CloseTab,
    CloseTabAt(usize),
    CloseOtherTabs,
    CloseAllTabs,
    Quit,

    // Navigazione
    NextTab,
    PreviousTab,
    QuickOpen,
    GotoLine,
    RevealActiveFile,

    // Ricerca
    Find,
    Replace,
    /// Sostituisce la corrispondenza attiva.
    ReplaceCurrent,
    /// Sostituisce tutte le corrispondenze.
    ReplaceAll,
    FindNext,
    FindPrevious,
    SearchInProject,
    Cancel,

    // Modifica
    ToggleComment,
    Indent,
    Dedent,
    DuplicateLines,
    MoveLinesUp,
    MoveLinesDown,
    DeleteLines,
    FormatDocument,

    // Build (Maven / Gradle)
    /// Esegue il comando di build `index` dello strumento indicato
    /// (indice in [`ride_core::build::commands`]).
    RunBuild(ride_core::build::BuildTool, usize),
    /// Esegue gli argomenti indicati (un goal di plugin e le sue opzioni, es.
    /// `dependency:tree -Dverbose`).
    RunBuildArgs(ride_core::build::BuildTool, String),
    /// Chiede quali argomenti passare al sistema di build.
    AskBuildArgs(ride_core::build::BuildTool),
    /// Apre la console dei comandi di build.
    ShowBuildConsole,
    /// Interrompe il comando di build in corso.
    StopBuild,

    // Terminale
    /// Mostra o nasconde il terminale integrato.
    ToggleTerminal,
    /// Nasconde il terminale lasciando vivo lo shell.
    HideTerminal,
    /// Riavvia lo shell del terminale.
    RestartTerminal,
    /// Attiva o disattiva l'autoscomparsa del terminale.
    ToggleTerminalAutoHide,

    // Vista
    ToggleTree,
    ToggleSearchPanel,
    ToggleHighlight,
    ToggleLineNumbers,
    ToggleAutoBrackets,
    ToggleBracketMatch,
    ToggleFolding,
    NextSyntaxTheme,
    NextTheme,
    ZoomIn,
    ZoomOut,
    ZoomReset,
    RefreshTree,

    // Informazioni e LSP
    ShowShortcuts,
    ShowAbout,
    ShowLspInfo,
    /// Mostra il popup dei problemi segnalati dal language server.
    ShowProblems,
    /// Porta il cursore sul problema che inizia al carattere indicato.
    GotoDiagnostic(usize),
    /// Riavvia il language server del linguaggio attivo.
    RestartLsp,
    /// Attiva o disattiva del tutto il language server.
    ToggleLsp,
    /// Vai alla definizione del simbolo sotto il cursore (F12).
    GotoDefinition,

    /// Attiva la tab indicata.
    ActivateTab(ride_core::DocumentId),
    /// Sposta la tab indicata alla posizione indicata (drag & drop).
    MoveTab(ride_core::DocumentId, usize),

    /// Elimina un file o una cartella.
    DeletePath(PathBuf),
    /// Vai alla riga indicata (1-based).
    GotoLineAt(usize),
    /// Esito del dialogo sui documenti modificati.
    ResolveClose(CloseChoice),
}

/// Cosa fare dei documenti modificati quando si chiude.
#[derive(Debug, Clone, PartialEq)]
pub struct CloseChoice {
    pub docs: Vec<ride_core::DocumentId>,
    pub quit: bool,
    pub save: bool,
    /// Progetto da chiudere dopo i documenti, se c'era.
    pub close_project: Option<std::path::PathBuf>,
}

/// Una scorciatoia con la sua descrizione (per la finestra di aiuto).
pub struct Shortcut {
    pub keys: &'static str,
    pub description: &'static str,
}

/// Tabella delle scorciatoie: **è** la documentazione dei comandi.
pub const SHORTCUTS: &[Shortcut] = &[
    Shortcut { keys: "Ctrl+N", description: "Nuovo file" },
    Shortcut { keys: "Ctrl+O", description: "Apri file…" },
    Shortcut { keys: "Ctrl+Shift+O", description: "Apri cartella come progetto…" },
    Shortcut { keys: "Ctrl+S", description: "Salva" },
    Shortcut { keys: "Ctrl+Shift+S", description: "Salva con nome…" },
    Shortcut { keys: "Ctrl+Alt+S", description: "Salva tutto" },
    Shortcut { keys: "Ctrl+R", description: "Ricarica dal disco" },
    Shortcut { keys: "Ctrl+W", description: "Chiudi la tab" },
    Shortcut { keys: "Ctrl+Shift+W", description: "Chiudi tutte le tab" },
    Shortcut { keys: "Ctrl+Tab", description: "Tab successiva" },
    Shortcut { keys: "Ctrl+Shift+Tab", description: "Tab precedente" },
    Shortcut { keys: "Ctrl+P", description: "Vai al file (apertura rapida)" },
    Shortcut { keys: "Ctrl+G", description: "Vai alla riga…" },
    Shortcut { keys: "Ctrl+Shift+E", description: "Mostra il file attivo nell'albero" },
    Shortcut { keys: "Ctrl+F", description: "Trova nel file" },
    Shortcut { keys: "Ctrl+H", description: "Trova e sostituisci" },
    Shortcut { keys: "F3", description: "Corrispondenza successiva" },
    Shortcut { keys: "Shift+F3", description: "Corrispondenza precedente" },
    Shortcut { keys: "Ctrl+Shift+F", description: "Cerca nel progetto" },
    Shortcut { keys: "Esc", description: "Chiudi la ricerca o il dialogo" },
    Shortcut { keys: "Ctrl+/", description: "Commenta o scommenta le righe" },
    Shortcut { keys: "Ctrl+]", description: "Indenta la selezione" },
    Shortcut { keys: "Ctrl+[", description: "Disindenta la selezione" },
    Shortcut { keys: "Ctrl+Shift+D", description: "Duplica la riga" },
    Shortcut { keys: "Alt+↑ / Alt+↓", description: "Sposta la riga" },
    Shortcut { keys: "Ctrl+Shift+K", description: "Cancella la riga" },
    Shortcut { keys: "Ctrl+Shift+I", description: "Formatta il documento" },
    Shortcut { keys: "F12", description: "Vai alla definizione (LSP)" },
    Shortcut { keys: "Ctrl+B", description: "Mostra/nascondi il pannello del progetto" },
    Shortcut { keys: "Ctrl+`", description: "Mostra/nascondi il terminale" },
    Shortcut { keys: "F5", description: "Ricarica l'albero del progetto" },
    Shortcut { keys: "Ctrl+ +", description: "Ingrandisci il codice" },
    Shortcut { keys: "Ctrl+-", description: "Rimpicciolisci il codice" },
    Shortcut { keys: "Ctrl+0", description: "Dimensione predefinita" },
    Shortcut { keys: "F1", description: "Elenco delle scorciatoie" },
    Shortcut { keys: "Ctrl+Q", description: "Esci" },
];

fn shortcut(modifiers: Modifiers, key: Key) -> KeyboardShortcut {
    KeyboardShortcut::new(modifiers, key)
}

impl Action {
    /// Raccoglie le azioni richieste dai tasti premuti in questo frame.
    ///
    /// L'ordine conta: `consume_shortcut` ignora `Shift` ed `Alt` in più, quindi
    /// le combinazioni più specifiche devono essere controllate per prime
    /// (altrimenti `Ctrl+Shift+S` verrebbe mangiato da `Ctrl+S`).
    pub fn collect(ctx: &Context) -> Vec<Action> {
        use Modifiers as M;
        let mut actions = Vec::new();
        ctx.input_mut(|input| {
            let mut check = |actions: &mut Vec<Action>, modifiers, key, action| {
                if input.consume_shortcut(&shortcut(modifiers, key)) {
                    actions.push(action);
                }
            };

            // Prima quelle con Shift/Alt.
            check(&mut actions, M::COMMAND | M::SHIFT, Key::S, Action::SaveAs);
            check(&mut actions, M::COMMAND | M::SHIFT, Key::O, Action::OpenFolderDialog);
            check(&mut actions, M::COMMAND | M::SHIFT, Key::F, Action::SearchInProject);
            check(&mut actions, M::COMMAND | M::SHIFT, Key::I, Action::FormatDocument);
            check(&mut actions, M::COMMAND | M::SHIFT, Key::W, Action::CloseAllTabs);
            check(&mut actions, M::COMMAND | M::SHIFT, Key::D, Action::DuplicateLines);
            check(&mut actions, M::COMMAND | M::SHIFT, Key::K, Action::DeleteLines);
            check(&mut actions, M::COMMAND | M::SHIFT, Key::E, Action::RevealActiveFile);
            check(&mut actions, M::COMMAND | M::SHIFT, Key::H, Action::ToggleHighlight);
            check(&mut actions, M::COMMAND | M::SHIFT, Key::L, Action::ToggleLineNumbers);
            check(&mut actions, M::COMMAND | M::SHIFT, Key::Tab, Action::PreviousTab);
            check(&mut actions, M::SHIFT, Key::F3, Action::FindPrevious);
            check(&mut actions, M::ALT, Key::ArrowUp, Action::MoveLinesUp);
            check(&mut actions, M::ALT, Key::ArrowDown, Action::MoveLinesDown);
            check(&mut actions, M::COMMAND | M::ALT, Key::S, Action::SaveAll);

            // Poi quelle semplici.
            check(&mut actions, M::COMMAND, Key::N, Action::NewFile);
            check(&mut actions, M::COMMAND, Key::O, Action::OpenFileDialog);
            check(&mut actions, M::COMMAND, Key::S, Action::Save);
            check(&mut actions, M::COMMAND, Key::R, Action::ReloadFile);
            check(&mut actions, M::COMMAND, Key::W, Action::CloseTab);
            check(&mut actions, M::COMMAND, Key::P, Action::QuickOpen);
            check(&mut actions, M::COMMAND, Key::G, Action::GotoLine);
            check(&mut actions, M::COMMAND, Key::F, Action::Find);
            check(&mut actions, M::COMMAND, Key::H, Action::Replace);
            check(&mut actions, M::COMMAND, Key::B, Action::ToggleTree);
            check(&mut actions, M::COMMAND, Key::Backtick, Action::ToggleTerminal);
            check(&mut actions, M::COMMAND, Key::Q, Action::Quit);
            check(&mut actions, M::COMMAND, Key::Slash, Action::ToggleComment);
            check(&mut actions, M::COMMAND, Key::CloseBracket, Action::Indent);
            check(&mut actions, M::COMMAND, Key::OpenBracket, Action::Dedent);
            check(&mut actions, M::COMMAND, Key::Equals, Action::ZoomIn);
            check(&mut actions, M::COMMAND, Key::Plus, Action::ZoomIn);
            check(&mut actions, M::COMMAND, Key::Minus, Action::ZoomOut);
            check(&mut actions, M::COMMAND, Key::Num0, Action::ZoomReset);
            check(&mut actions, M::COMMAND, Key::Tab, Action::NextTab);
            check(&mut actions, M::NONE, Key::F3, Action::FindNext);
            check(&mut actions, M::NONE, Key::F12, Action::GotoDefinition);
            check(&mut actions, M::NONE, Key::F5, Action::RefreshTree);
            check(&mut actions, M::NONE, Key::F1, Action::ShowShortcuts);
            check(&mut actions, M::NONE, Key::Escape, Action::Cancel);
        });
        actions
    }

    /// Scorciatoie consentite mentre il terminale ha il fuoco.
    ///
    /// Solo `Ctrl+` resta un comando dell'editor: **non** si devono consumare
    /// gli altri tasti (Ctrl+C, Ctrl+W, …) perché appartengono alla shell.
    pub fn collect_terminal(ctx: &Context) -> Vec<Action> {
        use Modifiers as M;
        let mut actions = Vec::new();
        ctx.input_mut(|input| {
            if input.consume_shortcut(&shortcut(M::COMMAND, Key::Backtick)) {
                actions.push(Action::ToggleTerminal);
            }
        });
        actions
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::Context;

    /// Simula un frame con un tasto premuto e raccoglie le azioni.
    fn press(modifiers: Modifiers, key: Key) -> Vec<Action> {
        let ctx = Context::default();
        ctx.begin_pass(egui::RawInput {
            events: vec![egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers,
            }],
            ..Default::default()
        });
        Action::collect(&ctx)
    }

    fn ctrl() -> Modifiers {
        Modifiers::COMMAND
    }

    /// Simula un frame con un tasto premuto e raccoglie le azioni consentite
    /// mentre il terminale ha il fuoco.
    fn press_in_terminal(modifiers: Modifiers, key: Key) -> Vec<Action> {
        let ctx = Context::default();
        ctx.begin_pass(egui::RawInput {
            events: vec![egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers,
            }],
            ..Default::default()
        });
        Action::collect_terminal(&ctx)
    }

    #[test]
    fn salva_e_salva_con_nome_non_si_confondono() {
        assert_eq!(press(ctrl(), Key::S), vec![Action::Save]);
        assert_eq!(
            press(ctrl() | Modifiers::SHIFT, Key::S),
            vec![Action::SaveAs]
        );
    }

    #[test]
    fn cerca_e_cerca_nel_progetto() {
        assert_eq!(press(ctrl(), Key::F), vec![Action::Find]);
        assert_eq!(
            press(ctrl() | Modifiers::SHIFT, Key::F),
            vec![Action::SearchInProject]
        );
    }

    #[test]
    fn un_tasto_senza_scorciatoia_non_produce_azioni() {
        assert!(press(ctrl(), Key::Y).is_empty());
    }

    #[test]
    fn con_il_fuoco_al_terminale_solo_ctrl_backtick_resta_un_comando() {
        assert_eq!(
            press_in_terminal(ctrl(), Key::Backtick),
            vec![Action::ToggleTerminal]
        );
        // Ctrl+W chiude la tab nell'editor, ma nel terminale è della shell.
        assert!(press_in_terminal(ctrl(), Key::W).is_empty());
        assert!(press_in_terminal(ctrl(), Key::C).is_empty());
    }

    #[test]
    fn la_tabella_delle_scorciatoie_e_coerente() {
        // Nessuna scorciatoia duplicata nella tabella di aiuto.
        let mut keys: Vec<&str> = SHORTCUTS.iter().map(|s| s.keys).collect();
        keys.sort_unstable();
        let count = keys.len();
        keys.dedup();
        assert_eq!(keys.len(), count);
    }
}
