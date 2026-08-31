//! Análise léxica do Titan.
//!
//! Porta manual de `titan/titan-compiler/lexer.lua`: o original usa LPeg
//! (combinadores de PEG) para descrever os tokens; aqui a mesma gramática de
//! tokens vira um lexer de varredura manual sobre `char`s. Cobre o subconjunto
//! da Fase 0 (T3 do PRD.md): palavras-chave `function local return end true
//! false nil`, as reservadas de tipo `boolean integer float string value`,
//! literais inteiros/float, strings curtas com escapes e long strings
//! `[[...]]`, os símbolos `( ) { } , : ; .. =`, e comentários `--` / `--[[ ]]`.
//! O símbolo `=` foi acrescentado na T4 (`parser.rs`), para suportar
//! `local x [: T] = exp` — o `lexer.lua` original já o reconhece, T3 só não
//! precisava dele ainda.
//!
//! A Fase 1 (T10 do PRD.md) acrescenta os operadores aritméticos
//! `+ - * / % ^`, os relacionais `== ~= < > <= >=` e as palavras-chave de
//! controle de fluxo e lógicas `and or not if then elseif else while do for`.
//! `~` só existe em `~=` — isolado é erro léxico (sem bitwise nesta fase).
//!
//! A Fase 2 (T20 do PRD.md) acrescenta `[ ] . #` e as palavras-chave
//! `record`/`as`, usados por arrays, records e maps. `as` deixa de poder ser
//! usado como identificador (é reservada no original também). Um `[` só vira
//! `LBracket` quando não abre uma long string/comment (`[[...]]` ou `[=*[`) —
//! a mesma checagem de `long_bracket_level` que já existe para strings tem
//! precedência, exatamente como no Lua/Titan original. O único caso ambíguo
//! seria `a[[b]]` (que nenhum programa Titan válido escreve), documentado com
//! teste como long string.
//!
//! A Fase 3 (T34 do PRD.md) acrescenta a palavra-chave `import`, usada para
//! trazer uma capability para o programa. Como `as`, deixa de poder ser usada
//! como identificador.

use crate::ast::Loc;

/// Um token com sua posição de início no fonte.
#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub loc: Loc,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    // Literais
    Integer(i64),
    Float(f64),
    String(String),
    Name(String),

    // Palavras-chave
    Function,
    Local,
    Return,
    End,
    True,
    False,
    Nil,

    // Palavras-chave lógicas e de controle de fluxo (Fase 1)
    And,
    Or,
    Not,
    If,
    Then,
    Elseif,
    Else,
    While,
    Do,
    For,

    // Reservadas de tipo (`lexer.lua:170`)
    KwBoolean,
    KwInteger,
    KwFloat,
    KwString,
    KwValue,

    // Palavras-chave de tipos compostos (Fase 2)
    KwRecord,
    KwAs,

    // Palavra-chave de capabilities (Fase 3)
    KwImport,

    // Símbolos
    LParen,
    RParen,
    LCurly,
    RCurly,
    LBracket, // [
    RBracket, // ]
    Comma,
    Colon,
    Semicolon,
    Dot,    // .
    Concat, // ..
    Assign, // =
    Hash,   // #

    // Operadores aritméticos (Fase 1)
    Plus,    // +
    Minus,   // -
    Star,    // *
    Slash,   // /
    Percent, // %
    Caret,   // ^

    // Operadores relacionais (Fase 1)
    Eq, // ==
    Ne, // ~=
    Lt, // <
    Gt, // >
    Le, // <=
    Ge, // >=

    Eof,
}

