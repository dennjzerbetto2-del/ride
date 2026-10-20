//! Terminale integrato: processo su un PTY, emulazione VT100 e stato del
//! pannello.
//!
//! Il terminale è **agganciato alla finestra** (un pannello in basso), ma può
//! nascondersi da solo: quando perde il fuoco e il puntatore non è più sopra di
//! lui, dopo un breve intervallo sparisce. Il tasto `Ctrl+` lo fa ricomparire.
//!
//! Il processo vero e proprio vive su un thread lettore; la GUI riceve i byte
//! già pronti e li passa all'emulatore [`vt100`]. Come per la console di build,
//! niente operazioni bloccanti nel thread dell'interfaccia.

use std::io::{Read, Write};
use std::num::NonZeroU16;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use egui::{Context, Key, Modifiers};
use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};

/// Righe di cronologia conservate per lo scorrimento all'indietro.
const SCROLLBACK_LINES: usize = 5_000;
/// Quanto resta visibile il terminale dopo che ha perso il fuoco.
const AUTO_HIDE_DELAY: Duration = Duration::from_millis(1_200);

/// Un evento prodotto dal processo del terminale.
#[derive(Debug)]
enum TerminalEvent {
    /// Byte grezzi (sequenze ANSI comprese).
    Output(Vec<u8>),
    /// Il processo è terminato, con una descrizione leggibile.
    Exited(String),
}

/// Il processo e i canali verso i suoi thread.
struct Process {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    killer: Box<dyn ChildKiller + Send + Sync>,
    events: Receiver<TerminalEvent>,
}

/// Stato del terminale integrato.
///
/// Non fa parte del modello (`ride-core`): è un servizio dell'interfaccia, come
/// la console di build.
pub struct TerminalState {
    /// Il pannello è visibile.
    pub open: bool,
    /// Il pannello si nasconde da solo quando perde il fuoco.
    pub auto_hide: bool,
    /// Il terminale ha il fuoco della tastiera (aggiornato a ogni frame).
    pub focused: bool,
    /// Cartella di lavoro del processo.
    cwd: PathBuf,
    /// Nome dello shell in esecuzione (per l'intestazione).
    shell: String,
    /// Emulatore VT100.
    parser: vt100::Parser,
    process: Option<Process>,
    /// Errore di avvio del processo.
    error: Option<String>,
    /// Messaggio di uscita dell'ultimo processo.
    exit: Option<String>,
    /// L'uscita è già stata mostrata nella barra di stato.
    exit_reported: bool,
    /// Righe di scorrimento all'indietro (`0` = fondo).
    scroll_offset: usize,
    /// Dimensioni correnti in celle.
    rows: u16,
    cols: u16,
    /// Il prossimo frame deve dare il fuoco al terminale.
    focus_requested: bool,
    /// Quando il terminale ha perso il fuoco (per l'autoscomparsa).
    blur_since: Option<Instant>,
    /// Contesto per risvegliare la GUI quando arriva output.
    repaint: Option<Context>,
}

impl Default for TerminalState {
    fn default() -> Self {
        Self::new()
    }
}

impl TerminalState {
    pub fn new() -> Self {
        TerminalState {
            open: false,
            auto_hide: true,
            focused: false,
            cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            shell: String::new(),
            parser: Self::parser(24, 80),
            process: None,
            error: None,
            exit: None,
            exit_reported: false,
            scroll_offset: 0,
            rows: 24,
            cols: 80,
            focus_requested: false,
            blur_since: None,
            repaint: None,
        }
    }

    fn parser(rows: u16, cols: u16) -> vt100::Parser {
        let rows = NonZeroU16::new(rows.max(1)).unwrap_or(NonZeroU16::MIN);
        let cols = NonZeroU16::new(cols.max(1)).unwrap_or(NonZeroU16::MIN);
        vt100::Parser::new(rows, cols, SCROLLBACK_LINES)
    }

    /// Il contesto egui con cui risvegliare l'interfaccia all'arrivo di output.
    pub fn set_repaint_context(&mut self, ctx: Context) {
        self.repaint = Some(ctx);
    }

