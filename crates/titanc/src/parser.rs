//! Análise sintática do Titan.
//!
//! Substitui a gramática PEG de `titan/titan-compiler/parser.lua` (575
//! linhas) por um parser de descida recursiva. Cobre o subconjunto da Fase 0
//! (T4 do PRD.md):
//!
//! ```text
//! [local] function nome(p: T, ...) [: TipoRetorno] ... end
//! local x [: T] = exp
//! ```
//!
//! Statements: `StatCall`, `StatReturn`, `StatDecl`.
//! Expressões: `ExpString`, `ExpInteger`, `ExpFloat`, `ExpBool`, `ExpNil`,
//! `ExpVar`, `ExpCall`, `ExpConcat` (`..`).
//! Tipos: `integer`, `float`, `boolean`, `string`, `nil`, `{T}`.
//!
//! Tudo fora desse subconjunto (`if`/`while`/`for`, records, maps, arrays
//! manipuláveis, `import`, operadores aritméticos, retornos múltiplos, ...)
//! produz um erro sintático claro — nunca panic.

use crate::ast::{Args, Decl, Exp, Loc, Program, Stat, TopLevel, Type, Var};
use crate::lexer::{Token, TokenKind};

/// Erro sintático com posição (no espírito de
/// `titan/titan-compiler/syntax_errors.lua`).
#[derive(Debug, Clone, PartialEq)]
pub struct ParseError {
    pub message: String,
    pub loc: Loc,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "erro de sintaxe (linha {}, coluna {}): {}",
            self.loc.line, self.loc.col, self.message
        )
    }
}

impl std::error::Error for ParseError {}