/// Palavras-chave do léxico e o `TokenKind` que cada uma vira — fonte única
/// de verdade para `lex_name_or_keyword` e para quem mais precisa da lista
/// (ex.: autocomplete do LSP, `titan-lsp/src/completion.rs`), evitando uma
/// segunda tabela desatualizável na mão.
pub const KEYWORDS: &[(&str, TokenKind)] = &[
    ("function", TokenKind::Function),
    ("local", TokenKind::Local),
    ("return", TokenKind::Return),
    ("end", TokenKind::End),
    ("true", TokenKind::True),
    ("false", TokenKind::False),
    ("nil", TokenKind::Nil),
    ("and", TokenKind::And),
    ("or", TokenKind::Or),
    ("not", TokenKind::Not),
    ("if", TokenKind::If),
    ("then", TokenKind::Then),
    ("elseif", TokenKind::Elseif),
    ("else", TokenKind::Else),
    ("while", TokenKind::While),
    ("do", TokenKind::Do),
    ("for", TokenKind::For),
    ("boolean", TokenKind::KwBoolean),
    ("integer", TokenKind::KwInteger),
    ("float", TokenKind::KwFloat),
    ("string", TokenKind::KwString),
    ("value", TokenKind::KwValue),
    ("record", TokenKind::KwRecord),
    ("as", TokenKind::KwAs),
    ("import", TokenKind::KwImport),
];

/// Erro léxico com posição (`lexer.lua` reporta via `lpeglabel`; aqui viramos
/// um `Result` comum — nunca panic).
#[derive(Debug, Clone, PartialEq)]
pub struct LexError {
    pub message: String,
    pub loc: Loc,
}

impl std::fmt::Display for LexError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "erro léxico (linha {}, coluna {}): {}",
            self.loc.line, self.loc.col, self.message
        )
    }
}

impl std::error::Error for LexError {}

/// Cursor sobre os `char`s do fonte, rastreando linha/coluna.
struct Lexer<'a> {
    chars: std::iter::Peekable<std::str::Chars<'a>>,
    line: usize,
    col: usize,
}

impl<'a> Lexer<'a> {
    fn new(source: &'a str) -> Self {
        Lexer {
            chars: source.chars().peekable(),
            line: 1,
            col: 1,
        }
    }

    fn loc(&self) -> Loc {
        Loc {
            line: self.line,
            col: self.col,
        }
    }

    fn peek(&mut self) -> Option<char> {
        self.chars.peek().copied()
    }

    fn peek2(&self) -> Option<char> {
        let mut it = self.chars.clone();
        it.next();
        it.next()
    }

    fn advance(&mut self) -> Option<char> {
        let c = self.chars.next()?;
        if c == '\n' {
            self.line += 1;
            self.col = 1;
        } else {
            self.col += 1;
        }
        Some(c)
    }

    fn starts_with(&self, s: &str) -> bool {
        let mut it = self.chars.clone();
        for expected in s.chars() {
            match it.next() {
                Some(c) if c == expected => {}
                _ => return false,
            }
        }
        true
    }

    /// Consome espaços, quebras de linha e comentários (`--` e `--[[ ]]`).
    fn skip_trivia(&mut self) -> Result<(), LexError> {
        loop {
            match self.peek() {
                Some(c) if c.is_whitespace() => {
                    self.advance();
                }
                Some('-') if self.starts_with("--") => {
                    self.advance();
                    self.advance();
                    if self.peek() == Some('[') && self.long_bracket_level().is_some() {
                        let loc = self.loc();
                        self.consume_long_bracket(loc)?;
                    } else {
                        while let Some(c) = self.peek() {
                            if c == '\n' {
                                break;
                            }
                            self.advance();
                        }
                    }
                }
                _ => break,
            }
        }
        Ok(())
    }

    /// Se o cursor está em `[=*[`, retorna o nível (quantidade de `=`) sem
    /// consumir nada; caso contrário `None`.
    fn long_bracket_level(&self) -> Option<usize> {
        let mut it = self.chars.clone();
        if it.next() != Some('[') {
            return None;
        }
        let mut level = 0;
        loop {
            match it.next() {
                Some('=') => level += 1,
                Some('[') => return Some(level),
                _ => return None,
            }
        }
    }

