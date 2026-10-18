//! Formattazione tramite strumenti esterni.
//!
//! Invece di reimplementare un formatter per linguaggio, l'editor invoca lo
//! strumento standard della comunità (`rustfmt`, `prettier`, `black`, …) e gli
//! passa il testo su **stdin**, rileggendo il risultato da **stdout**: è il
//! protocollo che tutti questi tool supportano, e non richiede file temporanei.
//!
//! La chiamata è bloccante (attende il processo): la GUI la esegue su un thread
//! separato per non bloccare il disegno.

use std::fmt;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use crate::syntax::Language;

/// Come invocare un formatter esterno.
#[derive(Debug, Clone, Copy)]
pub struct FormatterSpec {
    /// Eseguibile cercato nel `PATH`.
    pub program: &'static str,
    /// Argomenti.
    pub args: &'static [&'static str],
    /// Cosa scrivere nella barra di stato quando manca.
    pub hint: &'static str,
}

/// Il formatter predefinito per un linguaggio, se ne conosciamo uno.
pub fn formatter_for(language: Language) -> Option<FormatterSpec> {
    match language {
        Language::Rust => Some(FormatterSpec {
            program: "rustfmt",
            args: &["--emit", "stdout", "--edition", "2024"],
            hint: "installalo con: rustup component add rustfmt",
        }),
        Language::Python => Some(FormatterSpec {
            program: "black",
            args: &["-q", "-"],
            hint: "installalo con: pip install black",
        }),
        Language::JavaScript | Language::TypeScript => Some(FormatterSpec {
            program: "prettier",
            args: &["--stdin-filepath", "input.js"],
            hint: "installalo con: npm i -g prettier",
        }),
        Language::Java => Some(FormatterSpec {
            program: "google-java-format",
            args: &["-"],
            hint: "scaricalo da github.com/google/google-java-format",
        }),
        Language::Go => Some(FormatterSpec {
            program: "gofmt",
            args: &[],
            hint: "installalo con l'SDK di Go",
        }),
        Language::Json => Some(FormatterSpec {
            program: "prettier",
            args: &["--stdin-filepath", "input.json"],
            hint: "installalo con: npm i -g prettier",
        }),
        Language::Css => Some(FormatterSpec {
            program: "prettier",
            args: &["--stdin-filepath", "input.css"],
            hint: "installalo con: npm i -g prettier",
        }),
        Language::C | Language::Cpp => Some(FormatterSpec {
            program: "clang-format",
            args: &[],
            hint: "installalo con il pacchetto clang-format",
        }),
        Language::Shell => Some(FormatterSpec {
            program: "shfmt",
            args: &[],
            hint: "installalo con: go install mvdan.cc/sh/v3/cmd/shfmt@latest",
        }),
        _ => None,
    }
}

/// Esito della formattazione.
#[derive(Debug)]
pub enum FormatError {
    /// Non c'è un formatter noto per questo linguaggio.
    Unsupported(Language),
    /// L'eseguibile non è installato / non è nel `PATH`.
    NotFound(FormatterSpec),
    /// Il formatter è partito ma è terminato con errore.
    Failed {
        program: &'static str,
        message: String,
    },
    Io(std::io::Error),
}

impl fmt::Display for FormatError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FormatError::Unsupported(lang) => {
                write!(f, "nessun formatter configurato per {}", lang.name())
            }
            FormatError::NotFound(spec) => write!(
                f,
                "{} non è installato ({})",
                spec.program, spec.hint
            ),
            FormatError::Failed { program, message } => {
                write!(f, "{program} ha segnalato un errore: {message}")
            }
            FormatError::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for FormatError {}

impl From<std::io::Error> for FormatError {
    fn from(e: std::io::Error) -> Self {
        FormatError::Io(e)
    }
}

/// Formatta `text` per il linguaggio indicato.
///
/// Il testo formattato è quello che torna su stdout; se il formatter non cambia
/// nulla, il risultato è identico all'input.
pub fn format_text(language: Language, text: &str) -> Result<String, FormatError> {
    let spec = formatter_for(language).ok_or(FormatError::Unsupported(language))?;
    if !is_available(spec.program) {
        return Err(FormatError::NotFound(spec));
    }

    let mut child = Command::new(spec.program)
        .args(spec.args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    child
        .stdin
        .take()
        .ok_or_else(|| FormatError::Failed {
            program: spec.program,
            message: "impossibile scrivere sullo stdin".into(),
        })?
        .write_all(text.as_bytes())?;

    let out = child.wait_with_output()?;
    if !out.status.success() {
        return Err(FormatError::Failed {
            program: spec.program,
            message: String::from_utf8_lossy(&out.stderr).trim().to_string(),
        });
    }
    let formatted = String::from_utf8(out.stdout)
        .map_err(|_| FormatError::Failed {
            program: spec.program,
            message: "l'output del formatter non è UTF-8".into(),
        })?;
    if formatted.trim().is_empty() {
        return Err(FormatError::Failed {
            program: spec.program,
            message: "output vuoto".into(),
        });
    }
    Ok(formatted)
}

/// `true` se l'eseguibile è nel `PATH`.
pub fn is_available(program: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| {
        let candidate = dir.join(program);
        candidate.is_file() || candidate.with_extension("exe").is_file()
    })
}

/// `true` se il linguaggio ha un formatter e questo è installato.
pub fn can_format(language: Language) -> bool {
    formatter_for(language).is_some_and(|spec| is_available(spec.program))
}

/// Nome del formatter usato per un linguaggio (per la barra di stato).
pub fn formatter_name(language: Language) -> Option<&'static str> {
    formatter_for(language).map(|spec| spec.program)
}

/// Se la formattazione è utile per un file con questo percorso.
pub fn can_format_path(path: &Path) -> bool {
    can_format(Language::from_path(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linguaggi_senza_formatter() {
        assert!(formatter_for(Language::Markdown).is_none());
        assert!(matches!(
            format_text(Language::Markdown, "# ciao"),
            Err(FormatError::Unsupported(_))
        ));
    }

    #[test]
    fn formatter_mancante_viene_segnalato() {
        // Un programma che esiste di sicuro non è `ride-formatter-inesistente`.
        assert!(!is_available("ride-formatter-inesistente"));
    }

    #[test]
    #[cfg(unix)]
    fn formatta_con_un_finto_formatter() {
        // Verifica il protocollo stdin/stdout con `cat`, senza dipendere da
        // formatter realmente installati.
        if !is_available("cat") {
            return;
        }
        let spec = FormatterSpec {
            program: "cat",
            args: &[],
            hint: "",
        };
        assert!(is_available(spec.program));
        let out = run(spec, "fn  main(){}\n").unwrap();
        assert_eq!(out, "fn  main(){}\n");
    }

    fn run(spec: FormatterSpec, text: &str) -> Result<String, FormatError> {
        let mut child = Command::new(spec.program)
            .args(spec.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()?;
        child
            .stdin
            .take()
            .unwrap()
            .write_all(text.as_bytes())?;
        let out = child.wait_with_output()?;
        Ok(String::from_utf8_lossy(&out.stdout).to_string())
    }
}