/// Cursor sobre os tokens já lexados.
struct Parser<'a> {
    tokens: &'a [Token],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn new(tokens: &'a [Token]) -> Self {
        Parser { tokens, pos: 0 }
    }

    fn peek(&self) -> &Token {
        // O último token é sempre `Eof`, então `pos` nunca ultrapassa o slice.
        &self.tokens[self.pos]
    }

    fn loc(&self) -> Loc {
        self.peek().loc
    }

    fn advance(&mut self) -> Token {
        let token = self.tokens[self.pos].clone();
        if self.pos + 1 < self.tokens.len() {
            self.pos += 1;
        }
        token
    }

    fn check(&self, kind: &TokenKind) -> bool {
        &self.peek().kind == kind
    }

    /// Consome o token se ele casar com `kind`; devolve se consumiu.
    fn eat(&mut self, kind: &TokenKind) -> bool {
        if self.check(kind) {
            self.advance();
            true
        } else {
            false
        }
    }

    /// Exige `kind`, com `mensagem` de erro caso não esteja presente.
    fn expect(&mut self, kind: &TokenKind, mensagem: &str) -> Result<Token, ParseError> {
        if self.check(kind) {
            Ok(self.advance())
        } else {
            Err(self.erro(mensagem))
        }
    }

    fn erro(&self, mensagem: &str) -> ParseError {
        ParseError {
            message: mensagem.to_string(),
            loc: self.loc(),
        }
    }

    /// Exige um `Name` e devolve seu texto.
    fn expect_name(&mut self, mensagem: &str) -> Result<(String, Loc), ParseError> {
        let loc = self.loc();
        match &self.peek().kind {
            TokenKind::Name(_) => {
                let TokenKind::Name(nome) = self.advance().kind else {
                    unreachable!()
                };
                Ok((nome, loc))
            }
            _ => Err(self.erro(mensagem)),
        }
    }

    // ---- Programa e declarações de topo ----------------------------------

    fn parse_program(&mut self) -> Result<Program, ParseError> {
        let mut toplevels = Vec::new();
        while !self.check(&TokenKind::Eof) {
            toplevels.push(self.parse_toplevel()?);
        }
        Ok(toplevels)
    }

    fn parse_toplevel(&mut self) -> Result<TopLevel, ParseError> {
        let loc = self.loc();
        let islocal = self.eat(&TokenKind::Local);

        if self.eat(&TokenKind::Function) {
            return self.parse_toplevel_func(loc, islocal);
        }

        if islocal {
            return self.parse_toplevel_var(loc);
        }

        Err(self.erro("Esperava uma declaração de topo (`function` ou `local`) em vez disso."))
    }

    fn parse_toplevel_func(&mut self, loc: Loc, islocal: bool) -> Result<TopLevel, ParseError> {
        let (name, _) = self.expect_name("Esperava um nome de função após 'function'.")?;

        self.expect(
            &TokenKind::LParen,
            "Esperava '(' para a lista de parâmetros.",
        )?;
        let params = self.parse_param_list()?;
        self.expect(
            &TokenKind::RParen,
            "Esperava ')' para fechar a lista de parâmetros.",
        )?;

        let rettypes = self.parse_rettypes_opt()?;

        let block = self.parse_block()?;
        self.expect(
            &TokenKind::End,
            "Esperava 'end' para fechar o corpo da função.",
        )?;

        Ok(TopLevel::TopLevelFunc {
            loc,
            islocal,
            name,
            params,
            rettypes,
            block,
        })
    }

    fn parse_toplevel_var(&mut self, loc: Loc) -> Result<TopLevel, ParseError> {
        let decl = self.parse_decl_opt_type()?;
        self.expect(
            &TokenKind::Assign,
            "Esperava '=' após a declaração da variável.",
        )?;
        let value = self.parse_exp()?;
        Ok(TopLevel::TopLevelVar {
            loc,
            islocal: true,
            decl,
            value,
        })
    }

    fn parse_param_list(&mut self) -> Result<Vec<Decl>, ParseError> {
        let mut params = Vec::new();
        if self.check(&TokenKind::RParen) {
            return Ok(params);
        }
        params.push(self.parse_decl()?);
        while self.eat(&TokenKind::Comma) {
            params.push(self.parse_decl()?);
        }
        Ok(params)
    }

    /// `nome : Tipo` — nesta fase o tipo é sempre obrigatório em parâmetros e
    /// em `local`, exceto quando `parse_decl_opt_type` é usado.
    fn parse_decl(&mut self) -> Result<Decl, ParseError> {
        let (name, loc) = self.expect_name("Esperava um nome de parâmetro.")?;
        self.expect(&TokenKind::Colon, "Esperava ':' após o nome do parâmetro.")?;
        let r#type = self.parse_type()?;
        Ok(Decl {
            loc,
            name,
            r#type: Some(r#type),
            option: false,
        })
    }

    /// `nome [: Tipo]` — usado em `local`, onde a anotação é opcional.
    fn parse_decl_opt_type(&mut self) -> Result<Decl, ParseError> {
        let (name, loc) = self.expect_name("Esperava um nome de variável após 'local'.")?;
        let r#type = if self.eat(&TokenKind::Colon) {
            Some(self.parse_type()?)
        } else {
            None
        };
        Ok(Decl {
            loc,
            name,
            r#type,
            option: false,
        })
    }

    /// `[: Tipo]` — omitido vira `TypeNil` (`parser.lua:44-47`).
    fn parse_rettypes_opt(&mut self) -> Result<Vec<Type>, ParseError> {
        let loc = self.loc();
        if self.eat(&TokenKind::Colon) {
            Ok(vec![self.parse_type()?])
        } else {
            Ok(vec![Type::TypeNil { loc }])
        }
    }

    fn parse_type(&mut self) -> Result<Type, ParseError> {
        let loc = self.loc();
        match &self.peek().kind {
            TokenKind::Nil => {
                self.advance();
                Ok(Type::TypeNil { loc })
            }
            TokenKind::KwBoolean => {
                self.advance();
                Ok(Type::TypeBoolean { loc })
            }
            TokenKind::KwInteger => {
                self.advance();
                Ok(Type::TypeInteger { loc })
            }
            TokenKind::KwFloat => {
                self.advance();
                Ok(Type::TypeFloat { loc })
            }
            TokenKind::KwString => {
                self.advance();
                Ok(Type::TypeString { loc })
            }
            TokenKind::LCurly => {
                self.advance();
                let subtype = self.parse_type()?;
                self.expect(&TokenKind::RCurly, "Esperava '}' para fechar o tipo array.")?;
                Ok(Type::TypeArray {
                    loc,
                    subtype: Box::new(subtype),
                })
            }
            _ => Err(self.erro(
                "Esperava um tipo (`integer`, `float`, `boolean`, `string`, `nil` ou `{T}`).",
            )),
        }
    }

    // ---- Statements --------------------------------------------------

    fn parse_block(&mut self) -> Result<Stat, ParseError> {
        let loc = self.loc();
        let mut stats = Vec::new();
        while !self.check(&TokenKind::End) && !self.check(&TokenKind::Eof) {
            stats.push(self.parse_stat()?);
        }
        Ok(Stat::StatBlock { loc, stats })
    }

    fn parse_stat(&mut self) -> Result<Stat, ParseError> {
        let loc = self.loc();

        if self.eat(&TokenKind::Local) {
            return self.parse_stat_decl(loc);
        }

        if self.eat(&TokenKind::Return) {
            return self.parse_stat_return(loc);
        }

        // Único statement restante no subconjunto: chamada como statement.
        let exp = self.parse_suffixed_exp()?;
        if !matches!(exp, Exp::ExpCall { .. }) {
            return Err(ParseError {
                message: "Esperava um comando (`local`, `return` ou uma chamada de função)."
                    .to_string(),
                loc,
            });
        }
        self.eat(&TokenKind::Semicolon);
        Ok(Stat::StatCall { loc, callexp: exp })
    }

    fn parse_stat_decl(&mut self, loc: Loc) -> Result<Stat, ParseError> {
        let decl = self.parse_decl_opt_type()?;
        self.expect(
            &TokenKind::Assign,
            "Esperava '=' após a declaração da variável.",
        )?;
        let value = self.parse_exp()?;
        self.eat(&TokenKind::Semicolon);
        Ok(Stat::StatDecl {
            loc,
            decls: vec![decl],
            exps: vec![value],
        })
    }

    fn parse_stat_return(&mut self, loc: Loc) -> Result<Stat, ParseError> {
        let mut exps = Vec::new();
        if !self.check(&TokenKind::End)
            && !self.check(&TokenKind::Semicolon)
            && !self.check(&TokenKind::Eof)
        {
            exps.push(self.parse_exp()?);
        }
        self.eat(&TokenKind::Semicolon);
        Ok(Stat::StatReturn { loc, exps })
    }

    // ---- Expressões ----------------------------------------------------

    /// Ponto de entrada de expressão: por ora só `concat`, que é o único
    /// operador binário do subconjunto da Fase 0.
    fn parse_exp(&mut self) -> Result<Exp, ParseError> {
        self.parse_concat_exp()
    }

    /// `simple (.. simple)*` — associativo à direita no Titan original, mas
    /// como é o único nível de precedência aqui, a associatividade não altera
    /// o resultado observável (todos os operandos viram um único
    /// `ExpConcat.exps`).
    fn parse_concat_exp(&mut self) -> Result<Exp, ParseError> {
        let loc = self.loc();
        let first = self.parse_simple_exp()?;
        if !self.check(&TokenKind::Concat) {
            return Ok(first);
        }
        let mut exps = vec![first];
        while self.eat(&TokenKind::Concat) {
            exps.push(self.parse_simple_exp()?);
        }
        Ok(Exp::ExpConcat { loc, exps })
    }

    fn parse_simple_exp(&mut self) -> Result<Exp, ParseError> {
        let loc = self.loc();
        match &self.peek().kind {
            TokenKind::Nil => {
                self.advance();
                Ok(Exp::ExpNil { loc })
            }
            TokenKind::True => {
                self.advance();
                Ok(Exp::ExpBool { loc, value: true })
            }
            TokenKind::False => {
                self.advance();
                Ok(Exp::ExpBool { loc, value: false })
            }
            TokenKind::Integer(_) => {
                let TokenKind::Integer(value) = self.advance().kind else {
                    unreachable!()
                };
                Ok(Exp::ExpInteger { loc, value })
            }
            TokenKind::Float(_) => {
                let TokenKind::Float(value) = self.advance().kind else {
                    unreachable!()
                };
                Ok(Exp::ExpFloat { loc, value })
            }
            TokenKind::String(_) => {
                let TokenKind::String(value) = self.advance().kind else {
                    unreachable!()
                };
                Ok(Exp::ExpString { loc, value })
            }
            TokenKind::Name(_) | TokenKind::LParen => self.parse_suffixed_exp(),
            _ => Err(self.erro("Esperava uma expressão.")),
        }
    }

    /// Expressão primária (nome ou `( exp )`) seguida de zero ou mais
    /// sufixos de chamada — o único sufixo do subconjunto da Fase 0.
    fn parse_suffixed_exp(&mut self) -> Result<Exp, ParseError> {
        let mut exp = self.parse_primary_exp()?;

        while self.check(&TokenKind::LParen) {
            let call_loc = self.loc();
            let args = self.parse_call_args()?;
            exp = Exp::ExpCall {
                loc: call_loc,
                exp: Box::new(exp),
                args,
            };
        }

        Ok(exp)
    }

    fn parse_primary_exp(&mut self) -> Result<Exp, ParseError> {
        let loc = self.loc();
        match &self.peek().kind {
            TokenKind::Name(_) => {
                let (name, _) = self.expect_name("Esperava um nome.")?;
                Ok(Exp::ExpVar {
                    loc,
                    var: Box::new(Var::VarName { loc, name }),
                })
            }
            TokenKind::LParen => {
                self.advance();
                let exp = self.parse_exp()?;
                self.expect(&TokenKind::RParen, "Esperava ')' para fechar a expressão.")?;
                Ok(exp)
            }
            _ => Err(self.erro("Esperava um nome ou '(' seguido de expressão.")),
        }
    }

    fn parse_call_args(&mut self) -> Result<Args, ParseError> {
        let loc = self.loc();
        self.expect(
            &TokenKind::LParen,
            "Esperava '(' para os argumentos da chamada.",
        )?;
        let mut args = Vec::new();
        if !self.check(&TokenKind::RParen) {
            args.push(self.parse_exp()?);
            while self.eat(&TokenKind::Comma) {
                args.push(self.parse_exp()?);
            }
        }
        self.expect(
            &TokenKind::RParen,
            "Esperava ')' para fechar os argumentos da chamada.",
        )?;
        Ok(Args::ArgsFunc { loc, args })
    }
}