    /// `true` se il processo è ancora vivo.
    pub fn is_running(&self) -> bool {
        self.process.is_some() && self.exit.is_none()
    }

    /// `true` se esiste un terminale da mostrare (anche se il pannello è chiuso).
    pub fn has_session(&self) -> bool {
        self.process.is_some() || self.exit.is_some() || self.error.is_some()
    }

    pub fn cwd(&self) -> &Path {
        &self.cwd
    }

    pub fn shell(&self) -> &str {
        &self.shell
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn exit_message(&self) -> Option<&str> {
        self.exit.as_deref()
    }

    pub fn screen(&self) -> &vt100::Screen {
        self.parser.screen()
    }

    /// Offset dello scorrimento all'indietro (`0` = fondo).
    pub fn scroll_offset(&self) -> usize {
        self.scroll_offset
    }

    /// Apre il pannello (avviando lo shell se non c'è ancora).
    pub fn open(&mut self, root: Option<&Path>) {
        self.open = true;
        self.focus_requested = true;
        self.blur_since = None;
        if self.process.is_none() && self.exit.is_none() && self.error.is_none() {
            self.start(root);
        }
    }

    /// Nasconde il pannello lasciando vivo il processo.
    pub fn close(&mut self) {
        self.open = false;
        self.focused = false;
        self.blur_since = None;
    }

    /// Riavvia lo shell (nella cartella indicata).
    pub fn restart(&mut self, root: Option<&Path>) {
        self.stop();
        self.parser = Self::parser(self.rows, self.cols);
        self.error = None;
        self.exit = None;
        self.exit_reported = false;
        self.scroll_offset = 0;
        self.open = true;
        self.focus_requested = true;
        self.start(root);
    }

    /// Avvia lo shell nella radice indicata (o nella cartella corrente).
    fn start(&mut self, root: Option<&Path>) {
        let cwd = root
            .map(Path::to_path_buf)
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        let (program, args) = shell_command();
        self.shell = Path::new(&program)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| program.clone());
        match spawn(
            &program,
            &args,
            &cwd,
            self.rows,
            self.cols,
            self.repaint.clone(),
        ) {
            Ok(process) => {
                self.cwd = cwd;
                self.process = Some(process);
                self.error = None;
                self.exit = None;
                self.exit_reported = false;
                self.scroll_offset = 0;
            }
            Err(e) => {
                self.cwd = cwd;
                self.process = None;
                self.error = Some(e);
            }
        }
    }

    /// Termina il processo, se c'è.
    pub fn stop(&mut self) {
        if let Some(mut process) = self.process.take() {
            let _ = process.killer.kill();
        }
        self.focused = false;
        self.blur_since = None;
    }

    /// Chiude il terminale e termina lo shell (all'uscita dall'editor).
    pub fn shutdown(&mut self) {
        self.stop();
        self.open = false;
    }

    /// Raccoglie l'output pronto e aggiorna l'emulatore.
    ///
    /// Ritorna `true` se qualcosa è cambiato (per chiedere un repaint).
    pub fn poll(&mut self) -> bool {
        let mut chunks = Vec::new();
        let mut exit = None;
        let mut disconnected = false;
        if let Some(process) = &self.process {
            loop {
                match process.events.try_recv() {
                    Ok(TerminalEvent::Output(bytes)) => chunks.push(bytes),
                    Ok(TerminalEvent::Exited(message)) => exit = Some(message),
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        disconnected = true;
                        break;
                    }
                }
            }
        }

