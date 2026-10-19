//! Riconoscimento delle definizioni di metodi e funzioni.
//!
//! Serve al canale dei numeri di riga: una `M` segna la riga in cui *inizia* una
//! funzione o un metodo, e il nome compare in un popup al passaggio del mouse.
//!
//! Il riconoscimento è volutamente **euristico e per riga**: qui non c'è un
//! parser, solo la forma tipica di una dichiarazione. Per un uso "da editor" è
//! più che sufficiente e non costa quasi nulla, come la piegatura dei blocchi
//! (vedi [`crate::folds`]).

use crate::syntax::Language;

/// Una definizione trovata nel testo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionSymbol {
    /// Riga (0-based) in cui inizia la definizione.
    pub line: usize,
    /// Nome della funzione o del metodo.
    pub name: String,
}

/// Tutte le funzioni/metodi riconosciuti nel testo, ordinati per riga.
///
/// Le righe sono 0-based, esattamente come le conta l'editor.
pub fn function_symbols(text: &str, language: Language) -> Vec<FunctionSymbol> {
    let mut symbols = Vec::new();
    for (line, raw) in text.lines().enumerate() {
        let line_text = raw.trim();
        if line_text.is_empty() || is_comment(line_text) {
            continue;
        }
        let name = match language {
            Language::Rust => word_definition(line_text, "fn"),
            Language::Python => word_definition(line_text, "def"),
            Language::Go => go_function(line_text),
            Language::JavaScript | Language::TypeScript => js_function(line_text),
            Language::Java | Language::C | Language::Cpp => c_like_function(line_text),
            Language::Shell => shell_function(line_text),
            _ => None,
        };
        if let Some(name) = name {
            symbols.push(FunctionSymbol { line, name });
        }
    }
    symbols
}

/// Una riga che è solo un commento non contiene definizioni.
fn is_comment(line: &str) -> bool {
    line.starts_with("//")
        || line.starts_with('#')
        || line.starts_with('*')
        || line.starts_with("/*")
        || line.starts_with("<!--")
}

/// `true` se il carattere può far parte di un identificatore.
fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Il primo identificatore di `text`, saltando gli spazi iniziali.
fn first_ident(text: &str) -> Option<&str> {
    let trimmed = text.trim_start();
    let end = trimmed
        .find(|c: char| !is_ident_char(c))
        .unwrap_or(trimmed.len());
    if end == 0 {
        return None;
    }
    Some(&trimmed[..end])
}

/// L'ultimo identificatore di `text`, con la posizione in cui inizia.
fn last_ident(text: &str) -> Option<(&str, usize)> {
    let trimmed = text.trim_end();
    let end = trimmed.len();
    let mut begin = end;
    for (i, c) in trimmed.char_indices().rev() {
        if is_ident_char(c) {
            begin = i;
        } else {
            break;
        }
    }
    if begin == end {
        return None;
    }
    // Un nome non inizia con una cifra.
    if trimmed[begin..].starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    Some((&trimmed[begin..end], begin))
}

/// Trova `keyword` come parola intera e restituisce la posizione in byte.
fn find_word(line: &str, keyword: &str) -> Option<usize> {
    let mut from = 0;
    while let Some(offset) = line[from..].find(keyword) {
        let at = from + offset;
        let before_ok = line[..at]
            .chars()
            .next_back()
            .is_none_or(|c| !is_ident_char(c));
        let after = at + keyword.len();
        let after_ok = line[after..]
            .chars()
            .next()
            .is_none_or(|c| !is_ident_char(c));
        if before_ok && after_ok {
            return Some(at);
        }
        from = after;
    }
    None
}

/// Definizione del tipo `fn nome` / `def nome` (nome subito dopo la parola).
fn word_definition(line: &str, keyword: &str) -> Option<String> {
    let at = find_word(line, keyword)?;
    first_ident(&line[at + keyword.len()..]).map(str::to_string)
}

/// `func nome(…)` e `func (ricevitore) nome(…)` di Go.
fn go_function(line: &str) -> Option<String> {
    let at = find_word(line, "func")?;
    let rest = line[at + "func".len()..].trim_start();
    let rest = if let Some(receiver) = rest.strip_prefix('(') {
        rest[receiver.find(')')? + 2..].trim_start()
    } else {
        rest
    };
    first_ident(rest).map(str::to_string)
}

