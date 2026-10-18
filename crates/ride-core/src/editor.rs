//! Operazioni di editing sul testo, senza GUI.
//!
//! ## Convenzioni sulle posizioni
//!
//! Il testo è una `String` UTF-8, quindi gli indici "veri" sono offset in
//! **byte**. I cursori della GUI invece (in particolare `egui::text::CCursor`)
//! contano **caratteri**. Per non confondersi:
//!
//! * le funzioni che parlano di cursore usano l'indice di carattere
//!   ([`Lines::line_col_of_char`], [`word_at`], [`bracket_match`], …);
//! * le mutazioni lavorano per **numero di riga** ([`indent_lines`],
//!   [`toggle_comment`], …), che è l'unità con cui ragiona l'utente.
//!
//! Tutti gli helper di conversione byte↔carattere vivono in questo modulo.

/// Indice di riga massimo su cui le operazioni "a blocchi" lavorano, per non
/// esplodere su file enormi: oltre questa soglia la GUI disattiva l'operazione.
pub const MAX_BLOCK_LINES: usize = 200_000;

/// Mappa delle righe di un testo: offset di inizio (in byte) di ogni riga.
///
/// Segue la convenzione degli editor (e di `TextEdit` di egui): `"a\n"` ha
/// **due** righe, la seconda vuota.
#[derive(Debug, Clone)]
pub struct Lines {
    starts: Vec<usize>,
    len: usize,
}

impl Lines {
    /// Costruisce la mappa. Costo O(n) in byte: si fa una volta e si riusa.
    pub fn new(text: &str) -> Self {
        let mut starts = Vec::with_capacity(text.len() / 24 + 1);
        starts.push(0);
        for (i, b) in text.bytes().enumerate() {
            if b == b'\n' {
                starts.push(i + 1);
            }
        }
        Lines {
            starts,
            len: text.len(),
        }
    }

    /// Numero di righe (≥ 1).
    pub fn count(&self) -> usize {
        self.starts.len()
    }

    /// Offset di inizio della riga `line`, in byte.
    pub fn start(&self, line: usize) -> Option<usize> {
        self.starts.get(line).copied()
    }

    /// Intervallo `(inizio, fine)` della riga, **senza** il terminatore di riga.
    pub fn range(&self, text: &str, line: usize) -> Option<(usize, usize)> {
        let start = self.start(line)?;
        let mut end = self
            .starts
            .get(line + 1)
            .copied()
            .unwrap_or(self.len)
            .min(self.len);
        let bytes = text.as_bytes();
        if end > start && bytes[end - 1] == b'\n' {
            end -= 1;
        }
        if end > start && bytes[end - 1] == b'\r' {
            end -= 1;
        }
        Some((start, end.max(start)))
    }

    /// Testo della riga, senza terminatore.
    pub fn line<'a>(&self, text: &'a str, line: usize) -> Option<&'a str> {
        let (s, e) = self.range(text, line)?;
        Some(&text[s..e])
    }

    /// Riga e colonna (0-based, colonna in caratteri) di un offset in byte.
    pub fn line_col_of_byte(&self, text: &str, byte: usize) -> (usize, usize) {
        let byte = byte.min(text.len());
        // Indice dell'ultima riga il cui inizio è <= byte.
        let line = self.starts.partition_point(|start| *start <= byte).saturating_sub(1);
        let start = self.starts.get(line).copied().unwrap_or(0);
        let col = text.get(start..byte).map(|s| s.chars().count()).unwrap_or(0);
        (line, col)
    }

    /// Riga e colonna di un indice di carattere.
    pub fn line_col_of_char(&self, text: &str, char_idx: usize) -> (usize, usize) {
        self.line_col_of_byte(text, byte_of_char(text, char_idx))
    }

    /// Offset in byte della coppia riga/colonna (colonna in caratteri).
    pub fn byte_of_line_col(&self, text: &str, line: usize, col: usize) -> usize {
        let Some(start) = self.start(line) else {
            return text.len();
        };
        let end = self.range(text, line).map(|(_, e)| e).unwrap_or(self.len);
        let slice = &text[start..end];
        match slice.char_indices().nth(col) {
            Some((offset, _)) => start + offset,
            None => end,
        }
    }

    /// Offset in byte di un indice di carattere.
    pub fn byte_of_char(&self, text: &str, char_idx: usize) -> usize {
        byte_of_char(text, char_idx)
    }
}