    /// Consome um long string/comment `[=*[ ... ]=*]` já confirmado por
    /// `long_bracket_level`. `start` é a posição de abertura, usada no erro
    /// de string não terminada.
    fn consume_long_bracket(&mut self, start: Loc) -> Result<String, LexError> {
        let level = self
            .long_bracket_level()
            .expect("chamado só quando confirmado");
        self.advance(); // '['
        for _ in 0..level {
            self.advance(); // '='
        }
        self.advance(); // '['

        // Titan/Lua descartam uma quebra de linha logo após a abertura.
        if self.peek() == Some('\r') {
            self.advance();
            if self.peek() == Some('\n') {
                self.advance();
            }
        } else if self.peek() == Some('\n') {
            self.advance();
        }

        let close: String = std::iter::once(']')
            .chain(std::iter::repeat_n('=', level))
            .chain(std::iter::once(']'))
            .collect();

        let mut contents = String::new();
        loop {
            if self.peek().is_none() {
                return Err(LexError {
                    message: "long string ou long comment não terminado.".to_string(),
                    loc: start,
                });
            }
            if self.starts_with(&close) {
                for _ in 0..close.chars().count() {
                    self.advance();
                }
                return Ok(contents);
            }
            contents.push(self.advance().expect("checado acima"));
        }
    }

    fn lex_number(&mut self) -> Result<TokenKind, LexError> {
        let start = self.loc();
        let mut text = String::new();

        while let Some(c) = self.peek() {
            if c.is_ascii_digit() {
                text.push(c);
                self.advance();
            } else {
                break;
            }
        }

        let mut is_float = false;

        if self.peek() == Some('.') && self.peek2() != Some('.') {
            is_float = true;
            text.push('.');
            self.advance();
            while let Some(c) = self.peek() {
                if c.is_ascii_digit() {
                    text.push(c);
                    self.advance();
                } else {
                    break;
                }
            }
        }

        if matches!(self.peek(), Some('e') | Some('E')) {
            is_float = true;
            text.push(self.advance().expect("checado acima"));
            if matches!(self.peek(), Some('+') | Some('-')) {
                text.push(self.advance().expect("checado acima"));
            }
            let mut has_digits = false;
            while let Some(c) = self.peek() {
                if c.is_ascii_digit() {
                    has_digits = true;
                    text.push(c);
                    self.advance();
                } else {
                    break;
                }
            }
            if !has_digits {
                return Err(LexError {
                    message: "número malformado.".to_string(),
                    loc: start,
                });
            }
        }

        // Um identificador colado ao número (`1337require`) é malformado —
        // mesmo espírito de `lexer.lua:30-46`.
        if let Some(c) = self.peek()
            && (c.is_alphabetic() || c == '_')
        {
            return Err(LexError {
                message: "número malformado.".to_string(),
                loc: start,
            });
        }

        if is_float {
            text.parse::<f64>()
                .map(TokenKind::Float)
                .map_err(|_| LexError {
                    message: "número malformado.".to_string(),
                    loc: start,
                })
        } else {
            text.parse::<i64>()
                .map(TokenKind::Integer)
                .map_err(|_| LexError {
                    message: "número malformado.".to_string(),
                    loc: start,
                })
        }
    }