        let changed = !chunks.is_empty() || exit.is_some() || disconnected;
        for bytes in chunks {
            self.parser.process(&bytes);
        }
        if let Some(message) = exit {
            self.exit = Some(message);
        }
        if disconnected {
            self.process = None;
        }
        changed
    }

    /// Messaggio di uscita da mostrare una sola volta nella barra di stato.
    pub fn take_exit_notice(&mut self) -> Option<String> {
        if self.exit.is_some() && !self.exit_reported {
            self.exit_reported = true;
            return self.exit.clone();
        }
        None
    }

    /// Adatta l'emulatore e il PTY alle dimensioni del pannello.
    pub fn resize(&mut self, rows: u16, cols: u16) {
        let rows = rows.max(1);
        let cols = cols.max(1);
        if rows == self.rows && cols == self.cols {
            return;
        }
        self.rows = rows;
        self.cols = cols;
        let (Some(rows_nz), Some(cols_nz)) = (NonZeroU16::new(rows), NonZeroU16::new(cols)) else {
            return;
        };
        self.parser.set_size(rows_nz, cols_nz);
        if let Some(process) = &self.process {
            let _ = process.master.resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            });
        }
        self.scroll_offset = 0;
        self.parser.screen_mut().set_scrollback(0);
    }

    /// Invia byte allo shell (tasti, incolla, …).
    pub fn send_bytes(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        self.scroll_to_bottom();
        if let Some(process) = &mut self.process {
            if let Err(e) = process
                .writer
                .write_all(bytes)
                .and_then(|_| process.writer.flush())
            {
                self.error = Some(format!("impossibile scrivere nel terminale: {e}"));
            }
        }
    }

    /// Inserisce del testo (appunti), rispettando il *bracketed paste* se il
    /// programma lo ha attivato.
    pub fn paste(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        if self.parser.screen().bracketed_paste() {
            let mut bytes = b"\x1b[200~".to_vec();
            bytes.extend_from_slice(text.as_bytes());
            bytes.extend_from_slice(b"\x1b[201~");
            self.send_bytes(&bytes);
        } else {
            let text = text.replace("\r\n", "\n").replace('\n', "\r");
            self.send_bytes(text.as_bytes());
        }
    }

    /// Scorre la cronologia di `delta` righe (positivo = indietro).
    pub fn scroll(&mut self, delta: i32) {
        let max = self.parser.screen().scrollback_len() as i32;
        let offset = (self.scroll_offset as i32 + delta).clamp(0, max);
        self.scroll_offset = offset as usize;
        self.parser.screen_mut().set_scrollback(self.scroll_offset);
    }

    /// Torna al fondo (l'utente sta digitando).
    pub fn scroll_to_bottom(&mut self) {
        if self.scroll_offset != 0 {
            self.scroll_offset = 0;
            self.parser.screen_mut().set_scrollback(0);
        }
    }

    /// Il prossimo frame deve dare il fuoco al terminale.
    pub fn take_focus_request(&mut self) -> bool {
        std::mem::take(&mut self.focus_requested)
    }

    /// Registra l'esito del fuoco in questo frame.
    pub fn set_focused(&mut self, focused: bool, now: Instant) {
        if focused {
            self.focused = true;
            self.blur_since = None;
        } else {
            if self.focused {
                self.blur_since = Some(now);
            }
            self.focused = false;
        }
    }

    /// `true` se l'autoscomparsa sta per scattare (serve a svegliare la GUI).
    pub fn auto_hide_pending(&self) -> bool {
        self.auto_hide && !self.focused && self.blur_since.is_some()
    }

    /// Decide se il pannello deve nascondersi adesso.
    pub fn should_auto_hide(&self, now: Instant, pointer_inside: bool, pointer_down: bool) -> bool {
        if !self.auto_hide || !self.open || self.focused || pointer_inside || pointer_down {
            return false;
        }
        self.blur_since
            .is_some_and(|since| now.duration_since(since) >= AUTO_HIDE_DELAY)
    }
}