/// Offset in byte dell'`char_idx`-esimo carattere (`char_idx == n_caratteri` → fine testo).
pub fn byte_of_char(text: &str, char_idx: usize) -> usize {
    match text.char_indices().nth(char_idx) {
        Some((offset, _)) => offset,
        None => text.len(),
    }
}

/// Indice di carattere dell'offset in byte `byte` (arrotondato in basso).
pub fn char_of_byte(text: &str, byte: usize) -> usize {
    if byte >= text.len() {
        return text.chars().count();
    }
    let mut byte = byte;
    while byte > 0 && !text.is_char_boundary(byte) {
        byte -= 1;
    }
    text[..byte].chars().count()
}

/// Numero di righe di un testo (≥ 1).
pub fn line_count(text: &str) -> usize {
    text.bytes().filter(|b| *b == b'\n').count() + 1
}

/// Una riga senza il terminatore.
fn dedent_line<'a>(line: &'a str, unit: &str) -> &'a str {
    if let Some(rest) = line.strip_prefix(unit) {
        return rest;
    }
    // Dedenta comunque, rimuovendo fino a `unit` larghezza di spazi/tab.
    let mut removed = 0usize;
    for c in line.chars() {
        if removed >= unit.len() || (c != ' ' && c != '\t') {
            break;
        }
        removed += c.len_utf8();
    }
    &line[removed..]
}

/// Il testo spezzato in righe, per poterlo modificare "per righe" e ricomporre.
///
/// `split('\n')` / `join("\n")` è esatto per il testo che gestiamo (gli a capo
/// CRLF vengono normalizzati al caricamento, vedi [`crate::document`]).
struct LineBuf {
    lines: Vec<String>,
}

impl LineBuf {
    fn from_str(text: &str) -> Self {
        LineBuf {
            lines: text.split('\n').map(|l| l.to_string()).collect(),
        }
    }

    fn last_index(&self) -> usize {
        self.lines.len().saturating_sub(1)
    }
}

impl std::fmt::Display for LineBuf {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.lines.join("\n"))
    }
}

/// Indenta o dedenta le righe `first..=last`.
///
/// Ritorna l'intervallo di righe interessate (utile alla GUI per riposizionare
/// la selezione).
pub fn indent_lines(
    text: &mut String,
    first: usize,
    last: usize,
    unit: &str,
    dedent: bool,
) -> (usize, usize) {
    let mut buf = LineBuf::from_str(text);
    let last = last.min(buf.last_index());
    if first > last || buf.lines.is_empty() {
        return (first, last);
    }
    for line in &mut buf.lines[first..=last] {
        if dedent {
            let new = dedent_line(line, unit).to_string();
            *line = new;
        } else {
            line.insert_str(0, unit);
        }
    }
    *text = buf.to_string();
    (first, last)
}

/// Commenta (o scommenta) le righe `first..=last` con `prefix`.
///
/// Le righe vuote vengono ignorate nel calcolo di "è già commentato?"; se lo
/// sono tutte, l'operazione le scommenta.
pub fn toggle_comment(
    text: &mut String,
    first: usize,
    last: usize,
    prefix: &str,
) -> (usize, usize) {
    let mut buf = LineBuf::from_str(text);
    let last = last.min(buf.last_index());
    if first > last || prefix.is_empty() {
        return (first, last);
    }

    let is_commented = |line: &str| {
        let trimmed = line.trim_start();
        if trimmed.is_empty() {
            return true; // le righe vuote non contano
        }
        trimmed.starts_with(prefix)
    };
    let all_commented = buf.lines[first..=last].iter().all(|l| is_commented(l));

    for line in &mut buf.lines[first..=last] {
        if line.trim().is_empty() {
            continue;
        }
        if all_commented {
            // Scommenta: prefix subito dopo l'indentazione, più un eventuale spazio.
            let indent_len = line.len() - line.trim_start().len();
            let (indent, rest) = line.split_at(indent_len);
            if let Some(rest) = rest.strip_prefix(prefix) {
                let rest = rest.strip_prefix(' ').unwrap_or(rest);
                *line = format!("{indent}{rest}");
            }
        } else {
            let indent_len = line.len() - line.trim_start().len();
            let (indent, rest) = line.split_at(indent_len);
            if rest.starts_with(prefix) {
                continue;
            }
            let sep = if rest.is_empty() || rest.starts_with(' ') { "" } else { " " };
            *line = format!("{indent}{prefix}{sep}{rest}");
        }
    }
    *text = buf.to_string();
    (first, last)
}

