//! Conversão `Loc` ↔ `Position`, compartilhada por `diagnostics.rs`,
//! `analysis.rs` e `completion.rs` (PRD.md, T48/T49/T50).
//!
//! `ast::Loc` é 1-indexado e `col` conta **caracteres Unicode** (o lexer
//! itera `.chars()`, não bytes — `lexer.rs`, `Lexer::advance`). O LSP é
//! 0-indexado e `Position.character` conta **unidades de código UTF-16**
//! (`positionEncoding: "utf-16"`, `main.rs` — o único valor que o cliente
//! VS Code real aceita, `vscode-languageclient` derruba a conexão em
//! qualquer outro). Para a maioria dos caracteres (todo o BMP, o que cobre
//! acentos do português) as duas contagens coincidem char a char, mas um
//! caractere fora do BMP (emoji, por exemplo) ocupa 2 unidades UTF-16 e só
//! 1 char — daí a conversão precisa do texto da linha, não só de um
//! deslocamento aritmético.

use titanc::ast::Loc;
use tower_lsp::lsp_types::Position;

/// `Loc.col` (1-indexado, chars) → `Position.character` (0-indexado,
/// unidades UTF-16) na `line` dada.
fn char_col_to_utf16(line: &str, col: usize) -> u32 {
    line.chars()
        .take(col.saturating_sub(1))
        .map(|c| c.len_utf16() as u32)
        .sum()
}

/// `Position.character` (0-indexado, unidades UTF-16) → índice de char
/// 0-indexado na `line` dada (quantos `char`s vêm antes dessa unidade
/// UTF-16). Uma unidade UTF-16 que cai no meio de um caractere
/// surrogate-pair conta como se apontasse para o caractere que a contém
/// (mesma tolerância que editores costumam ter nessa borda).
pub fn utf16_col_to_char_index(line: &str, character: u32) -> usize {
    let mut utf16_used = 0u32;
    for (char_idx, c) in line.chars().enumerate() {
        if utf16_used >= character {
            return char_idx;
        }
        utf16_used += c.len_utf16() as u32;
    }
    line.chars().count()
}

/// `Position.character` (0-indexado, unidades UTF-16) → `Loc.col`
/// (1-indexado, chars) na `line` dada.
fn utf16_col_to_char(line: &str, character: u32) -> usize {
    utf16_col_to_char_index(line, character) + 1
}

/// Converte `Loc` para `Position`, buscando a linha correspondente em
/// `source` para a conversão char→UTF-16. `source` deve ser o mesmo buffer
/// de onde `loc` foi extraído.
pub fn loc_to_position(source: &str, loc: &Loc) -> Position {
    let line_text = source.split('\n').nth(loc.line.saturating_sub(1)).unwrap_or("");
    Position {
        line: loc.line.saturating_sub(1) as u32,
        character: char_col_to_utf16(line_text, loc.col),
    }
}

/// Converte `Position` para `Loc`, buscando a linha correspondente em
/// `source` para a conversão UTF-16→char.
pub fn position_to_loc(source: &str, pos: &Position) -> Loc {
    let line_text = source.split('\n').nth(pos.line as usize).unwrap_or("");
    Loc {
        line: pos.line as usize + 1,
        col: utf16_col_to_char(line_text, pos.character),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_char_e_utf16_coincidem() {
        let source = "local x: integer = 1";
        let loc = Loc { line: 1, col: 7 };
        let pos = loc_to_position(source, &loc);
        assert_eq!(pos, Position { line: 0, character: 6 });
        assert_eq!(position_to_loc(source, &pos), loc);
    }

    #[test]
    fn acento_do_bmp_conta_uma_unidade_utf16_por_char() {
        // 'preço': 'ç' é BMP — 1 char, 1 unidade UTF-16 — então a coluna
        // depois dele em char e em UTF-16 é a mesma.
        let source = "local preço: integer = 1";
        let loc = Loc { line: 1, col: 12 }; // logo depois de "preço"
        let pos = loc_to_position(source, &loc);
        assert_eq!(pos.character, 11);
        assert_eq!(position_to_loc(source, &pos), loc);
    }

    #[test]
    fn caractere_fora_do_bmp_ocupa_duas_unidades_utf16() {
        // '🦀' fora do BMP: 1 char, 2 unidades UTF-16. Um `Loc` apontando
        // para a coluna logo depois dele precisa virar `character` 2
        // unidades à frente de onde o char começa, não 1.
        let source = "local 🦀x: integer = 1";
        // "local " = 6 chars, '🦀' é o 7º char (col 7), 'x' é o 8º (col 8).
        let loc_after_crab = Loc { line: 1, col: 8 };
        let pos = loc_to_position(source, &loc_after_crab);
        // 6 unidades UTF-16 de "local " + 2 unidades do '🦀' = 8.
        assert_eq!(pos.character, 8);
        assert_eq!(position_to_loc(source, &pos), loc_after_crab);
    }
}