    fn lex_short_string(&mut self) -> Result<String, LexError> {
        let start = self.loc();
        let delimiter = self.advance().expect("chamado só com aspa presente");
        let mut contents = String::new();

        loop {
            match self.peek() {
                None => {
                    return Err(LexError {
                        message: "string não terminada.".to_string(),
                        loc: start,
                    });
                }
                Some('\n') => {
                    return Err(LexError {
                        message: "string não terminada.".to_string(),
                        loc: start,
                    });
                }
                Some(c) if c == delimiter => {
                    self.advance();
                    return Ok(contents);
                }
                Some('\\') => {
                    let esc_loc = self.loc();
                    self.advance();
                    match self.peek() {
                        None => {
                            return Err(LexError {
                                message: "string não terminada.".to_string(),
                                loc: start,
                            });
                        }
                        Some('a') => {
                            contents.push('\u{7}');
                            self.advance();
                        }
                        Some('b') => {
                            contents.push('\u{8}');
                            self.advance();
                        }
                        Some('f') => {
                            contents.push('\u{c}');
                            self.advance();
                        }
                        Some('n') => {
                            contents.push('\n');
                            self.advance();
                        }
                        Some('r') => {
                            contents.push('\r');
                            self.advance();
                        }
                        Some('t') => {
                            contents.push('\t');
                            self.advance();
                        }
                        Some('v') => {
                            contents.push('\u{b}');
                            self.advance();
                        }
                        Some('\\') => {
                            contents.push('\\');
                            self.advance();
                        }
                        Some('\'') => {
                            contents.push('\'');
                            self.advance();
                        }
                        Some('"') => {
                            contents.push('"');
                            self.advance();
                        }
                        Some('\n') => {
                            contents.push('\n');
                            self.advance();
                        }
                        Some(other) => {
                            return Err(LexError {
                                message: format!("sequência de escape inválida '\\{other}'."),
                                loc: esc_loc,
                            });
                        }
                    }
                }
                Some(_) => {
                    contents.push(self.advance().expect("checado acima"));
                }
            }
        }
    }

    fn lex_name_or_keyword(&mut self) -> TokenKind {
        let mut text = String::new();
        while let Some(c) = self.peek() {
            if c.is_alphanumeric() || c == '_' {
                text.push(c);
                self.advance();
            } else {
                break;
            }
        }

        match KEYWORDS.iter().find(|(kw, _)| *kw == text) {
            Some((_, kind)) => kind.clone(),
            None => TokenKind::Name(text),
        }
    }

    fn next_token(&mut self) -> Result<Token, LexError> {
        self.skip_trivia()?;
        let loc = self.loc();

        let Some(c) = self.peek() else {
            return Ok(Token {
                kind: TokenKind::Eof,
                loc,
            });
        };

        if c.is_ascii_digit() {
            let kind = self.lex_number()?;
            return Ok(Token { kind, loc });
        }

        if c.is_alphabetic() || c == '_' {
            let kind = self.lex_name_or_keyword();
            return Ok(Token { kind, loc });
        }

        if c == '"' || c == '\'' {
            let s = self.lex_short_string()?;
            return Ok(Token {
                kind: TokenKind::String(s),
                loc,
            });
        }

        if c == '[' && self.long_bracket_level().is_some() {
            let s = self.consume_long_bracket(loc)?;
            return Ok(Token {
                kind: TokenKind::String(s),
                loc,
            });
        }

        // Símbolos suportados: ( ) { } , : ; .. = e os operadores da Fase 1
        // (+ - * / % ^ == ~= < > <= >=). Ambiguidades resolvidas com o mesmo
        // lookahead de 1 char já usado para `.` vs `..`.
        let kind = match c {
            '(' => {
                self.advance();
                TokenKind::LParen
            }
            ')' => {
                self.advance();
                TokenKind::RParen
            }
            '{' => {
                self.advance();
                TokenKind::LCurly
            }
            '}' => {
                self.advance();
                TokenKind::RCurly
            }
            '[' => {
                self.advance();
                TokenKind::LBracket
            }
            ']' => {
                self.advance();
                TokenKind::RBracket
            }
            ',' => {
                self.advance();
                TokenKind::Comma
            }
            ':' => {
                self.advance();
                TokenKind::Colon
            }
            ';' => {
                self.advance();
                TokenKind::Semicolon
            }
            '#' => {
                self.advance();
                TokenKind::Hash
            }
            '.' if self.peek2() == Some('.') => {
                self.advance();
                self.advance();
                TokenKind::Concat
            }
            '.' => {
                self.advance();
                TokenKind::Dot
            }
            '+' => {
                self.advance();
                TokenKind::Plus
            }
            // Um `-` que chega aqui nunca inicia `--`: `skip_trivia` já teria
            // consumido o comentário.
            '-' => {
                self.advance();
                TokenKind::Minus
            }
            '*' => {
                self.advance();
                TokenKind::Star
            }
            '/' => {
                self.advance();
                TokenKind::Slash
            }
            '%' => {
                self.advance();
                TokenKind::Percent
            }
            '^' => {
                self.advance();
                TokenKind::Caret
            }
            '=' if self.peek2() == Some('=') => {
                self.advance();
                self.advance();
                TokenKind::Eq
            }
            '=' => {
                self.advance();
                TokenKind::Assign
            }
            '<' if self.peek2() == Some('=') => {
                self.advance();
                self.advance();
                TokenKind::Le
            }
            '<' => {
                self.advance();
                TokenKind::Lt
            }
            '>' if self.peek2() == Some('=') => {
                self.advance();
                self.advance();
                TokenKind::Ge
            }
            '>' => {
                self.advance();
                TokenKind::Gt
            }
            '~' if self.peek2() == Some('=') => {
                self.advance();
                self.advance();
                TokenKind::Ne
            }
            // `~` só existe em `~=` — sem bitwise nesta fase.
            '~' => {
                return Err(LexError {
                    message: "'~' isolado não é um operador; use '~=' para desigualdade."
                        .to_string(),
                    loc,
                });
            }
            other => {
                return Err(LexError {
                    message: format!("caractere inesperado '{other}'."),
                    loc,
                });
            }
        };

        Ok(Token { kind, loc })
    }
}