/// Duplica le righe `first..=last` subito sotto; ritorna le righe della copia.
pub fn duplicate_lines(text: &mut String, first: usize, last: usize) -> (usize, usize) {
    let mut buf = LineBuf::from_str(text);
    let last = last.min(buf.last_index());
    if first > last {
        return (first, last);
    }
    let block: Vec<String> = buf.lines[first..=last].to_vec();
    let n = block.len();
    // Non duplicare l'ultima riga vuota "virtuale" di un file che finisce con \n.
    buf.lines.splice(last + 1..last + 1, block);
    *text = buf.to_string();
    (first + n, last + n)
}

/// Sposta le righe `first..=last` di una posizione in su o in giù.
pub fn move_lines(
    text: &mut String,
    first: usize,
    last: usize,
    up: bool,
) -> (usize, usize) {
    let mut buf = LineBuf::from_str(text);
    let last = last.min(buf.last_index());
    if first > last {
        return (first, last);
    }
    if up {
        if first == 0 {
            return (first, last);
        }
        let block: Vec<String> = buf.lines.drain(first..=last).collect();
        buf.lines.splice(first - 1..first - 1, block);
        *text = buf.to_string();
        (first - 1, last - 1)
    } else {
        let last_index = buf.last_index();
        if last >= last_index {
            return (first, last);
        }
        // Non far scendere l'ultima riga di contenuto oltre la riga vuota
        // "virtuale" creata dall'a capo finale: sarebbe un salto nel vuoto.
        if last + 1 == last_index && buf.lines[last_index].is_empty() {
            return (first, last);
        }
        let block: Vec<String> = buf.lines.drain(first..=last).collect();
        buf.lines.splice(first + 1..first + 1, block);
        *text = buf.to_string();
        (first + 1, last + 1)
    }
}

/// Cancella le righe `first..=last`.
pub fn delete_lines(text: &mut String, first: usize, last: usize) -> usize {
    let mut buf = LineBuf::from_str(text);
    let last = last.min(buf.last_index());
    if first > last {
        return first;
    }
    buf.lines.drain(first..=last);
    if buf.lines.is_empty() {
        buf.lines.push(String::new());
    }
    *text = buf.to_string();
    first.min(buf.last_index())
}

/// La "parola" (identificatore) attorno al carattere `char_idx`.
///
/// Serve per il completamento/definition: quando implementeremo l'LSP la
/// useremo per decidere *cosa* chiedere al server.
pub fn word_at(text: &str, char_idx: usize) -> &str {
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let idx = char_idx.min(chars.len());
    let mut start = idx;
    while start > 0 && is_word(chars[start - 1].1) {
        start -= 1;
    }
    let mut end = idx;
    while end < chars.len() && is_word(chars[end].1) {
        end += 1;
    }
    let from = chars.get(start).map(|(b, _)| *b).unwrap_or(text.len());
    let to = chars.get(end).map(|(b, _)| *b).unwrap_or(text.len());
    &text[from..to]
}

const BRACKETS: [(char, char); 3] = [('(', ')'), ('[', ']'), ('{', '}')];

/// Offset in byte della parentesi corrispondente a quella sotto (o subito prima
/// de) il cursore.
///
/// Nota: il conteggio è "ingenuo", non conosce stringhe e commenti. È
/// sufficiente nella maggior parte dei casi; la versione precisa arriverà
/// dall'LSP.
pub fn bracket_match(text: &str, char_idx: usize) -> Option<usize> {
    mate_of(text, bracket_at(text, char_idx)?)
}

/// Offset in byte della parentesi corrispondente a quella in `at` (indice di
/// carattere).
fn mate_of(text: &str, at: usize) -> Option<usize> {
    let here = char_at(text, at)?;
    let (opening, closing) = pair_of(here)?;

    if here == opening {
        // In avanti, contando le annidature.
        let mut depth = 1i32;
        for (byte, c) in text.char_indices().skip(at + 1) {
            if c == opening {
                depth += 1;
            } else if c == closing {
                depth -= 1;
                if depth == 0 {
                    return Some(byte);
                }
            }
        }
        None
    } else {
        // All'indietro, partendo da `at`.
        let byte_at = byte_of_char(text, at);
        let mut depth = 1i32;
        for (byte, c) in text[..byte_at].char_indices().rev() {
            if c == closing {
                depth += 1;
            } else if c == opening {
                depth -= 1;
                if depth == 0 {
                    return Some(byte);
                }
            }
        }
        None
    }
}

