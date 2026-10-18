//! Piegatura (*folding*) dei blocchi di codice delimitati da graffe.
//!
//! Un blocco `{ … }` che occupa più righe si può "piegare": le righe interne
//! spariscono e restano visibili solo la riga di apertura e quella di chiusura.
//! Tutta la logica sta qui, senza GUI, così è testabile a parte.
//!
//! Il conteggio delle graffe è **ingenuo** (come quello di
//! [`crate::editor::bracket_match`]): non conosce stringhe, commenti né
//! *raw string*, quindi in rari casi può vedere blocchi che il compilatore non
//! vede. Per un uso "da editor" è più che sufficiente e non costa nulla.

use std::ops::Range;

/// Un blocco di codice delimitato da una `{` e dalla sua `}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FoldBlock {
    /// Offset in byte della `{` che apre il blocco.
    pub open_byte: usize,
    /// Riga (0-based) della `{`.
    pub open_line: usize,
    /// Offset in byte della `}` che chiude il blocco.
    pub close_byte: usize,
    /// Riga (0-based) della `}`.
    pub close_line: usize,
}

impl FoldBlock {
    /// Le righe nascoste quando il blocco è piegato: quelle *fra* le graffe.
    pub fn hidden_lines(&self) -> Range<usize> {
        (self.open_line + 1)..self.close_line
    }

    /// `true` se piegare il blocco nasconde almeno una riga.
    ///
    /// Un blocco scritto tutto su una riga (o con la chiusura subito sotto)
    /// non ha nulla da nascondere e viene ignorato.
    pub fn is_foldable(&self) -> bool {
        self.close_line > self.open_line + 1
    }
}

/// Tutti i blocchi `{ … }` pieghevoli del testo, ordinati per posizione.
///
/// Ogni `{` viene accoppiata con la `}` corrispondente secondo la convenzione
/// "a pila" (l'ultima aperta è la prima chiusa); le graffe spaiate vengono
/// semplicemente ignorate.
pub fn fold_blocks(text: &str) -> Vec<FoldBlock> {
    let mut stack: Vec<(usize, usize)> = Vec::new(); // (byte, riga) delle `{`
    let mut blocks = Vec::new();
    let mut line = 0usize;

    for (byte, ch) in text.char_indices() {
        match ch {
            '{' => stack.push((byte, line)),
            '}' => {
                if let Some((open_byte, open_line)) = stack.pop() {
                    let block = FoldBlock {
                        open_byte,
                        open_line,
                        close_byte: byte,
                        close_line: line,
                    };
                    if block.is_foldable() {
                        blocks.push(block);
                    }
                }
            }
            '\n' => line += 1,
            _ => {}
        }
    }

    blocks.sort_by_key(|block| block.open_byte);
    blocks
}

/// Ordina e unisce intervalli di righe eventualmente sovrapposti.
///
/// Le piegature possono essere annidate: gli intervalli che ne derivano si
/// sovrappongono e vanno fusi prima di contarli o interrogarli.
pub fn merge_ranges(ranges: &mut Vec<Range<usize>>) {
    ranges.retain(|range| range.start < range.end);
    ranges.sort_by_key(|range| range.start);
    let mut merged: Vec<Range<usize>> = Vec::with_capacity(ranges.len());
    for range in ranges.drain(..) {
        match merged.last_mut() {
            Some(last) if range.start <= last.end => {
                if range.end > last.end {
                    last.end = range.end;
                }
            }
            _ => merged.push(range),
        }
    }
    *ranges = merged;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trova_i_blocchi_pieghevoli() {
        let text = "fn main() {\n    let x = 1;\n    println!(\"{x}\");\n}\n";
        let blocks = fold_blocks(text);
        assert_eq!(blocks.len(), 1);
        let block = blocks[0];
        assert_eq!(block.open_line, 0);
        assert_eq!(block.close_line, 3);
        assert_eq!(block.hidden_lines(), 1..3);
        assert!(block.is_foldable());
    }

    #[test]
    fn un_blocco_su_una_sola_riga_non_si_piega() {
        assert!(fold_blocks("fn main() {}\n").is_empty());
        // Chiusura subito sotto: non c'è nessuna riga da nascondere.
        assert!(fold_blocks("fn main() {\n}\n").is_empty());
    }

    #[test]
    fn i_blocchi_annidati_si_sovrappongono_e_si_fondono() {
        let text = "a {\n    b {\n        c\n    }\n    d\n}\n";
        let blocks = fold_blocks(text);
        // Il blocco interno (riga 1) e quello esterno (riga 0).
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].open_line, 0);
        assert_eq!(blocks[1].open_line, 1);

        let mut hidden: Vec<Range<usize>> = blocks.iter().map(|b| b.hidden_lines()).collect();
        assert_eq!(hidden, vec![1..5, 2..3]);
        merge_ranges(&mut hidden);
        assert_eq!(hidden, vec![1..5]);
    }

    #[test]
    fn le_graffe_spaiate_vengono_ignorate() {
        let text = "}\n{\n    x\n";
        assert!(fold_blocks(text).is_empty());
    }

    #[test]
    fn gli_intervalli_vuoti_spariscono_nell_unione() {
        let mut ranges = vec![2..2, 0..1];
        merge_ranges(&mut ranges);
        assert_eq!(ranges, vec![0..1]);
    }
}