impl Drop for TerminalState {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Traduce un tasto in byte da mandare alla shell.
pub fn key_bytes(key: Key, modifiers: Modifiers, application_cursor: bool) -> Option<Vec<u8>> {
    // Ctrl+lettera = codice di controllo (Ctrl+C = 0x03, Ctrl+D = 0x04, …).
    if modifiers.ctrl {
        if let Some(code) = control_code(key) {
            return Some(vec![code]);
        }
    }

    let sequence: &[u8] = match key {
        Key::Enter => b"\r",
        Key::Backspace => b"\x7f",
        Key::Tab if modifiers.shift => b"\x1b[Z",
        Key::Tab => b"\t",
        Key::Escape => b"\x1b",
        Key::ArrowUp if application_cursor => b"\x1bOA",
        Key::ArrowDown if application_cursor => b"\x1bOB",
        Key::ArrowRight if application_cursor => b"\x1bOC",
        Key::ArrowLeft if application_cursor => b"\x1bOD",
        Key::ArrowUp => b"\x1b[A",
        Key::ArrowDown => b"\x1b[B",
        Key::ArrowRight => b"\x1b[C",
        Key::ArrowLeft => b"\x1b[D",
        Key::Home if application_cursor => b"\x1bOH",
        Key::End if application_cursor => b"\x1bOF",
        Key::Home => b"\x1b[H",
        Key::End => b"\x1b[F",
        Key::Insert => b"\x1b[2~",
        Key::Delete => b"\x1b[3~",
        Key::PageUp => b"\x1b[5~",
        Key::PageDown => b"\x1b[6~",
        _ => return None,
    };
    Some(sequence.to_vec())
}

/// Codice di controllo per `Ctrl+<tasto>`.
fn control_code(key: Key) -> Option<u8> {
    let name = key.name();
    let mut chars = name.chars();
    if let (Some(letter), None) = (chars.next(), chars.next()) {
        if letter.is_ascii_uppercase() {
            return Some(letter as u8 - b'A' + 1);
        }
    }
    match name {
        "2" | "Space" | "Backtick" => Some(0),
        "OpenBracket" => Some(27),
        "Backslash" => Some(28),
        "CloseBracket" => Some(29),
        _ => None,
    }
}

/// Programma e argomenti dello shell interattivo della piattaforma.
#[cfg(unix)]
fn shell_command() -> (String, Vec<String>) {
    let shell = std::env::var("SHELL")
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| {
            if Path::new("/bin/bash").exists() {
                "/bin/bash".to_string()
            } else {
                "/bin/sh".to_string()
            }
        });
    (shell, Vec::new())
}

#[cfg(windows)]
fn shell_command() -> (String, Vec<String>) {
    let shell = std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".to_string());
    (shell, Vec::new())
}