/// La coppia `(apertura, chiusura)` di cui `c` fa parte.
fn pair_of(c: char) -> Option<(char, char)> {
    BRACKETS
        .iter()
        .find(|(o, cl)| c == *o || c == *cl)
        .copied()
}

/// Il `char_idx`-esimo carattere del testo (`None` se oltre la fine).
pub fn char_at(text: &str, char_idx: usize) -> Option<char> {
    text.chars().nth(char_idx)
}

/// I due caratteri attorno a `char_idx`: `(subito a sinistra, sotto il
/// cursore)`, in una sola scansione.
fn chars_around(text: &str, char_idx: usize) -> (Option<char>, Option<char>) {
    match char_idx.checked_sub(1) {
        Some(before) => {
            let mut iter = text.chars().skip(before);
            (iter.next(), iter.next())
        }
        None => (None, text.chars().next()),
    }
}

/// La parentesi toccata dal cursore: quella sotto di esso o, in mancanza,
/// quella immediatamente a sinistra (indice di carattere).
pub fn bracket_at(text: &str, char_idx: usize) -> Option<usize> {
    // Stesso ordine di preferenza di prima: sotto, poi a sinistra.
    let (before, here) = chars_around(text, char_idx);
    if here.is_some_and(|c| pair_of(c).is_some()) {
        return Some(char_idx);
    }
    if before.is_some_and(|c| pair_of(c).is_some()) {
        return Some(char_idx - 1);
    }
    None
}

/// La coppia di parentesi da evidenziare quando il cursore è a `char_idx`.
///
/// Restituisce `(posizione, corrispondente)`: la parentesi toccata dal cursore
/// e la sua gemella (se esiste), in **indici di carattere**. `None` se il
/// cursore non tocca alcuna parentesi.
pub fn bracket_pair(text: &str, char_idx: usize) -> Option<(usize, Option<usize>)> {
    let at = bracket_at(text, char_idx)?;
    let mate = mate_of(text, at).map(|byte| char_of_byte(text, byte));
    Some((at, mate))
}

/// La chiusura corrispondente a una parentesi aperta.
pub fn closing_bracket(open: char) -> Option<char> {
    BRACKETS.iter().find(|(o, _)| *o == open).map(|(_, cl)| *cl)
}

/// `true` se `c` è una parentesi aperta (`(`, `[`, `{`).
pub fn is_opening_bracket(c: char) -> bool {
    closing_bracket(c).is_some()
}

/// `true` se `c` è una parentesi chiusa (`)`, `]`, `}`).
pub fn is_closing_bracket(c: char) -> bool {
    BRACKETS.iter().any(|(_, cl)| *cl == c)
}

/// `true` se `c` fa parte di una parola (lettera, cifra o `_`).
pub fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Cosa fare automaticamente dopo che l'utente ha digitato un carattere.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoPair {
    /// Aggiungi la chiusura e lascia il cursore fra le due.
    Close,
    /// La chiusura è già presente: scavalcala, non inserire nulla.
    Skip,
    /// Nessun automatismo.
    Plain,
}

/// Decide l'automatismo per il carattere appena digitato.
///
/// `next` è il carattere che segue il cursore **prima** della digitazione e
/// presuppone che non ci sia selezione.
pub fn auto_pair(next: Option<char>, typed: char) -> AutoPair {
    if let Some(close) = closing_bracket(typed) {
        // Niente coppia se la chiusura c'è già o se stiamo scrivendo dentro una
        // parola (`foo(` non deve diventare `foo()`).
        match next {
            Some(c) if c == close || is_word_char(c) => AutoPair::Plain,
            _ => AutoPair::Close,
        }
    } else if is_closing_bracket(typed) && next == Some(typed) {
        AutoPair::Skip
    } else {
        AutoPair::Plain
    }
}

/// Cosa inserire dopo un `Invio` capitato all'inizio di una riga.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnterEdit {
    /// Nessun inserimento.
    None,
    /// Indenta la riga nuova con questo testo.
    Indent(String),
    /// Apre un blocco: `inner` sulla riga del cursore, poi a capo e
    /// `close_indent` prima della chiusura già presente.
    Block { inner: String, close_indent: String },
}