/// Funzioni e arrow function di JavaScript/TypeScript.
fn js_function(line: &str) -> Option<String> {
    if let Some(at) = find_word(line, "function") {
        let rest = line[at + "function".len()..].trim_start();
        let rest = rest.strip_prefix('*').map(str::trim_start).unwrap_or(rest);
        if let Some(name) = first_ident(rest) {
            return Some(name.to_string());
        }
    }
    if let Some(name) = arrow_name(line) {
        return Some(name);
    }
    c_like_function(line)
}

/// Il nome a sinistra di `= (…) =>`.
fn arrow_name(line: &str) -> Option<String> {
    let arrow = line.find("=>")?;
    let lhs = &line[..arrow];
    let eq = lhs.rfind('=')?;
    if eq == 0 || lhs[..eq].chars().next_back().is_some_and(is_operator) {
        return None;
    }
    let (name, _) = last_ident(lhs[..eq].trim_end())?;
    if is_keyword(name) {
        return None;
    }
    Some(name.to_string())
}

/// Dichiarazione in stile C: Java, C, C++ (e ripiego di altri linguaggi).
///
/// Il nome è l'ultimo identificatore prima della prima `(`; la riga deve
/// terminare con `{` (definizione) o `;` (prototipo). Le chiamate vengono
/// scartate perché il nome è preceduto da `.`, `=`, `return`, ….
fn c_like_function(line: &str) -> Option<String> {
    let end = line.trim_end();
    if !(end.ends_with('{') || end.ends_with(';')) {
        return None;
    }
    let paren = line.find('(')?;
    let before = line[..paren].trim_end();
    let (name, start) = last_ident(before)?;
    if is_keyword(name) {
        return None;
    }

    let mut prev = before[..start].trim_end();
    // `Tipo Metodo::nome(…)`: il `::` è parte della dichiarazione, non una chiamata.
    if let Some(stripped) = prev.strip_suffix("::") {
        prev = stripped.trim_end();
    }
    if prev.ends_with('.') || prev.ends_with("->") || prev.ends_with("::") {
        return None;
    }
    if prev.contains('=') || prev.contains('!') {
        return None;
    }
    // Un `;` senza tipo prima del nome è una chiamata, non un prototipo.
    if end.ends_with(';') && prev.is_empty() {
        return None;
    }
    if let Some(c) = prev.chars().next_back() {
        if is_operator(c) {
            return None;
        }
    }
    // `return foo(…)`, `new Foo(…)`, `throw …`: il nome è preceduto da una
    // parola chiave.
    if let Some(word) = last_ident(prev).map(|(w, _)| w) {
        if is_control(word) {
            return None;
        }
    }
    Some(name.to_string())
}

/// Funzioni di shell: `nome() { … }` o `function nome`.
fn shell_function(line: &str) -> Option<String> {
    if let Some(at) = find_word(line, "function") {
        let rest = line[at + "function".len()..].trim_start();
        if let Some(name) = first_ident(rest) {
            return Some(name.to_string());
        }
    }
    c_like_function(line)
}

/// Caratteri che, prima del nome, indicano un'espressione e non una firma.
fn is_operator(c: char) -> bool {
    matches!(
        c,
        '=' | '.' | ',' | '(' | '!' | '<' | '>' | '+' | '/' | '%' | '?' | ':' | '-' | '|'
    )
}

/// Parole chiave che non possono essere il *nome* di una definizione.
fn is_keyword(word: &str) -> bool {
    is_control(word)
        || matches!(
            word,
            "fn" | "func"
                | "def"
                | "function"
                | "class"
                | "struct"
                | "enum"
                | "interface"
                | "union"
                | "namespace"
                | "impl"
                | "trait"
                | "package"
                | "import"
                | "using"
                | "typedef"
                | "template"
                | "let"
                | "var"
                | "const"
                | "static"
                | "public"
                | "private"
                | "protected"
                | "final"
                | "abstract"
                | "virtual"
                | "override"
                | "explicit"
                | "operator"
        )
}