/// Analisa os tokens já lexados e produz o programa (`Vec<TopLevel>`).
///
/// Para no primeiro erro sintático — nunca entra em pânico.
pub fn parse(tokens: &[Token]) -> Result<Program, ParseError> {
    let mut parser = Parser::new(tokens);
    let program = parser.parse_program()?;
    Ok(program)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::lex;

    fn parse_source(source: &str) -> Result<Program, ParseError> {
        let tokens =
            lex(source).unwrap_or_else(|e| panic!("fonte não deveria ter erro léxico: {e}"));
        parse(&tokens)
    }

    #[test]
    fn produz_ast_esperada_para_hello_titan() {
        let source = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../examples/hello.titan"
        ))
        .expect("examples/hello.titan deve existir");

        let program = parse_source(&source).unwrap_or_else(|e| panic!("esperava sucesso: {e}"));

        assert_eq!(program.len(), 1);
        let TopLevel::TopLevelFunc {
            islocal,
            name,
            params,
            rettypes,
            block,
            ..
        } = &program[0]
        else {
            panic!("esperava TopLevelFunc");
        };

        assert!(!islocal);
        assert_eq!(name, "main");

        assert_eq!(params.len(), 1);
        assert_eq!(params[0].name, "args");
        match &params[0].r#type {
            Some(Type::TypeArray { subtype, .. }) => {
                assert!(matches!(**subtype, Type::TypeString { .. }));
            }
            other => panic!("esperava TypeArray{{TypeString}}, obteve {other:?}"),
        }

        assert_eq!(rettypes.len(), 1);
        assert!(matches!(rettypes[0], Type::TypeInteger { .. }));

        let Stat::StatBlock { stats, .. } = block else {
            panic!("esperava StatBlock");
        };
        assert_eq!(stats.len(), 2);

        let Stat::StatCall { callexp, .. } = &stats[0] else {
            panic!("esperava StatCall");
        };
        let Exp::ExpCall { exp, args, .. } = callexp else {
            panic!("esperava ExpCall");
        };
        let Exp::ExpVar { var, .. } = exp.as_ref() else {
            panic!("esperava ExpVar");
        };
        let Var::VarName { name, .. } = var.as_ref() else {
            panic!("esperava VarName");
        };
        assert_eq!(name, "print");
        let Args::ArgsFunc { args, .. } = args else {
            panic!("esperava ArgsFunc");
        };
        assert_eq!(args.len(), 1);
        assert!(matches!(
            &args[0],
            Exp::ExpString { value, .. } if value == "Olá, mundo!"
        ));

        let Stat::StatReturn { exps, .. } = &stats[1] else {
            panic!("esperava StatReturn");
        };
        assert_eq!(exps.len(), 1);
        assert!(matches!(exps[0], Exp::ExpInteger { value: 0, .. }));
    }

    #[test]
    fn end_faltando_produz_erro_sem_panic() {
        let err = parse_source("function main(): integer\n    return 0\n").unwrap_err();
        assert!(err.message.contains("end"));
    }

    #[test]
    fn local_com_tipo_explicito() {
        let program = parse_source(
            "local function f(): integer\n    local x: integer = 42\n    return x\nend",
        )
        .unwrap_or_else(|e| panic!("esperava sucesso: {e}"));

        let TopLevel::TopLevelFunc { islocal, block, .. } = &program[0] else {
            panic!("esperava TopLevelFunc");
        };
        assert!(islocal);

        let Stat::StatBlock { stats, .. } = block else {
            panic!("esperava StatBlock");
        };
        let Stat::StatDecl { decls, exps, .. } = &stats[0] else {
            panic!("esperava StatDecl");
        };
        assert_eq!(decls[0].name, "x");
        assert!(matches!(decls[0].r#type, Some(Type::TypeInteger { .. })));
        assert!(matches!(exps[0], Exp::ExpInteger { value: 42, .. }));
    }

    #[test]
    fn local_sem_tipo_explicito_fica_none() {
        let program =
            parse_source("local function f(): integer\n    local x = 42\n    return x\nend")
                .unwrap_or_else(|e| panic!("esperava sucesso: {e}"));

        let TopLevel::TopLevelFunc { block, .. } = &program[0] else {
            panic!("esperava TopLevelFunc");
        };
        let Stat::StatBlock { stats, .. } = block else {
            panic!("esperava StatBlock");
        };
        let Stat::StatDecl { decls, .. } = &stats[0] else {
            panic!("esperava StatDecl");
        };
        assert_eq!(decls[0].r#type, None);
    }

    #[test]
    fn tipo_de_retorno_omitido_vira_typenil() {
        let program =
            parse_source("function f()\nend").unwrap_or_else(|e| panic!("esperava sucesso: {e}"));
        let TopLevel::TopLevelFunc { rettypes, .. } = &program[0] else {
            panic!("esperava TopLevelFunc");
        };
        assert_eq!(rettypes.len(), 1);
        assert!(matches!(rettypes[0], Type::TypeNil { .. }));
    }

    #[test]
    fn concat_produz_expconcat_com_todos_os_operandos() {
        let program = parse_source(
            r#"function f(): string
    return "a" .. "b" .. "c"
end"#,
        )
        .unwrap_or_else(|e| panic!("esperava sucesso: {e}"));
        let TopLevel::TopLevelFunc { block, .. } = &program[0] else {
            panic!("esperava TopLevelFunc");
        };
        let Stat::StatBlock { stats, .. } = block else {
            panic!("esperava StatBlock");
        };
        let Stat::StatReturn { exps, .. } = &stats[0] else {
            panic!("esperava StatReturn");
        };
        let Exp::ExpConcat { exps, .. } = &exps[0] else {
            panic!("esperava ExpConcat");
        };
        assert_eq!(exps.len(), 3);
    }

    #[test]
    fn tipo_array_de_string() {
        let program = parse_source("function f(xs: {string}): integer\n    return 0\nend")
            .unwrap_or_else(|e| panic!("esperava sucesso: {e}"));
        let TopLevel::TopLevelFunc { params, .. } = &program[0] else {
            panic!("esperava TopLevelFunc");
        };
        assert!(matches!(params[0].r#type, Some(Type::TypeArray { .. })));
    }

    #[test]
    fn parametro_sem_dois_pontos_produz_erro_claro() {
        let err = parse_source("function f(x integer): integer\n    return 0\nend").unwrap_err();
        assert!(err.message.contains("':'"));
    }

    #[test]
    fn abre_parenteses_sem_fechar_produz_erro_claro() {
        let err = parse_source("function f(\n    return 0\nend").unwrap_err();
        assert!(!err.message.is_empty());
    }

    #[test]
    fn if_fora_do_subconjunto_produz_erro_claro_sem_panic() {
        let err =
            parse_source("function f(): integer\n    if true then\n    end\n    return 0\nend")
                .unwrap_err();
        assert!(!err.message.is_empty());
    }

    #[test]
    fn arquivo_vazio_produz_programa_vazio() {
        let program = parse_source("").unwrap_or_else(|e| panic!("esperava sucesso: {e}"));
        assert!(program.is_empty());
    }
}