/// Il piano di indentazione per un `Invio`: `caret` è l'indice di carattere del
/// cursore (ora a inizio riga), `unit` l'unità di indentazione del linguaggio.
///
/// Se la riga precedente è stata aperta (`{`, `(`, `[`) **e** la chiusura è
/// subito dopo il cursore, il risultato è un blocco su tre righe:
///
/// ```text
/// {
///     |
/// }
/// ```
pub fn enter_edit(text: &str, caret: usize, unit: &str) -> EnterEdit {
    let lines = Lines::new(text);
    let (line, column) = lines.line_col_of_char(text, caret);
    if column != 0 || line == 0 {
        return EnterEdit::None;
    }
    let previous = lines.line(text, line - 1).unwrap_or("");
    let indent: String = previous
        .chars()
        .take_while(|c| matches!(c, ' ' | '\t'))
        .collect();

    match previous.trim_end().chars().last() {
        Some(open) if is_opening_bracket(open) => {
            let close = closing_bracket(open).expect("apertura senza chiusura");
            if char_at(text, caret) == Some(close) {
                EnterEdit::Block {
                    inner: format!("{indent}{unit}"),
                    close_indent: indent,
                }
            } else {
                EnterEdit::Indent(format!("{indent}{unit}"))
            }
        }
        _ if indent.is_empty() => EnterEdit::None,
        _ => EnterEdit::Indent(indent),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mappa_delle_righe() {
        let text = "a\nbb\nccc";
        let lines = Lines::new(text);
        assert_eq!(lines.count(), 3);
        assert_eq!(lines.range(text, 1), Some((2, 4)));
        assert_eq!(lines.line(text, 2), Some("ccc"));
        assert_eq!(line_count(text), 3);
        // Testo che finisce con a capo: c'è una riga vuota in fondo.
        let text = "a\n";
        assert_eq!(Lines::new(text).count(), 2);
        assert_eq!(Lines::new(text).range(text, 1), Some((2, 2)));
    }

    #[test]
    fn riga_colonna() {
        let text = "ciao\nè bello\nx";
        let lines = Lines::new(text);
        assert_eq!(lines.line_col_of_char(text, 0), (0, 0));
        assert_eq!(lines.line_col_of_char(text, 5), (1, 0));
        assert_eq!(lines.line_col_of_char(text, 7), (1, 2));
        // `è` occupa due byte: la colonna 2 della riga 1 è al byte 8.
        assert_eq!(lines.byte_of_line_col(text, 1, 2), 8);
        assert_eq!(char_of_byte(text, 8), 7);
        assert_eq!(lines.byte_of_char(text, 7), 8);
        // Oltre la fine della riga si finisce a fine riga.
        assert_eq!(lines.byte_of_line_col(text, 1, 99), 13);
    }

    #[test]
    fn indenta_e_dedenta() {
        let mut text = "fn main() {\nreturn;\n}\n".to_string();
        indent_lines(&mut text, 1, 1, "    ", false);
        assert_eq!(text, "fn main() {\n    return;\n}\n");
        indent_lines(&mut text, 1, 1, "    ", true);
        assert_eq!(text, "fn main() {\nreturn;\n}\n");
        // Dedenta anche con meno spazi del previsto.
        let mut text = "  due spazi\n".to_string();
        indent_lines(&mut text, 0, 0, "    ", true);
        assert_eq!(text, "due spazi\n");
    }

    #[test]
    fn commenta_e_scommenta() {
        let mut text = "let a = 1;\nlet b = 2;\n".to_string();
        toggle_comment(&mut text, 0, 1, "//");
        assert_eq!(text, "// let a = 1;\n// let b = 2;\n");
        toggle_comment(&mut text, 0, 1, "//");
        assert_eq!(text, "let a = 1;\nlet b = 2;\n");
    }

    #[test]
    fn duplica_e_sposta() {
        let mut text = "a\nb\nc\n".to_string();
        let (f, l) = duplicate_lines(&mut text, 1, 1);
        assert_eq!(text, "a\nb\nb\nc\n");
        assert_eq!((f, l), (2, 2));
        let (f, l) = move_lines(&mut text, 1, 1, true);
        assert_eq!(text, "b\na\nb\nc\n");
        assert_eq!((f, l), (0, 0));
        let (_, _) = move_lines(&mut text, 3, 3, false);
        assert_eq!(text, "b\na\nb\nc\n");
    }

    #[test]
    fn cancella_righe() {
        let mut text = "a\nb\nc".to_string();
        delete_lines(&mut text, 1, 1);
        assert_eq!(text, "a\nc");
        let mut text = "a".to_string();
        delete_lines(&mut text, 0, 0);
        assert_eq!(text, "");
    }

    #[test]
    fn parola_sotto_al_cursore() {
        assert_eq!(word_at("let valore = 1;", 6), "valore");
        assert_eq!(word_at("let valore = 1;", 4), "valore");
        // Sullo spazio dopo `let` si prende comunque la parola a sinistra.
        assert_eq!(word_at("let valore = 1;", 3), "let");
        assert_eq!(word_at("let valore = 1;", 0), "let");
        // Su un carattere non-parola si prende la parola a sinistra (comodo per
        // il completamento dopo il punto).
        assert_eq!(word_at("dopo_il_punto.", 13), "dopo_il_punto");
        // Senza nessuna parola attorno: vuoto.
        assert_eq!(word_at(" ()", 1), "");
    }

    #[test]
    fn parentesi_corrispondenti() {
        let text = "fn f(a: i32) -> i32 { a }";
        let open = text.find('(').unwrap();
        let close = bracket_match(text, open).unwrap();
        assert_eq!(&text[close..close + 1], ")");
        // E dalla chiusura si torna all'apertura.
        assert_eq!(bracket_match(text, close), Some(open));

        // Con il cursore subito dopo la graffa aperta si trova la chiusura.
        let brace = text.find('{').unwrap();
        let end = bracket_match(text, brace + 1).unwrap();
        assert_eq!(&text[end..end + 1], "}");
        assert_eq!(bracket_match(text, end), Some(brace));

        // Parentesi non bilanciata: nessun risultato.
        assert_eq!(bracket_match("fn f(a", 4), None);
    }

    #[test]
    fn coppia_da_evidenziare() {
        let text = "fn f(a) { b }";
        let open = text.chars().position(|c| c == '(').unwrap();
        let close = text.chars().position(|c| c == ')').unwrap();
        // Cursore subito dopo `(`: si evidenzia la coppia.
        assert_eq!(bracket_pair(text, open + 1), Some((open, Some(close))));
        // Cursore sulla `)`.
        assert_eq!(bracket_pair(text, close), Some((close, Some(open))));
        // Nessuna parentesi sotto il cursore.
        assert_eq!(bracket_pair("ciao", 2), None);
        // Apertura senza chiusura.
        assert_eq!(bracket_pair("a(b", 2), Some((1, None)));
        // Con caratteri non ASCII restano indici di *carattere*.
        assert_eq!(bracket_pair("è (x)", 3), Some((2, Some(4))));
    }

    #[test]
    fn chiusura_automatica_delle_parentesi() {
        assert_eq!(auto_pair(None, '('), AutoPair::Close);
        assert_eq!(auto_pair(Some(' '), '{'), AutoPair::Close);
        assert_eq!(auto_pair(Some('\n'), '['), AutoPair::Close);
        // Davanti alla chiusura giusta, o dentro una parola, non si duplica.
        assert_eq!(auto_pair(Some(')'), '('), AutoPair::Plain);
        assert_eq!(auto_pair(Some('x'), '('), AutoPair::Plain);
        assert_eq!(auto_pair(Some('_'), '{'), AutoPair::Plain);
        // Scavalco di una chiusura già presente.
        assert_eq!(auto_pair(Some(')'), ')'), AutoPair::Skip);
        assert_eq!(auto_pair(Some('}'), '}'), AutoPair::Skip);
        assert_eq!(auto_pair(Some('a'), ')'), AutoPair::Plain);
        // Caratteri normali: nessun automatismo.
        assert_eq!(auto_pair(None, ';'), AutoPair::Plain);
    }

    #[test]
    fn piano_di_indentazione_dopo_invio() {
        // Blocco aperto, chiusura auto-inserita subito dopo il cursore.
        assert_eq!(
            enter_edit("{\n}", 2, "    "),
            EnterEdit::Block {
                inner: "    ".into(),
                close_indent: String::new(),
            }
        );
        // Blocco indentato: la chiusura eredita il rientro della riga aperta.
        assert_eq!(
            enter_edit("    if x {\n}", 11, "    "),
            EnterEdit::Block {
                inner: "        ".into(),
                close_indent: "    ".into(),
            }
        );
        // Apertura senza chiusura: si indenta soltanto.
        assert_eq!(
            enter_edit("foo(\n", 5, "  "),
            EnterEdit::Indent("  ".into())
        );
        // Nessuna apertura ma riga indentata: si mantiene il rientro.
        assert_eq!(enter_edit("    \n", 5, "    "), EnterEdit::Indent("    ".into()));
        // Nulla da fare.
        assert_eq!(enter_edit("x\n", 2, "  "), EnterEdit::None);
    }
}