/// Parole chiave di controllo: se precedono il nome, non è una definizione.
fn is_control(word: &str) -> bool {
    matches!(
        word,
        "if" | "else"
            | "for"
            | "while"
            | "switch"
            | "case"
            | "catch"
            | "do"
            | "return"
            | "new"
            | "delete"
            | "throw"
            | "throws"
            | "sizeof"
            | "typeof"
            | "assert"
            | "yield"
            | "await"
            | "in"
            | "of"
            | "instanceof"
            | "synchronized"
            | "match"
            | "when"
            | "with"
            | "try"
            | "select"
            | "go"
            | "defer"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(text: &str, language: Language) -> Vec<(usize, String)> {
        function_symbols(text, language)
            .into_iter()
            .map(|symbol| (symbol.line, symbol.name))
            .collect()
    }

    #[test]
    fn rust_riconosce_fn_con_modificatori() {
        let text = "pub fn uno() {\n}\n\nasync unsafe fn due<T>(x: T) -> T {\n    x\n}\n";
        assert_eq!(
            names(text, Language::Rust),
            vec![(0, "uno".into()), (3, "due".into())]
        );
    }

    #[test]
    fn rust_ignora_i_puntatori_a_funzione() {
        let text = "let f: fn(i32) -> i32 = raddoppia;\n";
        assert!(names(text, Language::Rust).is_empty());
    }

    #[test]
    fn python_riconosce_def_anche_async() {
        let text = "class A:\n    def uno(self):\n        pass\n\nasync def due():\n    pass\n";
        assert_eq!(
            names(text, Language::Python),
            vec![(1, "uno".into()), (4, "due".into())]
        );
    }

    #[test]
    fn go_riconosce_funzioni_e_metodi() {
        let text = "func uno() {\n}\n\nfunc (r T) Due() error {\n\treturn nil\n}\n";
        assert_eq!(
            names(text, Language::Go),
            vec![(0, "uno".into()), (3, "Due".into())]
        );
    }

    #[test]
    fn java_riconosce_i_metodi_e_scarta_i_controlli() {
        let text = "public class A {\n    public static void main(String[] args) {\n        if (args.length > 0) {\n            System.out.println(args[0]);\n        }\n    }\n\n    int somma(int a, int b) {\n        return a + b;\n    }\n}\n";
        assert_eq!(
            names(text, Language::Java),
            vec![(1, "main".into()), (7, "somma".into())]
        );
    }

    #[test]
    fn c_riconosce_le_definizioni_e_i_prototipi() {
        let text = "int foo(int a);\n\nstatic char *bar(void) {\n    return 0;\n}\n";
        assert_eq!(
            names(text, Language::C),
            vec![(0, "foo".into()), (2, "bar".into())]
        );
    }

    #[test]
    fn javascript_riconosce_function_arrow_e_metodi() {
        let text = "function uno() {}\nconst due = (x) => {\n  return x;\n};\nclass A {\n  tre() {\n    return 3;\n  }\n}\n";
        assert_eq!(
            names(text, Language::JavaScript),
            vec![(0, "uno".into()), (1, "due".into()), (5, "tre".into())]
        );
    }

    #[test]
    fn le_chiamate_non_sono_definizioni() {
        let text = "return calcola(1);\nx = calcola(2);\nobj.metodo(3);\nnuovo(4);\n";
        assert!(names(text, Language::JavaScript).is_empty());
    }

    #[test]
    fn shell_riconosce_le_funzioni() {
        let text = "saluta() {\n  echo ciao\n}\n\nfunction addio {\n  echo addio\n}\n";
        assert_eq!(
            names(text, Language::Shell),
            vec![(0, "saluta".into()), (4, "addio".into())]
        );
    }

    #[test]
    fn i_commenti_vengono_saltati() {
        let text = "// fn finta() {\n# def finta():\n* fn pure()\n";
        assert!(names(text, Language::Rust).is_empty());
        assert!(names(text, Language::Python).is_empty());
    }

    #[test]
    fn i_linguaggi_non_di_codice_non_producono_simboli() {
        let text = "# Titolo\n\nTesto con `fn finta()`\n";
        assert!(names(text, Language::Markdown).is_empty());
    }
}