/// Crea il PTY e avvia il processo, con un thread che ne legge l'output.
fn spawn(
    program: &str,
    args: &[String],
    cwd: &Path,
    rows: u16,
    cols: u16,
    repaint: Option<Context>,
) -> Result<Process, String> {
    let pty = native_pty_system()
        .openpty(PtySize {
            rows: rows.max(1),
            cols: cols.max(1),
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| format!("impossibile creare il terminale: {e}"))?;

    let mut command = CommandBuilder::new(program);
    for arg in args {
        command.arg(arg);
    }
    command.cwd(cwd);
    // Un terminale vero, non una pipe: i programmi scelgono colori e modalità
    // interattive in base a queste variabili.
    command.env("TERM", "xterm-256color");
    command.env("COLORTERM", "truecolor");
    command.env("RIDE_TERMINAL", "1");

    let mut child = pty
        .slave
        .spawn_command(command)
        .map_err(|e| format!("impossibile avviare «{program}»: {e}"))?;
    // Il lato slave serve solo al figlio.
    drop(pty.slave);

    let mut reader = pty
        .master
        .try_clone_reader()
        .map_err(|e| format!("impossibile leggere dal terminale: {e}"))?;
    let writer = pty
        .master
        .take_writer()
        .map_err(|e| format!("impossibile scrivere nel terminale: {e}"))?;
    let killer = child.clone_killer();

    let (tx, events) = mpsc::channel();

    // Lettore: manda i byte alla GUI e la risveglia.
    let output_tx = tx.clone();
    let output_ctx = repaint.clone();
    std::thread::Builder::new()
        .name("ride-terminal-out".into())
        .spawn(move || {
            let mut buffer = [0u8; 8192];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(n) => {
                        if output_tx
                            .send(TerminalEvent::Output(buffer[..n].to_vec()))
                            .is_err()
                        {
                            break;
                        }
                        if let Some(ctx) = &output_ctx {
                            ctx.request_repaint();
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                }
            }
        })
        .ok();

    // Attesa: comunica la fine del processo senza bloccare la GUI.
    let exit_tx = tx;
    let exit_ctx = repaint;
    std::thread::Builder::new()
        .name("ride-terminal-wait".into())
        .spawn(move || {
            let message = match child.wait() {
                Ok(status) if status.success() => "Terminale terminato".to_string(),
                Ok(status) => match status.signal() {
                    Some(signal) => format!("Terminale terminato dal segnale {signal}"),
                    None => format!("Terminale terminato (codice {})", status.exit_code()),
                },
                Err(e) => format!("Terminale terminato con errore: {e}"),
            };
            let _ = exit_tx.send(TerminalEvent::Exited(message));
            if let Some(ctx) = exit_ctx {
                ctx.request_repaint();
            }
        })
        .ok();

    Ok(Process {
        master: pty.master,
        writer,
        killer,
        events,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn i_tasti_speciali_diventano_sequenze_del_terminale() {
        let none = Modifiers::NONE;
        assert_eq!(key_bytes(Key::Enter, none, false), Some(b"\r".to_vec()));
        assert_eq!(
            key_bytes(Key::Backspace, none, false),
            Some(b"\x7f".to_vec())
        );
        assert_eq!(
            key_bytes(Key::ArrowUp, none, false),
            Some(b"\x1b[A".to_vec())
        );
        assert_eq!(
            key_bytes(Key::ArrowUp, none, true),
            Some(b"\x1bOA".to_vec())
        );
        assert_eq!(
            key_bytes(Key::Tab, Modifiers::SHIFT, false),
            Some(b"\x1b[Z".to_vec())
        );
    }

    #[test]
    fn ctrl_lettera_e_un_codice_di_controllo() {
        let ctrl = Modifiers::CTRL;
        assert_eq!(key_bytes(Key::C, ctrl, false), Some(vec![3]));
        assert_eq!(key_bytes(Key::D, ctrl, false), Some(vec![4]));
        assert_eq!(key_bytes(Key::Space, ctrl, false), Some(vec![0]));
    }

    #[test]
    fn l_autoscomparsa_scatta_solo_dopo_il_ritardo_e_fuori_dal_pannello() {
        let now = Instant::now();
        let mut terminal = TerminalState::new();
        terminal.open = true;
        terminal.auto_hide = true;
        terminal.set_focused(true, now);
        terminal.set_focused(false, now);

        assert!(!terminal.should_auto_hide(now + Duration::from_millis(200), false, false));
        assert!(terminal.should_auto_hide(now + AUTO_HIDE_DELAY, false, false));
        // Con il puntatore sopra (o il mouse premuto) non si nasconde.
        assert!(!terminal.should_auto_hide(now + AUTO_HIDE_DELAY, true, false));
        assert!(!terminal.should_auto_hide(now + AUTO_HIDE_DELAY, false, true));
        // Disattivata, non nasconde mai.
        terminal.auto_hide = false;
        assert!(!terminal.should_auto_hide(now + AUTO_HIDE_DELAY, false, false));
    }

    #[cfg(unix)]
    #[test]
    fn il_terminale_legge_l_output_del_processo() {
        let mut terminal = TerminalState::default();
        terminal.resize(10, 40);
        let (program, args) = (
            "/bin/sh".to_string(),
            vec!["-c".to_string(), "printf 'ciao dal terminale'".to_string()],
        );
        let cwd = std::env::temp_dir();
        terminal.process =
            Some(spawn(&program, &args, &cwd, 10, 40, None).expect("il processo non è partito"));
        terminal.cwd = cwd;

        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            terminal.poll();
            if terminal.screen().contents().contains("ciao dal terminale") {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            terminal.screen().contents().contains("ciao dal terminale"),
            "output non ricevuto: {:?}",
            terminal.screen().contents()
        );
    }
}