/// Tokeniza um fonte Titan por completo, incluindo o `Eof` final.
///
/// Para no primeiro erro léxico — nunca entra em pânico.
pub fn lex(source: &str) -> Result<Vec<Token>, LexError> {
    let mut lexer = Lexer::new(source);
    let mut tokens = Vec::new();

    loop {
        let token = lexer.next_token()?;
        let is_eof = token.kind == TokenKind::Eof;
        tokens.push(token);
        if is_eof {
            break;
        }
    }

    Ok(tokens)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(source: &str) -> Vec<TokenKind> {
        lex(source)
            .unwrap_or_else(|e| panic!("esperava sucesso, obteve erro: {e}"))
            .into_iter()
            .map(|t| t.kind)
            .collect()
    }

    #[test]
    fn tokeniza_hello_titan() {
        let source = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../examples/hello.titan"
        ))
        .expect("examples/hello.titan deve existir");

        let tokens = kinds(&source);

        assert_eq!(
            tokens,
            vec![
                TokenKind::Function,
                TokenKind::Name("main".to_string()),
                TokenKind::LParen,
                TokenKind::Name("args".to_string()),
                TokenKind::Colon,
                TokenKind::LCurly,
                TokenKind::KwString,
                TokenKind::RCurly,
                TokenKind::RParen,
                TokenKind::Colon,
                TokenKind::KwInteger,
                TokenKind::Name("print".to_string()),
                TokenKind::LParen,
                TokenKind::String("Olá, mundo!".to_string()),
                TokenKind::RParen,
                TokenKind::Return,
                TokenKind::Integer(0),
                TokenKind::End,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn string_com_acentos_utf8_preserva_conteudo() {
        let tokens = kinds(r#""Olá, mundo!""#);
        assert_eq!(
            tokens,
            vec![TokenKind::String("Olá, mundo!".to_string()), TokenKind::Eof]
        );
    }

    #[test]
    fn string_nao_terminada_produz_erro_com_posicao() {
        let err = lex("\"abc").unwrap_err();
        assert_eq!(err.loc, Loc { line: 1, col: 1 });
        assert!(err.message.contains("não terminada"));
    }

    #[test]
    fn string_nao_terminada_por_quebra_de_linha() {
        let err = lex("\"abc\ndef\"").unwrap_err();
        assert_eq!(err.loc, Loc { line: 1, col: 1 });
    }

    #[test]
    fn distingue_inteiro_de_float() {
        assert_eq!(kinds("42"), vec![TokenKind::Integer(42), TokenKind::Eof]);
        assert_eq!(kinds("2.5"), vec![TokenKind::Float(2.5), TokenKind::Eof]);
        assert_eq!(kinds("1e10"), vec![TokenKind::Float(1e10), TokenKind::Eof]);
    }

    #[test]
    fn distingue_ponto_de_concat() {
        assert_eq!(kinds(".."), vec![TokenKind::Concat, TokenKind::Eof]);
        // Um único `.` fora de número não é suportado nesta fase — mas não é
        // exercitado no subconjunto de T3, então não aparece aqui.
    }

    #[test]
    fn concat_nao_confunde_com_numero_float() {
        assert_eq!(
            kinds("1 .. 2"),
            vec![
                TokenKind::Integer(1),
                TokenKind::Concat,
                TokenKind::Integer(2),
                TokenKind::Eof
            ]
        );
    }

    #[test]
    fn reconhece_palavras_chave() {
        assert_eq!(
            kinds("function local return end true false nil"),
            vec![
                TokenKind::Function,
                TokenKind::Local,
                TokenKind::Return,
                TokenKind::End,
                TokenKind::True,
                TokenKind::False,
                TokenKind::Nil,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn reconhece_tipos_reservados() {
        assert_eq!(
            kinds("boolean integer float string value"),
            vec![
                TokenKind::KwBoolean,
                TokenKind::KwInteger,
                TokenKind::KwFloat,
                TokenKind::KwString,
                TokenKind::KwValue,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn identificador_nao_e_confundido_com_palavra_chave_prefixada() {
        // `lexer.lua` original evita casar "local" no meio de "localx".
        assert_eq!(
            kinds("localx"),
            vec![TokenKind::Name("localx".to_string()), TokenKind::Eof]
        );
    }

    #[test]
    fn ignora_comentario_de_linha() {
        assert_eq!(
            kinds("-- comentário\n42"),
            vec![TokenKind::Integer(42), TokenKind::Eof]
        );
    }

    #[test]
    fn ignora_comentario_longo() {
        assert_eq!(
            kinds("--[[ bloco\nde comentário ]]42"),
            vec![TokenKind::Integer(42), TokenKind::Eof]
        );
    }

    #[test]
    fn ignora_comentario_longo_com_nivel_de_igual() {
        assert_eq!(
            kinds("--[==[ ]] ainda comentário ]==]42"),
            vec![TokenKind::Integer(42), TokenKind::Eof]
        );
    }

    #[test]
    fn long_string_preserva_conteudo() {
        assert_eq!(
            kinds("[[texto\ncom quebra]]"),
            vec![
                TokenKind::String("texto\ncom quebra".to_string()),
                TokenKind::Eof
            ]
        );
    }

    #[test]
    fn long_string_com_nivel_de_igual() {
        assert_eq!(
            kinds("[==[ ]] dentro ]==]"),
            vec![TokenKind::String(" ]] dentro ".to_string()), TokenKind::Eof]
        );
    }

    #[test]
    fn long_string_nao_terminada_produz_erro() {
        let err = lex("[[abc").unwrap_err();
        assert!(err.message.contains("não terminado"));
    }

    #[test]
    fn escapes_da_string_curta() {
        assert_eq!(
            kinds(r#""a\nb\tc\"d""#),
            vec![TokenKind::String("a\nb\tc\"d".to_string()), TokenKind::Eof]
        );
    }

    #[test]
    fn escape_invalido_produz_erro_com_posicao() {
        let err = lex(r#""a\qb""#).unwrap_err();
        assert!(err.message.contains("inválida"));
        assert_eq!(err.loc, Loc { line: 1, col: 3 });
    }

    #[test]
    fn simbolos_suportados_na_fase() {
        assert_eq!(
            kinds("(){},:;..="),
            vec![
                TokenKind::LParen,
                TokenKind::RParen,
                TokenKind::LCurly,
                TokenKind::RCurly,
                TokenKind::Comma,
                TokenKind::Colon,
                TokenKind::Semicolon,
                TokenKind::Concat,
                TokenKind::Assign,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn rastreia_linha_e_coluna() {
        let tokens = lex("function\nmain").unwrap();
        assert_eq!(tokens[0].loc, Loc { line: 1, col: 1 });
        assert_eq!(tokens[1].loc, Loc { line: 2, col: 1 });
    }

    #[test]
    fn caractere_inesperado_produz_erro() {
        let err = lex("@").unwrap_err();
        assert!(err.message.contains("inesperado"));
        assert_eq!(err.loc, Loc { line: 1, col: 1 });
    }

    #[test]
    fn numero_malformado_por_identificador_colado() {
        let err = lex("1337require").unwrap_err();
        assert!(err.message.contains("malformado"));
    }

    // ---- Fase 1 (T10): operadores e keywords de controle de fluxo ----

    #[test]
    fn reconhece_operadores_aritmeticos() {
        assert_eq!(
            kinds("+ - * / % ^"),
            vec![
                TokenKind::Plus,
                TokenKind::Minus,
                TokenKind::Star,
                TokenKind::Slash,
                TokenKind::Percent,
                TokenKind::Caret,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn reconhece_operadores_relacionais() {
        assert_eq!(
            kinds("== ~= < > <= >="),
            vec![
                TokenKind::Eq,
                TokenKind::Ne,
                TokenKind::Lt,
                TokenKind::Gt,
                TokenKind::Le,
                TokenKind::Ge,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn reconhece_keywords_logicas_e_de_controle_de_fluxo() {
        assert_eq!(
            kinds("and or not if then elseif else while do for"),
            vec![
                TokenKind::And,
                TokenKind::Or,
                TokenKind::Not,
                TokenKind::If,
                TokenKind::Then,
                TokenKind::Elseif,
                TokenKind::Else,
                TokenKind::While,
                TokenKind::Do,
                TokenKind::For,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn keyword_nova_nao_casa_prefixo_de_identificador() {
        // Mesma garantia do teste `localx`: `forma` não é `for` + `ma`.
        assert_eq!(
            kinds("forma iface ander"),
            vec![
                TokenKind::Name("forma".to_string()),
                TokenKind::Name("iface".to_string()),
                TokenKind::Name("ander".to_string()),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn distingue_assign_de_eq() {
        assert_eq!(
            kinds("x = y == z"),
            vec![
                TokenKind::Name("x".to_string()),
                TokenKind::Assign,
                TokenKind::Name("y".to_string()),
                TokenKind::Eq,
                TokenKind::Name("z".to_string()),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn distingue_menor_de_menor_igual() {
        assert_eq!(
            kinds("a < b <= c"),
            vec![
                TokenKind::Name("a".to_string()),
                TokenKind::Lt,
                TokenKind::Name("b".to_string()),
                TokenKind::Le,
                TokenKind::Name("c".to_string()),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn distingue_maior_de_maior_igual() {
        assert_eq!(
            kinds("a > b >= c"),
            vec![
                TokenKind::Name("a".to_string()),
                TokenKind::Gt,
                TokenKind::Name("b".to_string()),
                TokenKind::Ge,
                TokenKind::Name("c".to_string()),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn operadores_colados_sem_espaco() {
        assert_eq!(
            kinds("a<=b~=c==d"),
            vec![
                TokenKind::Name("a".to_string()),
                TokenKind::Le,
                TokenKind::Name("b".to_string()),
                TokenKind::Ne,
                TokenKind::Name("c".to_string()),
                TokenKind::Eq,
                TokenKind::Name("d".to_string()),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn concat_segue_intacto_apos_novos_operadores() {
        assert_eq!(
            kinds(r#""a" .. "b""#),
            vec![
                TokenKind::String("a".to_string()),
                TokenKind::Concat,
                TokenKind::String("b".to_string()),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn menos_nao_confunde_com_comentario() {
        assert_eq!(
            kinds("1 - 2 -- comentário\n- 3"),
            vec![
                TokenKind::Integer(1),
                TokenKind::Minus,
                TokenKind::Integer(2),
                TokenKind::Minus,
                TokenKind::Integer(3),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn expressao_aritmetica_completa() {
        assert_eq!(
            kinds("(1 + 2) * 3 ^ 4 % 5 / 6"),
            vec![
                TokenKind::LParen,
                TokenKind::Integer(1),
                TokenKind::Plus,
                TokenKind::Integer(2),
                TokenKind::RParen,
                TokenKind::Star,
                TokenKind::Integer(3),
                TokenKind::Caret,
                TokenKind::Integer(4),
                TokenKind::Percent,
                TokenKind::Integer(5),
                TokenKind::Slash,
                TokenKind::Integer(6),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn til_isolado_produz_erro_lexico_com_posicao() {
        let err = lex("a ~ b").unwrap_err();
        assert!(err.message.contains("'~'"));
        assert!(err.message.contains("'~='"));
        assert_eq!(err.loc, Loc { line: 1, col: 3 });
    }

    #[test]
    fn indexacao_v_colchete_1() {
        assert_eq!(
            kinds("v[1]"),
            vec![
                TokenKind::Name("v".to_string()),
                TokenKind::LBracket,
                TokenKind::Integer(1),
                TokenKind::RBracket,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn acesso_a_campo_p_ponto_campo() {
        assert_eq!(
            kinds("p.campo"),
            vec![
                TokenKind::Name("p".to_string()),
                TokenKind::Dot,
                TokenKind::Name("campo".to_string()),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn hash_tamanho_de_v() {
        assert_eq!(
            kinds("#v"),
            vec![
                TokenKind::Hash,
                TokenKind::Name("v".to_string()),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn keyword_record_seguida_de_nome() {
        assert_eq!(
            kinds("record Ponto"),
            vec![
                TokenKind::KwRecord,
                TokenKind::Name("Ponto".to_string()),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn keyword_as_para_cast_de_tipo() {
        assert_eq!(
            kinds("x as integer"),
            vec![
                TokenKind::Name("x".to_string()),
                TokenKind::KwAs,
                TokenKind::KwInteger,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn keyword_import_data() {
        assert_eq!(
            kinds("import data"),
            vec![
                TokenKind::KwImport,
                TokenKind::Name("data".to_string()),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn keyword_import_nao_casa_prefixo_de_identificador() {
        // Mesma garantia do teste `localx`/`forma`: `importante` não é
        // `import` + `ante`.
        assert_eq!(
            kinds("importante"),
            vec![TokenKind::Name("importante".to_string()), TokenKind::Eof]
        );
    }

    #[test]
    fn indexacao_aninhada_a_colchete_b_colchete_1() {
        // `a[b[1]]` — o `[b` faz `long_bracket_level` devolver `None`, então
        // não há ambiguidade com long string aqui: 5 tokens antes do Eof.
        assert_eq!(
            kinds("a[b[1]]"),
            vec![
                TokenKind::Name("a".to_string()),
                TokenKind::LBracket,
                TokenKind::Name("b".to_string()),
                TokenKind::LBracket,
                TokenKind::Integer(1),
                TokenKind::RBracket,
                TokenKind::RBracket,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn colchete_duplo_colado_continua_sendo_long_string_como_no_lua() {
        // `a[[b]]` é ambíguo entre "indexação de indexação" e "long string
        // `[[b]]`"; nenhum programa Titan válido escreve isso, e a precedência
        // é a mesma do Lua: long string ganha. Documentado aqui, não é um
        // caso que o parser precisa desambiguar.
        assert_eq!(
            kinds("a[[b]]"),
            vec![
                TokenKind::Name("a".to_string()),
                TokenKind::String("b".to_string()),
                TokenKind::Eof,
            ]
        );
    }
}
