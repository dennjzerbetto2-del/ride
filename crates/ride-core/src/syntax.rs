//! Linguaggio di un file e convenzioni di stile associate.
//!
//! Il riconoscimento avviene per estensione: è volutamente semplice e senza
//! dipendenze. L'evidenziazione vera e propria (syntect) vive nella GUI, che a
//! partire dal [`Language`] qui dedotto sceglie la sintassi da usare.

use std::path::Path;

/// Convenzione di indentazione di un linguaggio.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Indent {
    /// `n` spazi per livello.
    Spaces(u8),
    /// Un tab per livello.
    Tabs,
}

impl Indent {
    /// Larghezza in "colonne" di un livello di indentazione.
    pub const fn width(self) -> u8 {
        match self {
            Indent::Spaces(n) => n,
            Indent::Tabs => 1,
        }
    }

    /// La stringa da inserire per un livello di indentazione.
    pub fn unit(self) -> &'static str {
        match self {
            Indent::Spaces(2) => "  ",
            Indent::Spaces(3) => "   ",
            Indent::Spaces(8) => "        ",
            Indent::Spaces(_) => "    ",
            Indent::Tabs => "\t",
        }
    }
}

impl Default for Indent {
    fn default() -> Self {
        Indent::Spaces(4)
    }
}

/// Linguaggi riconosciuti.
///
/// Non serve che sia un elenco esaustivo: [`Language::Text`] è il ripiego e
/// disabilita le operazioni che richiedono di conoscere la sintassi.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Language {
    Rust,
    Python,
    JavaScript,
    TypeScript,
    Java,
    Go,
    Json,
    Toml,
    Yaml,
    Markdown,
    Html,
    Css,
    C,
    Cpp,
    Shell,
    #[default]
    Text,
}

impl Language {
    /// Deduce il linguaggio dal nome del file.
    pub fn from_path(path: &Path) -> Self {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        Self::from_extension(&ext)
    }

    /// Deduce il linguaggio da un'estensione (senza punto).
    pub fn from_extension(ext: &str) -> Self {
        match ext {
            "rs" => Language::Rust,
            "py" | "pyi" => Language::Python,
            "js" | "mjs" | "cjs" | "jsx" => Language::JavaScript,
            "ts" | "tsx" | "mts" | "cts" => Language::TypeScript,
            "java" => Language::Java,
            "go" => Language::Go,
            "json" | "jsonc" => Language::Json,
            "toml" => Language::Toml,
            "yaml" | "yml" => Language::Yaml,
            "md" | "markdown" | "mdx" => Language::Markdown,
            "html" | "htm" => Language::Html,
            "css" | "scss" | "sass" => Language::Css,
            "c" | "h" => Language::C,
            "cc" | "cpp" | "cxx" | "hpp" | "hh" | "hxx" => Language::Cpp,
            "sh" | "bash" | "zsh" | "fish" => Language::Shell,
            _ => Language::Text,
        }
    }

    /// Nome leggibile (mostrato nella status bar).
    pub const fn name(self) -> &'static str {
        match self {
            Language::Rust => "Rust",
            Language::Python => "Python",
            Language::JavaScript => "JavaScript",
            Language::TypeScript => "TypeScript",
            Language::Java => "Java",
            Language::Go => "Go",
            Language::Json => "JSON",
            Language::Toml => "TOML",
            Language::Yaml => "YAML",
            Language::Markdown => "Markdown",
            Language::Html => "HTML",
            Language::Css => "CSS",
            Language::C => "C",
            Language::Cpp => "C++",
            Language::Shell => "Shell",
            Language::Text => "Testo",
        }
    }

    /// Prefisso di commento di riga (`//`, `#`, …).
    pub const fn line_comment(self) -> Option<&'static str> {
        match self {
            Language::Rust
            | Language::JavaScript
            | Language::TypeScript
            | Language::Java
            | Language::Go
            | Language::C
            | Language::Cpp => Some("//"),
            Language::Python | Language::Toml | Language::Yaml | Language::Shell => Some("#"),
            Language::Html => Some("<!--"),
            Language::Css => Some("/*"),
            _ => None,
        }
    }

    /// Delimitatori di commento a blocco.
    pub const fn block_comment(self) -> Option<(&'static str, &'static str)> {
        match self {
            Language::Rust
            | Language::JavaScript
            | Language::TypeScript
            | Language::Java
            | Language::Go
            | Language::C
            | Language::Cpp
            | Language::Css => Some(("/*", "*/")),
            Language::Html => Some(("<!--", "-->")),
            _ => None,
        }
    }

    /// Indentazione convenzionale.
    pub const fn indent(self) -> Indent {
        match self {
            Language::Python => Indent::Spaces(4),
            // Go usa i tab per convenzione (`gofmt`).
            Language::Go => Indent::Tabs,
            Language::Rust
            | Language::JavaScript
            | Language::TypeScript
            | Language::Java
            | Language::Json
            | Language::Yaml
            | Language::C
            | Language::Cpp
            | Language::Css => Indent::Spaces(4),
            Language::Html | Language::Markdown => Indent::Spaces(2),
            Language::Toml | Language::Shell => Indent::Spaces(4),
            Language::Text => Indent::Spaces(4),
        }
    }

    /// `true` se le operazioni basate su sintassi (es. indentazione) hanno senso.
    pub const fn is_code(self) -> bool {
        !matches!(self, Language::Text | Language::Markdown)
    }

    /// Nome del linguaggio usato da syntect, se diverso dal nome visualizzato.
    pub const fn syntect_hint(self) -> Option<&'static str> {
        match self {
            Language::Shell => Some("Bourne Again Shell (bash)"),
            Language::Text => None,
            _ => Some(self.name()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn riconosce_le_estensioni() {
        assert_eq!(Language::from_path(&PathBuf::from("src/main.rs")), Language::Rust);
        assert_eq!(Language::from_path(&PathBuf::from("a/b/c.PY")), Language::Python);
        assert_eq!(Language::from_path(&PathBuf::from("src/Main.java")), Language::Java);
        assert_eq!(Language::from_path(&PathBuf::from("cmd/main.go")), Language::Go);
        assert_eq!(Language::from_path(&PathBuf::from("Cargo.toml")), Language::Toml);
        assert_eq!(Language::from_path(&PathBuf::from("senza_estensione")), Language::Text);
    }

    #[test]
    fn unita_di_indentazione() {
        assert_eq!(Indent::Spaces(4).unit(), "    ");
        assert_eq!(Indent::Spaces(2).unit(), "  ");
        assert_eq!(Indent::Tabs.unit(), "\t");
    }

    #[test]
    fn convenzioni_di_java_e_go() {
        assert_eq!(Language::Java.line_comment(), Some("//"));
        assert_eq!(Language::Go.line_comment(), Some("//"));
        // Go indenta con i tab (`gofmt`), Java con quattro spazi.
        assert_eq!(Language::Go.indent(), Indent::Tabs);
        assert_eq!(Language::Java.indent(), Indent::Spaces(4));
    }
}
