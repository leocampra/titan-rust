//! Análise sintática do Titan.
//!
//! Substitui a gramática PEG de `titan/titan-compiler/parser.lua` (575
//! linhas) por um parser de descida recursiva. Cobre o subconjunto das
//! Fases 0 e 1 (T4 e T11 do PRD.md):
//!
//! ```text
//! [local] function nome(p: T, ...) [: TipoRetorno] ... end
//! local x [: T] = exp
//! ```
//!
//! Statements: `StatCall`, `StatReturn` (lista de valores desde a T65),
//! `StatDecl`, `StatIf`, `StatWhile`, `StatRepeat` (T64), `StatFor`
//! (numérico), `StatAssign`, `StatMatch` (T75). `StatDecl` e `StatAssign`
//! aceitam lista de alvos desde a T67 (`local a, b = f()`, `a, b = b, a`).
//! Expressões: literais, `ExpVar`, `ExpCall`, `ExpConcat` (`..`),
//! `ExpMatch` (T75) e `ExpBinop`/`ExpUnop` numa cascata de precedência que
//! espelha `parser.lua:369-395` **por completo**, incluindo os níveis
//! bitwise (`|`, `~`, `&`, `<<`, `>>`) e a divisão inteira `//` (T60).
//! Declarações de topo: `record`, `enum` (T75), `import`,
//! `foreign function` (T73).
//! Tipos: `integer`, `float`, `boolean`, `string`, `nil`, `{T}`, a lista
//! de tipos de retorno da assinatura (`: integer, integer`, T65) e o sufixo
//! `?` de tipo opcional (`integer?`, T68) — que tem par no `?` depois do
//! nome numa declaração (`local x? = 10`, `Decl.option`).
//!
//! Tudo fora desse subconjunto (records, maps, arrays manipuláveis,
//! `import`, ...) produz um erro sintático claro — nunca panic.

use crate::ast::{
    Args, Decl, Exp, Field, FieldName, Loc, MatchArm, Pattern, Program, Stat, Then, TopLevel, Type,
    Var,
};
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

    /// Olha um token à frente sem consumir; no fim do fluxo, repete `Eof`.
    fn peek2(&self) -> &Token {
        &self.tokens[(self.pos + 1).min(self.tokens.len() - 1)]
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

        if self.eat(&TokenKind::KwRecord) {
            return self.parse_toplevel_record(loc);
        }

        if self.eat(&TokenKind::KwEnum) {
            return self.parse_toplevel_enum(loc);
        }

        if self.eat(&TokenKind::KwImport) {
            return self.parse_toplevel_import(loc);
        }

        if self.eat(&TokenKind::KwForeign) {
            return self.parse_toplevel_foreign(loc);
        }

        let islocal = self.eat(&TokenKind::Local);

        if self.eat(&TokenKind::Function) {
            return self.parse_toplevel_func(loc, islocal);
        }

        if islocal {
            return self.parse_toplevel_var(loc);
        }

        Err(self.erro(
            "Esperava uma declaração de topo (`function`, `foreign function`, `import`, \
             `local`, `record` ou `enum`) em vez disso.",
        ))
    }

    /// `foreign function abs(n: integer): integer` (T73) — declaração de
    /// função externa, sem corpo e sem `end`.
    ///
    /// A forma do Titan original (`foreign import stdio "stdio.h"`) é
    /// recusada aqui com erro claro: ela nomeia um header C e deixa as
    /// assinaturas implícitas, o que exigiria ler o header — exatamente o
    /// gerador de bindings que o ADR 0025 recusa. A mensagem aponta a grafia
    /// que existe, em vez de só dizer que a outra não existe.
    fn parse_toplevel_foreign(&mut self, _loc: Loc) -> Result<TopLevel, ParseError> {
        if self.check(&TokenKind::KwImport) {
            return Err(self.erro(
                "`foreign import` (a forma do Titan original, que lê um header C) não existe: \
                 declare cada função externa com sua assinatura Titan, como em \
                 `foreign function abs(n: integer): integer`.",
            ));
        }
        self.expect(
            &TokenKind::Function,
            "Esperava 'function' após 'foreign' (a única forma é \
             `foreign function nome(...): Tipo`).",
        )?;

        let (name, name_loc) =
            self.expect_name("Esperava um nome de função após 'foreign function'.")?;

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

        // Sem corpo: um `end` aqui é o erro típico de quem escreveu a
        // declaração no molde de `function`, e merece apontar a causa.
        if self.check(&TokenKind::End) {
            return Err(self.erro(
                "`foreign function` não tem corpo: remova o 'end' — a declaração termina \
                 no tipo de retorno.",
            ));
        }

        Ok(TopLevel::TopLevelForeignFunc {
            loc: name_loc,
            name,
            params,
            rettypes,
        })
    }

    /// `import Nome` e `import Nome as apelido` (T72). O ADR 0011 já trata
    /// `import data` como açúcar de `local data = import "data"` com
    /// `localname == modname`; o alias é só `localname != modname`, sem
    /// mecanismo novo — `SymbolKind::Module` (`checker.rs`) já guarda o nome
    /// do módulo separado do nome local.
    ///
    /// `import "data"` (nome de módulo como string) segue fora de escopo,
    /// com erro claro em vez de aceitar silenciosamente.
    fn parse_toplevel_import(&mut self, loc: Loc) -> Result<TopLevel, ParseError> {
        if matches!(self.peek().kind, TokenKind::String(_)) {
            return Err(self.erro(
                "Esperava um nome de módulo após 'import' (nome de string não é suportado).",
            ));
        }
        let (modname, _) = self.expect_name("Esperava um nome de módulo após 'import'.")?;

        let localname = if self.eat(&TokenKind::KwAs) {
            self.expect_name("Esperava um nome local após 'as'.")?.0
        } else {
            modname.clone()
        };

        Ok(TopLevel::TopLevelImport {
            loc,
            localname,
            modname,
        })
    }

    /// `record Nome campo: Tipo ... end` — campos são `Decl` (reusa
    /// `parse_decl`), até `end`; `;` opcional entre campos.
    ///
    /// Não replica o desaçúcar do original (`parser.lua:215-229`, que gera um
    /// `TopLevelStatic` sintético `Nome.new`): métodos estáticos estão fora
    /// do escopo, e implementá-los só para o construtor traria um caso
    /// especial que nada mais usa (ADR 0009).
    fn parse_toplevel_record(&mut self, loc: Loc) -> Result<TopLevel, ParseError> {
        let (name, _) = self.expect_name("Esperava um nome de record após 'record'.")?;

        let mut fields = Vec::new();
        while !self.check(&TokenKind::End) && !self.check(&TokenKind::Eof) {
            fields.push(self.parse_decl()?);
            self.eat(&TokenKind::Semicolon);
        }
        self.expect(&TokenKind::End, "Esperava 'end' para fechar o 'record'.")?;

        if fields.is_empty() {
            return Err(ParseError {
                message: "Um 'record' precisa de pelo menos um campo.".to_string(),
                loc,
            });
        }

        Ok(TopLevel::TopLevelRecord { loc, name, fields })
    }

    /// `enum Nome Variante Variante(T, ...) ... end` (T75), no molde de
    /// [`Parser::parse_toplevel_record`].
    ///
    /// A diferença de forma com o `record` é que os campos de uma variante
    /// são **posicionais** — só tipos, sem nome (`ast::Variant::fields` é
    /// `Vec<Type>`): o nome de cada campo aparece no braço do `match` que o
    /// liga, não aqui. Variante sem payload se escreve **sem parênteses**;
    /// `ExpNil()` é erro, não sinônimo, porque parênteses vazios sugeririam
    /// uma variante de aridade zero distinta da sem payload — e ela não
    /// existe.
    ///
    /// Recursão (`ExpBinop(string, Exp, Exp)`) é só uma `Type::TypeName`
    /// como outra qualquer para o parser: quem a permite de propósito é o
    /// checker (decisão técnica 6 do PRD.md).
    fn parse_toplevel_enum(&mut self, loc: Loc) -> Result<TopLevel, ParseError> {
        let (name, _) = self.expect_name("Esperava um nome de enum após 'enum'.")?;

        let mut variants = Vec::new();
        while !self.check(&TokenKind::End) && !self.check(&TokenKind::Eof) {
            variants.push(self.parse_variant()?);
            self.eat(&TokenKind::Semicolon);
        }
        self.expect(&TokenKind::End, "Esperava 'end' para fechar o 'enum'.")?;

        if variants.is_empty() {
            return Err(ParseError {
                message: "Um 'enum' precisa de pelo menos uma variante.".to_string(),
                loc,
            });
        }

        Ok(TopLevel::TopLevelEnum {
            loc,
            name,
            variants,
        })
    }

    /// Uma variante de `enum`: `Nome` ou `Nome(T1, T2, ...)`.
    fn parse_variant(&mut self) -> Result<crate::ast::Variant, ParseError> {
        let (name, loc) = self.expect_name("Esperava um nome de variante.")?;

        let mut fields = Vec::new();
        if self.eat(&TokenKind::LParen) {
            if self.check(&TokenKind::RParen) {
                return Err(self.erro(
                    "Variante sem payload se escreve sem parênteses: remova o '()' — \
                     parênteses vazios não são sinônimo de variante sem campos.",
                ));
            }
            fields.push(self.parse_type()?);
            while self.eat(&TokenKind::Comma) {
                fields.push(self.parse_type()?);
            }
            self.expect(
                &TokenKind::RParen,
                "Esperava ')' para fechar os campos da variante.",
            )?;
        }

        Ok(crate::ast::Variant { loc, name, fields })
    }

    fn parse_toplevel_func(&mut self, _loc: Loc, islocal: bool) -> Result<TopLevel, ParseError> {
        let (name, loc) = self.expect_name("Esperava um nome de função após 'function'.")?;

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
        let decl = self.parse_decl_opt_type("Esperava um nome de variável após 'local'.")?;
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

    /// `nome[?] [: Tipo]` — usado em `local` e no `for`, onde a anotação é
    /// opcional. `mensagem_nome` é o erro caso o nome não esteja presente.
    ///
    /// O `?` depois do **nome** (T68, `Decl.option`) é a forma inferida do
    /// tipo opcional: `local x? = 10` declara `x: integer?` sem repetir o
    /// `integer`. Escrever as duas coisas (`local x?: integer`) seria
    /// ambíguo — o `?` está do lado do nome, mas o tipo está escrito por
    /// extenso —, então é erro claro que aponta a grafia canônica.
    fn parse_decl_opt_type(&mut self, mensagem_nome: &str) -> Result<Decl, ParseError> {
        let (name, loc) = self.expect_name(mensagem_nome)?;
        let option = self.eat(&TokenKind::Question);
        let r#type = if self.eat(&TokenKind::Colon) {
            if option {
                return Err(self.erro(
                    "`?` depois do nome e anotação de tipo na mesma declaração: escreva \
                     `local nome: T?` (tipo explícito) ou `local nome? = valor` (tipo inferido).",
                ));
            }
            Some(self.parse_type()?)
        } else {
            None
        };
        Ok(Decl {
            loc,
            name,
            r#type,
            option,
        })
    }

    /// `[: Tipo {, Tipo}]` — omitido vira `TypeNil` (`parser.lua:44-47`).
    /// A lista separada por vírgula é o lado da assinatura dos retornos
    /// múltiplos (T65); um retorno só continua sendo o caso comum.
    fn parse_rettypes_opt(&mut self) -> Result<Vec<Type>, ParseError> {
        let loc = self.loc();
        if !self.eat(&TokenKind::Colon) {
            return Ok(vec![Type::TypeNil { loc }]);
        }
        let mut types = vec![self.parse_type()?];
        while self.eat(&TokenKind::Comma) {
            types.push(self.parse_type()?);
        }
        Ok(types)
    }

    /// Um tipo, com o sufixo `?` opcional (T68): `integer?` é
    /// `TypeOption { basetype: TypeInteger }`.
    ///
    /// O `?` é sufixo de **todo** o tipo à esquerda, então `{integer}?` é um
    /// array opcional e `{integer?}` é um array de inteiros opcionais — a
    /// diferença sai de graça, porque quem lê o `{...}` chama este método de
    /// volta para o elemento.
    ///
    /// `T??` é recusado **aqui**, e não no checker: um segundo `?` nunca tem
    /// leitura útil (`Option<Option<T>>` não acrescenta estado nenhum sobre
    /// `Option<T>`), e o erro sintático aponta exatamente o `?` sobrando.
    fn parse_type(&mut self) -> Result<Type, ParseError> {
        let loc = self.loc();
        let base = self.parse_type_base()?;
        if !self.eat(&TokenKind::Question) {
            return Ok(base);
        }
        if self.check(&TokenKind::Question) {
            return Err(self.erro(
                "`?` duplicado no tipo: `T??` não existe — um tipo opcional já cobre a ausência de valor.",
            ));
        }
        Ok(Type::TypeOption {
            loc,
            basetype: Box::new(base),
        })
    }

    /// O tipo sem o sufixo `?` — a parte que [`Parser::parse_type`] envolve.
    fn parse_type_base(&mut self) -> Result<Type, ParseError> {
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
            TokenKind::KwValue => {
                self.advance();
                Ok(Type::TypeValue { loc })
            }
            TokenKind::Name(_) => {
                let (name, _) = self.expect_name("Esperava um nome de tipo.")?;
                if self.eat(&TokenKind::Dot) {
                    let (member, _) =
                        self.expect_name("Esperava um nome de tipo após '.' no tipo qualificado.")?;
                    return Ok(Type::TypeQualName {
                        loc,
                        module: name,
                        name: member,
                    });
                }
                Ok(Type::TypeName { loc, name })
            }
            TokenKind::LCurly => {
                self.advance();
                let first = self.parse_type()?;
                if self.eat(&TokenKind::Colon) {
                    let valuestype = self.parse_type()?;
                    self.expect(&TokenKind::RCurly, "Esperava '}' para fechar o tipo map.")?;
                    Ok(Type::TypeMap {
                        loc,
                        keystype: Box::new(first),
                        valuestype: Box::new(valuestype),
                    })
                } else {
                    self.expect(&TokenKind::RCurly, "Esperava '}' para fechar o tipo array.")?;
                    Ok(Type::TypeArray {
                        loc,
                        subtype: Box::new(first),
                    })
                }
            }
            _ => Err(self.erro(
                "Esperava um tipo (`integer`, `float`, `boolean`, `string`, `value`, `nil`, \
                 um nome de record, `{T}`, `{K: V}` ou qualquer um deles seguido de `?`).",
            )),
        }
    }

    // ---- Statements --------------------------------------------------

    fn parse_block(&mut self) -> Result<Stat, ParseError> {
        let loc = self.loc();
        let mut stats = Vec::new();
        // `elseif`/`else` também terminam um bloco — quem os consome (ou
        // rejeita, no caso de um bloco de função) é o chamador. `until`
        // (T64) entra na mesma lista: é ele, e não `end`, que fecha o corpo
        // de um `repeat`.
        while !self.check(&TokenKind::End)
            && !self.check(&TokenKind::Elseif)
            && !self.check(&TokenKind::Else)
            && !self.check(&TokenKind::KwUntil)
            && !self.check(&TokenKind::Eof)
        {
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

        if self.eat(&TokenKind::If) {
            return self.parse_stat_if(loc);
        }

        if self.eat(&TokenKind::While) {
            return self.parse_stat_while(loc);
        }

        if self.eat(&TokenKind::KwRepeat) {
            return self.parse_stat_repeat(loc);
        }

        if self.eat(&TokenKind::For) {
            return self.parse_stat_for(loc);
        }

        if self.eat(&TokenKind::KwBreak) {
            self.eat(&TokenKind::Semicolon);
            return Ok(Stat::StatBreak { loc });
        }

        // `continue` (T63): até a T62 esta posição carregava uma rejeição
        // explícita, porque o `for` desaçucarado punha o incremento no fim do
        // corpo e um `continue` pularia por cima dele (ADR 0017). Com o
        // incremento no topo do `loop` (ADR 0022), a mensagem deixou de ser
        // verdade e o comando entra como qualquer outro — a checagem de estar
        // dentro de laço fica no checker, igual a `break`.
        if self.eat(&TokenKind::KwContinue) {
            self.eat(&TokenKind::Semicolon);
            return Ok(Stat::StatContinue { loc });
        }

        if self.eat(&TokenKind::KwMatch) {
            return self.parse_stat_match(loc);
        }

        // Chamada ou atribuição — desambiguadas sem backtracking, como no
        // original (`suffixedexp` + checar `ASSIGN`, `parser.lua:354-358`):
        // parseia a expressão sufixada e o token seguinte decide.
        // A vírgula entra na desambiguação com a T67: `a, b = ...` é a
        // única forma que continua depois de uma expressão sufixada sem
        // um `=` logo a seguir, então ela decide tão cedo quanto o `=`
        // decidia sozinho — e segue sem backtracking.
        let exp = self.parse_suffixed_exp()?;
        if self.check(&TokenKind::Assign) || self.check(&TokenKind::Comma) {
            return self.parse_stat_assign(loc, exp);
        }
        if !matches!(exp, Exp::ExpCall { .. }) {
            return Err(ParseError {
                message: "Esperava um comando (`local`, `return`, `if`, `while`, `repeat`, \
                          `for`, `break`, `continue`, `match`, uma atribuição ou uma \
                          chamada de função)."
                    .to_string(),
                loc,
            });
        }
        self.eat(&TokenKind::Semicolon);
        Ok(Stat::StatCall { loc, callexp: exp })
    }

    /// `if exp then block (elseif exp then block)* (else block)? end`
    fn parse_stat_if(&mut self, loc: Loc) -> Result<Stat, ParseError> {
        let mut thens = Vec::new();
        let mut branch_loc = loc;
        loop {
            let condition = self.parse_exp()?;
            self.expect(&TokenKind::Then, "Esperava 'then' após a condição.")?;
            let block = self.parse_block()?;
            thens.push(Then {
                loc: branch_loc,
                condition,
                block,
            });
            if !self.check(&TokenKind::Elseif) {
                break;
            }
            branch_loc = self.loc();
            self.advance();
        }
        let elsestat = if self.eat(&TokenKind::Else) {
            Some(Box::new(self.parse_block()?))
        } else {
            None
        };
        self.expect(&TokenKind::End, "Esperava 'end' para fechar o 'if'.")?;
        Ok(Stat::StatIf {
            loc,
            thens,
            elsestat,
        })
    }

    /// `match exp with (padrão then block)+ end` em posição de comando (T75).
    fn parse_stat_match(&mut self, loc: Loc) -> Result<Stat, ParseError> {
        let (exp, arms) = self.parse_match_corpo(loc, Parser::parse_match_arm_block)?;
        Ok(Stat::StatMatch {
            loc,
            exp: Box::new(exp),
            arms,
        })
    }

    /// `match exp with (padrão then exp)+ end` em posição de expressão (T75).
    ///
    /// O corpo de cada braço é uma **expressão**, o que torna a fronteira
    /// entre braços exata sem lookahead nenhum: a expressão acaba, e o que
    /// vier depois ou é `end` ou é o próximo padrão. É a diferença que faz
    /// [`Parser::parse_match_arm_block`] existir só do lado do comando.
    fn parse_exp_match(&mut self, loc: Loc) -> Result<Exp, ParseError> {
        self.advance(); // consome 'match'
        let (exp, arms) = self.parse_match_corpo(loc, Parser::parse_exp)?;
        Ok(Exp::ExpMatch {
            loc,
            exp: Box::new(exp),
            arms,
        })
    }

    /// O que as duas formas de `match` têm em comum — tudo menos o corpo do
    /// braço, que `parse_body` fornece: um bloco no comando, uma expressão
    /// na expressão.
    ///
    /// A exaustividade dos braços — e até se o escrutinado é mesmo um tipo
    /// soma — é assunto do checker (T76): aqui só se exige que exista ao
    /// menos um braço, porque um `match` sem braço nenhum é erro de escrita,
    /// não programa vazio.
    fn parse_match_corpo<T>(
        &mut self,
        loc: Loc,
        parse_body: fn(&mut Self) -> Result<T, ParseError>,
    ) -> Result<(Exp, Vec<MatchArm<T>>), ParseError> {
        let exp = self.parse_exp()?;
        self.expect(
            &TokenKind::KwWith,
            "Esperava 'with' após a expressão do 'match'.",
        )?;

        let mut arms = Vec::new();
        while !self.check(&TokenKind::End) && !self.check(&TokenKind::Eof) {
            let arm_loc = self.loc();
            let pattern = self.parse_pattern()?;
            self.expect(&TokenKind::Then, "Esperava 'then' após o padrão do braço.")?;
            let body = parse_body(self)?;
            self.eat(&TokenKind::Semicolon);
            arms.push(MatchArm {
                loc: arm_loc,
                pattern,
                body,
            });
        }
        self.expect(&TokenKind::End, "Esperava 'end' para fechar o 'match'.")?;

        if arms.is_empty() {
            return Err(ParseError {
                message: "Um 'match' precisa de pelo menos um braço.".to_string(),
                loc,
            });
        }

        Ok((exp, arms))
    }

    /// O padrão de um braço: `_`, `Variante` ou `Variante(a, b, ...)`.
    ///
    /// Os campos ligam nomes locais **posicionalmente** — o tipo de cada um
    /// vem da declaração do `enum`, então escrevê-lo aqui seria repetição
    /// que poderia divergir. Por isso são `Decl` com `type: None`, e um
    /// `Variante(n: integer)` é recusado com a razão, não só com "esperava
    /// ')'".
    fn parse_pattern(&mut self) -> Result<Pattern, ParseError> {
        let loc = self.loc();

        // `_` chega como `Name("_")`: é identificador para o léxico, e só o
        // parser de padrão lhe dá o sentido de curinga.
        if matches!(&self.peek().kind, TokenKind::Name(n) if n == "_") {
            self.advance();
            return Ok(Pattern::Wildcard { loc });
        }

        let (name, _) = self.expect_name(
            "Esperava um padrão de braço: um nome de variante ou '_' (o braço curinga).",
        )?;

        let mut fields = Vec::new();
        if self.eat(&TokenKind::LParen) {
            if self.check(&TokenKind::RParen) {
                return Err(self.erro(
                    "Variante sem payload se escreve sem parênteses no braço: remova o '()'.",
                ));
            }
            fields.push(self.parse_pattern_field()?);
            while self.eat(&TokenKind::Comma) {
                fields.push(self.parse_pattern_field()?);
            }
            self.expect(
                &TokenKind::RParen,
                "Esperava ')' para fechar os campos do padrão.",
            )?;
        }

        Ok(Pattern::Variant { loc, name, fields })
    }

    /// Um nome ligado por um padrão. Sem anotação de tipo — ela vem do
    /// `enum`.
    fn parse_pattern_field(&mut self) -> Result<Decl, ParseError> {
        let (name, loc) = self.expect_name("Esperava um nome para ligar o campo da variante.")?;

        if self.check(&TokenKind::Colon) {
            return Err(self.erro(
                "O campo de um padrão não leva anotação de tipo: o tipo vem da declaração \
                 do 'enum'.",
            ));
        }

        Ok(Decl {
            loc,
            name,
            r#type: None,
            option: false,
        })
    }

    /// O bloco de um braço de `match` em posição de comando, que termina no
    /// `end` do `match` ou no início do braço seguinte.
    ///
    /// A fronteira entre braços não tem token próprio (não há `elseif` do
    /// `match`), e um padrão começa com um `Name` — exatamente como uma
    /// chamada ou uma atribuição. Daí [`Parser::inicia_braco_de_match`]:
    /// uma varredura à frente, sem consumir token nem desfazer parse —
    /// olhar não é backtracking.
    fn parse_match_arm_block(&mut self) -> Result<Stat, ParseError> {
        let loc = self.loc();
        let mut stats = Vec::new();
        while !self.check(&TokenKind::End)
            && !self.check(&TokenKind::Eof)
            && !self.inicia_braco_de_match()
        {
            stats.push(self.parse_stat()?);
        }
        Ok(Stat::StatBlock { loc, stats })
    }

    /// Decide, olhando à frente sem consumir, se a posição atual começa um
    /// braço de `match` — isto é, se é `_ then`, `Nome then` ou
    /// `Nome ( ... ) then`.
    ///
    /// O caso com parênteses é o único que exige varrer mais de dois tokens,
    /// e o faz contando profundidade até o `)` que fecha: a lista de campos
    /// de um padrão é plana (só nomes e vírgulas), mas contar é mais barato
    /// que provar que é plana, e não depende dessa garantia continuar
    /// valendo.
    fn inicia_braco_de_match(&self) -> bool {
        if !matches!(self.peek().kind, TokenKind::Name(_)) {
            return false;
        }

        if matches!(self.peek2().kind, TokenKind::Then) {
            return true;
        }

        if !matches!(self.peek2().kind, TokenKind::LParen) {
            return false;
        }

        let mut i = self.pos + 2;
        let mut profundidade = 1usize;
        while i < self.tokens.len() {
            match self.tokens[i].kind {
                TokenKind::LParen => profundidade += 1,
                TokenKind::RParen => {
                    profundidade -= 1;
                    if profundidade == 0 {
                        return matches!(
                            self.tokens.get(i + 1).map(|t| &t.kind),
                            Some(TokenKind::Then)
                        );
                    }
                }
                // Um `end` ou um `then` solto antes do fecha-parênteses quer
                // dizer que os parênteses não fecham: não é braço, e o erro
                // de sintaxe aparece onde ele realmente está.
                TokenKind::End | TokenKind::Then | TokenKind::Eof => return false,
                _ => {}
            }
            i += 1;
        }
        false
    }

    /// `while exp do block end`
    fn parse_stat_while(&mut self, loc: Loc) -> Result<Stat, ParseError> {
        let condition = self.parse_exp()?;
        self.expect(&TokenKind::Do, "Esperava 'do' após a condição do 'while'.")?;
        let block = self.parse_block()?;
        self.expect(&TokenKind::End, "Esperava 'end' para fechar o 'while'.")?;
        Ok(Stat::StatWhile {
            loc,
            condition,
            block: Box::new(block),
        })
    }

    /// `repeat block until exp` (T64) — o único laço da linguagem que testa
    /// a condição **no fim**, e por isso roda o corpo ao menos uma vez.
    ///
    /// Diferente de `while`/`for`, não há `do` nem `end`: quem abre é o
    /// próprio `repeat` e quem fecha é o `until`, motivo pelo qual
    /// [`Self::parse_block`] o reconhece como terminador de bloco. A
    /// condição fica **fora** do `StatBlock` na AST, mas em Lua — e no Titan
    /// — ela enxerga os `local` declarados no corpo; garantir isso é
    /// trabalho do checker, que só fecha o escopo do corpo depois de tipar
    /// o `until`.
    fn parse_stat_repeat(&mut self, loc: Loc) -> Result<Stat, ParseError> {
        let block = self.parse_block()?;
        self.expect(
            &TokenKind::KwUntil,
            "Esperava 'until' para fechar o 'repeat'.",
        )?;
        let condition = self.parse_exp()?;
        self.eat(&TokenKind::Semicolon);
        Ok(Stat::StatRepeat {
            loc,
            block: Box::new(block),
            condition,
        })
    }

    /// As duas formas de `for`:
    ///
    /// - numérica — `for nome [: T] = exp, exp [, exp] do block end`;
    /// - `for`-in (T71) — `for nome [: T] in exp do block end` sobre `{T}` e
    ///   `for k [: K], v [: V] in exp do block end` sobre `{K: V}`.
    ///
    /// O que decide entre elas é o token **depois** da primeira declaração:
    /// `=` abre a numérica, `,` ou `in` abrem a forma de iteração. As duas
    /// compartilham só a primeira `Decl`; daí em diante são gramáticas
    /// distintas, e o erro de um `for` que não é nem uma nem outra precisa
    /// citar as duas continuações possíveis.
    fn parse_stat_for(&mut self, loc: Loc) -> Result<Stat, ParseError> {
        let decl = self.parse_decl_opt_type("Esperava um nome de variável após 'for'.")?;
        if self.check(&TokenKind::Comma) || self.check(&TokenKind::KwIn) {
            return self.parse_stat_for_in(loc, decl);
        }
        self.expect(
            &TokenKind::Assign,
            "Esperava '=' (for numérico) ou 'in' (for sobre array/map) após a variável do 'for'.",
        )?;
        let start = self.parse_exp()?;
        self.expect(
            &TokenKind::Comma,
            "Esperava ',' entre o início e o fim do 'for'.",
        )?;
        let finish = self.parse_exp()?;
        let inc = if self.eat(&TokenKind::Comma) {
            Some(Box::new(self.parse_exp()?))
        } else {
            None
        };
        self.expect(&TokenKind::Do, "Esperava 'do' após os limites do 'for'.")?;
        let block = self.parse_block()?;
        self.expect(&TokenKind::End, "Esperava 'end' para fechar o 'for'.")?;
        Ok(Stat::StatFor {
            loc,
            decl: Box::new(decl),
            start: Box::new(start),
            finish: Box::new(finish),
            inc,
            block: Box::new(block),
        })
    }

    /// `for`-in (T71) — a continuação depois que [`Self::parse_stat_for`]
    /// já leu a primeira declaração e viu `,` ou `in`.
    ///
    /// O parser aceita **qualquer** número de declarações e deixa a aridade
    /// para o checker: quantos nomes cabem depende do tipo do container
    /// (`{T}` liga um, `{K: V}` liga dois), e o tipo é coisa que só o checker
    /// conhece. Errar aqui produziria "esperava 1 ou 2 nomes" sem poder dizer
    /// qual dos dois o programa deveria ter escrito.
    fn parse_stat_for_in(&mut self, loc: Loc, primeira: Decl) -> Result<Stat, ParseError> {
        let mut decls = vec![primeira];
        while self.eat(&TokenKind::Comma) {
            decls
                .push(self.parse_decl_opt_type("Esperava um nome de variável após ',' no 'for'.")?);
        }
        self.expect(
            &TokenKind::KwIn,
            "Esperava 'in' após as variáveis do 'for'.",
        )?;
        let exp = self.parse_exp()?;
        self.expect(
            &TokenKind::Do,
            "Esperava 'do' após a expressão iterada do 'for'.",
        )?;
        let block = self.parse_block()?;
        self.expect(&TokenKind::End, "Esperava 'end' para fechar o 'for'.")?;
        Ok(Stat::StatForIn {
            loc,
            decls,
            exp: Box::new(exp),
            block: Box::new(block),
        })
    }

    /// `var {, var} = exp {, exp}` — a atribuição, single-target ou múltipla
    /// (T67). `target` é a primeira expressão sufixada, já parseada pelo
    /// chamador; nem a vírgula nem o `=` foram consumidos.
    fn parse_stat_assign(&mut self, loc: Loc, target: Exp) -> Result<Stat, ParseError> {
        let mut vars = vec![self.exp_para_var(target)?];
        while self.eat(&TokenKind::Comma) {
            let alvo = self.parse_suffixed_exp()?;
            vars.push(self.exp_para_var(alvo)?);
        }
        self.expect(&TokenKind::Assign, "Esperava '=' na atribuição.")?;
        let exps = self.parse_exp_list()?;
        self.eat(&TokenKind::Semicolon);
        Ok(Stat::StatAssign { loc, vars, exps })
    }

    /// Converte uma expressão sufixada já parseada em alvo de atribuição.
    /// Os dois erros são os mesmos de sempre — só passaram a valer para
    /// cada alvo da lista, não só para o primeiro.
    fn exp_para_var(&self, exp: Exp) -> Result<Var, ParseError> {
        match exp {
            Exp::ExpVar { var, .. } => Ok(*var),
            Exp::ExpCall { .. } => {
                Err(self.erro("Não é possível atribuir a uma chamada de função."))
            }
            _ => Err(self.erro("Esperava uma variável do lado esquerdo de '='.")),
        }
    }

    fn parse_stat_decl(&mut self, loc: Loc) -> Result<Stat, ParseError> {
        let mut decls =
            vec![self.parse_decl_opt_type("Esperava um nome de variável após 'local'.")?];
        // `local a, b = ...` (T67): cada nome pode trazer sua própria
        // anotação de tipo (`local q: integer, r: integer = divmod(...)`).
        while self.eat(&TokenKind::Comma) {
            decls.push(self.parse_decl_opt_type("Esperava um nome de variável após ','.")?);
        }
        self.expect(
            &TokenKind::Assign,
            "Esperava '=' após a declaração da variável.",
        )?;
        let exps = self.parse_exp_list()?;
        self.eat(&TokenKind::Semicolon);
        Ok(Stat::StatDecl { loc, decls, exps })
    }

    /// `exp {, exp}` — o lado direito de uma declaração ou atribuição.
    fn parse_exp_list(&mut self) -> Result<Vec<Exp>, ParseError> {
        let mut exps = vec![self.parse_exp()?];
        while self.eat(&TokenKind::Comma) {
            exps.push(self.parse_exp()?);
        }
        Ok(exps)
    }

    fn parse_stat_return(&mut self, loc: Loc) -> Result<Stat, ParseError> {
        let mut exps = Vec::new();
        // Os terminadores de bloco (`end`/`elseif`/`else`) e o `;` indicam
        // `return` sem valor.
        if !self.check(&TokenKind::End)
            && !self.check(&TokenKind::Elseif)
            && !self.check(&TokenKind::Else)
            && !self.check(&TokenKind::Semicolon)
            && !self.check(&TokenKind::Eof)
        {
            exps.push(self.parse_exp()?);
            // `return a, b` — a lista de valores dos retornos múltiplos (T65).
            while self.eat(&TokenKind::Comma) {
                exps.push(self.parse_exp()?);
            }
        }
        self.eat(&TokenKind::Semicolon);
        Ok(Stat::StatReturn { loc, exps })
    }

    // ---- Expressões ----------------------------------------------------
    //
    // Cascata de níveis de precedência (do mais fraco ao mais forte),
    // espelhando `parser.lua:369-395` por completo — os níveis bitwise
    // (`op4`–`op7` do original) entraram na T60:
    //
    // ```text
    // parse_exp → parse_or_exp
    // or_exp     : and_exp (or and_exp)*                       — assoc. esquerda
    // and_exp    : rel_exp (and rel_exp)*                      — assoc. esquerda
    // rel_exp    : bor_exp ((== ~= < > <= >=) bor_exp)?        — sem encadear
    // bor_exp    : bxor_exp (| bxor_exp)*                      — assoc. esquerda
    // bxor_exp   : band_exp (~ band_exp)*                      — assoc. esquerda
    // band_exp   : shift_exp (& shift_exp)*                    — assoc. esquerda
    // shift_exp  : concat_exp ((<< >>) concat_exp)*            — assoc. esquerda
    // concat_exp : add_exp (.. add_exp)*                       — assoc. direita
    // add_exp    : mul_exp ((+ -) mul_exp)*                    — assoc. esquerda
    // mul_exp    : unary_exp ((* / // %) unary_exp)*           — assoc. esquerda
    // unary_exp  : (not | - | # | ~)* pow_exp
    // pow_exp    : cast_exp (^ unary_exp)?                     — assoc. direita
    // cast_exp   : simple_exp (as tipo)*                       — assoc. esquerda
    // ```
    //
    // O `~` é ambíguo por natureza: binário é XOR (nível `bxor_exp`),
    // unário é NOT (nível `unary_exp`). A cascata resolve sozinha — quem
    // chega em `parse_unary_exp` com um `~` na frente está em posição de
    // prefixo; quem chega em `parse_bxor_exp` já consumiu o operando
    // esquerdo.

    fn parse_exp(&mut self) -> Result<Exp, ParseError> {
        self.parse_or_exp()
    }

    /// Nível binário associativo à esquerda: `next ((ops) next)*`.
    ///
    /// `op_for` devolve a grafia do operador quando o token pertence ao
    /// nível — exatamente as strings do Titan original (`"+"`, `"~="`,
    /// `"and"`, ...), que é o que o checker vai casar.
    fn parse_left_assoc_binop(
        &mut self,
        next: fn(&mut Self) -> Result<Exp, ParseError>,
        op_for: fn(&TokenKind) -> Option<&'static str>,
    ) -> Result<Exp, ParseError> {
        let mut lhs = next(self)?;
        while let Some(op) = op_for(&self.peek().kind) {
            let loc = self.loc();
            self.advance();
            let rhs = next(self)?;
            lhs = Exp::ExpBinop {
                loc,
                lhs: Box::new(lhs),
                op: op.to_string(),
                rhs: Box::new(rhs),
            };
        }
        Ok(lhs)
    }

    fn parse_or_exp(&mut self) -> Result<Exp, ParseError> {
        self.parse_left_assoc_binop(Self::parse_and_exp, |kind| match kind {
            TokenKind::Or => Some("or"),
            _ => None,
        })
    }

    fn parse_and_exp(&mut self) -> Result<Exp, ParseError> {
        self.parse_left_assoc_binop(Self::parse_rel_exp, |kind| match kind {
            TokenKind::And => Some("and"),
            _ => None,
        })
    }

    /// Relacionais **não encadeiam** (fiel ao original): `a == b == c` é
    /// erro sintático, não `(a == b) == c`.
    fn parse_rel_exp(&mut self) -> Result<Exp, ParseError> {
        let lhs = self.parse_bor_exp()?;
        let op = match &self.peek().kind {
            TokenKind::Eq => "==",
            TokenKind::Ne => "~=",
            TokenKind::Lt => "<",
            TokenKind::Gt => ">",
            TokenKind::Le => "<=",
            TokenKind::Ge => ">=",
            _ => return Ok(lhs),
        };
        let loc = self.loc();
        self.advance();
        let rhs = self.parse_bor_exp()?;
        Ok(Exp::ExpBinop {
            loc,
            lhs: Box::new(lhs),
            op: op.to_string(),
            rhs: Box::new(rhs),
        })
    }

    /// `bxor (| bxor)*` — OU bitwise, o mais fraco dos níveis bitwise.
    fn parse_bor_exp(&mut self) -> Result<Exp, ParseError> {
        self.parse_left_assoc_binop(Self::parse_bxor_exp, |kind| match kind {
            TokenKind::Pipe => Some("|"),
            _ => None,
        })
    }

    /// `band (~ band)*` — XOR bitwise. No Titan, como em Lua 5.3, o `~`
    /// binário é XOR (o `~=` já foi resolvido pelo lexer por lookahead).
    fn parse_bxor_exp(&mut self) -> Result<Exp, ParseError> {
        self.parse_left_assoc_binop(Self::parse_band_exp, |kind| match kind {
            TokenKind::Tilde => Some("~"),
            _ => None,
        })
    }

    /// `shift (& shift)*` — E bitwise.
    fn parse_band_exp(&mut self) -> Result<Exp, ParseError> {
        self.parse_left_assoc_binop(Self::parse_shift_exp, |kind| match kind {
            TokenKind::Amp => Some("&"),
            _ => None,
        })
    }

    /// `concat ((<< >>) concat)*` — deslocamentos, logo acima da
    /// concatenação: `1 << 2 + 3` agrupa o `+` primeiro.
    fn parse_shift_exp(&mut self) -> Result<Exp, ParseError> {
        self.parse_left_assoc_binop(Self::parse_concat_exp, |kind| match kind {
            TokenKind::Shl => Some("<<"),
            TokenKind::Shr => Some(">>"),
            _ => None,
        })
    }

    /// `add (.. add)*` — associativo à direita no Titan original, mas como
    /// todos os operandos viram um único `ExpConcat.exps` (mesma forma
    /// achatada do `ast.lua`), a associatividade não altera o resultado
    /// observável.
    fn parse_concat_exp(&mut self) -> Result<Exp, ParseError> {
        let loc = self.loc();
        let first = self.parse_add_exp()?;
        if !self.check(&TokenKind::Concat) {
            return Ok(first);
        }
        let mut exps = vec![first];
        while self.eat(&TokenKind::Concat) {
            exps.push(self.parse_add_exp()?);
        }
        Ok(Exp::ExpConcat { loc, exps })
    }

    fn parse_add_exp(&mut self) -> Result<Exp, ParseError> {
        self.parse_left_assoc_binop(Self::parse_mul_exp, |kind| match kind {
            TokenKind::Plus => Some("+"),
            TokenKind::Minus => Some("-"),
            _ => None,
        })
    }

    fn parse_mul_exp(&mut self) -> Result<Exp, ParseError> {
        self.parse_left_assoc_binop(Self::parse_unary_exp, |kind| match kind {
            TokenKind::Star => Some("*"),
            TokenKind::Slash => Some("/"),
            TokenKind::DoubleSlash => Some("//"),
            TokenKind::Percent => Some("%"),
            _ => None,
        })
    }

    /// `(not | - | # | ~)* pow_exp` — a repetição vira recursão: `- -1` e
    /// `not not true` produzem `ExpUnop` aninhados. `#` (T20/T25: lexado
    /// desde a T20, mas sem produtor no parser até aqui — T30 fecha essa
    /// lacuna, exigida por `#xs`/`#s` ponta a ponta) segue o mesmo lugar na
    /// gramática: `checker::check_unop` já sabe mapear `"#"` para
    /// `UnOp::Len`. O `~` em posição de prefixo é o NOT bitwise (`BNEG` do
    /// original) — é aqui que ele se separa do XOR binário de
    /// `parse_bxor_exp`.
    fn parse_unary_exp(&mut self) -> Result<Exp, ParseError> {
        let loc = self.loc();
        let op = match &self.peek().kind {
            TokenKind::Not => "not",
            TokenKind::Minus => "-",
            TokenKind::Hash => "#",
            TokenKind::Tilde => "~",
            _ => return self.parse_pow_exp(),
        };
        self.advance();
        let exp = self.parse_unary_exp()?;
        Ok(Exp::ExpUnop {
            loc,
            op: op.to_string(),
            exp: Box::new(exp),
        })
    }

    /// `simple (^ unary)?` — associativo à direita (`2 ^ 3 ^ 2` = `2 ^ (3 ^ 2)`).
    /// O expoente volta ao nível unário para aceitar `2 ^ -3`.
    fn parse_pow_exp(&mut self) -> Result<Exp, ParseError> {
        let base = self.parse_cast_exp()?;
        if !self.check(&TokenKind::Caret) {
            return Ok(base);
        }
        let loc = self.loc();
        self.advance();
        let expoente = self.parse_unary_exp()?;
        Ok(Exp::ExpBinop {
            loc,
            lhs: Box::new(base),
            op: "^".to_string(),
            rhs: Box::new(expoente),
        })
    }

    /// `simple (as tipo)*` — o cast do Titan (T70), mais forte que todos os
    /// operadores e mais fraco que os sufixos de `parse_suffixed_exp`.
    ///
    /// O lugar na cascata decide três leituras, todas iguais às do original:
    ///
    /// - `-x as float` é `-(x as float)`: o unário fica **acima**, então o
    ///   cast morde primeiro. Para inteiro isso dá no mesmo, mas para
    ///   `i64::MIN` a ordem é observável.
    /// - `a + b as float` é `a + (b as float)`, não `(a + b) as float` —
    ///   binário nenhum chega a competir com `as`.
    /// - `f() as integer` converte o **retorno** da chamada: o `(` é sufixo
    ///   de `parse_suffixed_exp`, que já rodou por dentro de `simple`.
    ///
    /// O laço (`*`, não `?`) aceita `x as float as value` sem parênteses;
    /// cada `as` embrulha o anterior, então a associatividade é à esquerda.
    fn parse_cast_exp(&mut self) -> Result<Exp, ParseError> {
        let mut exp = self.parse_simple_exp()?;
        while self.check(&TokenKind::KwAs) {
            let loc = self.loc();
            self.advance();
            let target = self.parse_type()?;
            exp = Exp::ExpCast {
                loc,
                exp: Box::new(exp),
                target,
            };
        }
        Ok(exp)
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
            TokenKind::LCurly => self.parse_init_list(),
            // `match` também é expressão (T75): `local r = match e with ... end`.
            // Não há ambiguidade com o comando — `parse_stat` consome o
            // `match` antes de chegar aqui.
            TokenKind::KwMatch => self.parse_exp_match(loc),
            _ => Err(self.erro("Esperava uma expressão.")),
        }
    }

    /// `{` em posição de expressão (`ast.lua`: `ExpInitList`) — literal de
    /// array, record ou map. O parser não desambigua qual dos três é: essa
    /// decisão é semântica (T29, `checker.lua:646-662`), porque `{}` vazio só
    /// se resolve por contexto.
    fn parse_init_list(&mut self) -> Result<Exp, ParseError> {
        let loc = self.loc();
        self.advance(); // consome '{'

        let mut fields = Vec::new();
        while !self.check(&TokenKind::RCurly) && !self.check(&TokenKind::Eof) {
            fields.push(self.parse_field()?);
            if !self.eat(&TokenKind::Comma) && !self.eat(&TokenKind::Semicolon) {
                break;
            }
        }
        self.expect(
            &TokenKind::RCurly,
            "Esperava '}' para fechar o inicializador.",
        )?;

        Ok(Exp::ExpInitList { loc, fields })
    }

    /// Um campo de `ExpInitList`: `[ exp ] = exp` (chave-expressão),
    /// `nome = exp` (chave-nome, lookahead de 2: `Name` seguido de `=`; senão
    /// é expressão que começa com nome) ou `exp` (posicional).
    fn parse_field(&mut self) -> Result<Field, ParseError> {
        let loc = self.loc();

        if self.check(&TokenKind::LBracket) {
            self.advance();
            let key = self.parse_exp()?;
            self.expect(
                &TokenKind::RBracket,
                "Esperava ']' para fechar a chave do campo.",
            )?;
            self.expect(&TokenKind::Assign, "Esperava '=' após a chave do campo.")?;
            let exp = self.parse_exp()?;
            return Ok(Field {
                loc,
                name: FieldName::Key(Box::new(key)),
                exp,
            });
        }

        if matches!(self.peek().kind, TokenKind::Name(_)) && self.peek2().kind == TokenKind::Assign
        {
            let (name, _) = self.expect_name("Esperava um nome de campo.")?;
            self.advance(); // consome '='
            let exp = self.parse_exp()?;
            return Ok(Field {
                loc,
                name: FieldName::Name(name),
                exp,
            });
        }

        let exp = self.parse_exp()?;
        Ok(Field {
            loc,
            name: FieldName::None,
            exp,
        })
    }

    /// Expressão primária (nome ou `( exp )`) seguida de zero ou mais
    /// sufixos: `(` chamada, `[` indexação (`VarBracket`), `.` campo
    /// (`VarDot`), `:` chamada de método (`ArgsMethod`, T72).
    /// `VarBracket`/`VarDot` são embrulhados em `ExpVar` para poderem
    /// seguir sendo sufixados (`a[1].campo[2]`).
    fn parse_suffixed_exp(&mut self) -> Result<Exp, ParseError> {
        let mut exp = self.parse_primary_exp()?;

        loop {
            if self.check(&TokenKind::LParen) {
                let call_loc = self.loc();
                let args = self.parse_call_args()?;
                exp = Exp::ExpCall {
                    loc: call_loc,
                    exp: Box::new(exp),
                    args,
                };
            } else if self.check(&TokenKind::Colon) {
                // `df:soma(...)` (T72): a forma do Titan original, que o
                // ADR 0014 rejeitava. O receptor fica na própria `ExpCall`
                // e o nome do método em `Args::ArgsMethod` — o checker
                // resolve no mesmo ponto em que `.` resolve, então as duas
                // formas produzem o mesmo `Callee::Method`.
                let call_loc = self.loc();
                self.advance();
                let (method, _) = self.expect_name("Esperava um nome de método após ':'.")?;
                let Args::ArgsFunc { loc, args } = self.parse_call_args()? else {
                    unreachable!("parse_call_args só produz ArgsFunc");
                };
                exp = Exp::ExpCall {
                    loc: call_loc,
                    exp: Box::new(exp),
                    args: Args::ArgsMethod { loc, method, args },
                };
            } else if self.check(&TokenKind::LBracket) {
                let loc = self.loc();
                self.advance();
                let index = self.parse_exp()?;
                self.expect(
                    &TokenKind::RBracket,
                    "Esperava ']' para fechar a indexação.",
                )?;
                exp = Exp::ExpVar {
                    loc,
                    var: Box::new(Var::VarBracket {
                        loc,
                        exp1: Box::new(exp),
                        exp2: Box::new(index),
                    }),
                };
            } else if self.check(&TokenKind::Dot) {
                self.advance();
                // `loc` do nome do campo, não do `.` (T49: é o range que
                // hover/go-to-definition do LSP precisam apontar).
                let (name, loc) = self.expect_name("Esperava um nome de campo após '.'.")?;
                exp = Exp::ExpVar {
                    loc,
                    var: Box::new(Var::VarDot {
                        loc,
                        exp: Box::new(exp),
                        name,
                    }),
                };
            } else {
                break;
            }
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
    fn arquivo_vazio_produz_programa_vazio() {
        let program = parse_source("").unwrap_or_else(|e| panic!("esperava sucesso: {e}"));
        assert!(program.is_empty());
    }

    // ---- Fase 1 (T11): statements novos --------------------------------

    /// Extrai os statements do corpo da primeira função do fonte.
    fn stats_da_primeira_funcao(source: &str) -> Vec<Stat> {
        let program = parse_source(source).unwrap_or_else(|e| panic!("esperava sucesso: {e}"));
        let TopLevel::TopLevelFunc { block, .. } = &program[0] else {
            panic!("esperava TopLevelFunc");
        };
        let Stat::StatBlock { stats, .. } = block else {
            panic!("esperava StatBlock");
        };
        stats.clone()
    }

    #[test]
    fn if_elseif_else_produz_statif_estruturado() {
        let stats = stats_da_primeira_funcao(
            "function f(a: boolean, b: boolean): integer\n\
             \x20   if a then\n\
             \x20       return 1\n\
             \x20   elseif b then\n\
             \x20       return 2\n\
             \x20   else\n\
             \x20       return 3\n\
             \x20   end\n\
             end",
        );
        let Stat::StatIf {
            thens, elsestat, ..
        } = &stats[0]
        else {
            panic!("esperava StatIf, obteve {:?}", stats[0]);
        };
        assert_eq!(thens.len(), 2);
        assert!(matches!(thens[0].condition, Exp::ExpVar { .. }));
        assert!(matches!(thens[0].block, Stat::StatBlock { .. }));
        let Some(elsestat) = elsestat else {
            panic!("esperava ramo else");
        };
        assert!(matches!(**elsestat, Stat::StatBlock { .. }));
    }

    #[test]
    fn if_sem_else_fica_none() {
        let stats = stats_da_primeira_funcao(
            "function f(a: boolean): integer\n    if a then\n    end\n    return 0\nend",
        );
        let Stat::StatIf {
            thens, elsestat, ..
        } = &stats[0]
        else {
            panic!("esperava StatIf");
        };
        assert_eq!(thens.len(), 1);
        assert!(elsestat.is_none());
    }

    #[test]
    fn return_sem_valor_antes_de_elseif_e_else() {
        let stats = stats_da_primeira_funcao(
            "function f(a: boolean, b: boolean)\n\
             \x20   if a then\n\
             \x20       return\n\
             \x20   elseif b then\n\
             \x20       return\n\
             \x20   else\n\
             \x20       return\n\
             \x20   end\n\
             end",
        );
        assert!(matches!(stats[0], Stat::StatIf { .. }));
    }

    #[test]
    fn while_produz_statwhile_com_condicao_e_bloco() {
        let stats = stats_da_primeira_funcao(
            "function f(): integer\n\
             \x20   local x: integer = 0\n\
             \x20   while x < 10 do\n\
             \x20       x = x + 1\n\
             \x20   end\n\
             \x20   return x\n\
             end",
        );
        let Stat::StatWhile {
            condition, block, ..
        } = &stats[1]
        else {
            panic!("esperava StatWhile, obteve {:?}", stats[1]);
        };
        assert!(matches!(condition, Exp::ExpBinop { op, .. } if op == "<"));
        let Stat::StatBlock { stats: corpo, .. } = block.as_ref() else {
            panic!("esperava StatBlock");
        };
        assert!(matches!(corpo[0], Stat::StatAssign { .. }));
    }

    #[test]
    fn for_sem_inc_fica_none() {
        let stats = stats_da_primeira_funcao(
            "function f(): integer\n    for i = 1, 10 do\n    end\n    return 0\nend",
        );
        let Stat::StatFor {
            decl,
            start,
            finish,
            inc,
            ..
        } = &stats[0]
        else {
            panic!("esperava StatFor, obteve {:?}", stats[0]);
        };
        assert_eq!(decl.name, "i");
        assert_eq!(decl.r#type, None);
        assert!(matches!(**start, Exp::ExpInteger { value: 1, .. }));
        assert!(matches!(**finish, Exp::ExpInteger { value: 10, .. }));
        assert!(inc.is_none());
    }

    /// T71: o token depois da primeira declaração é o que separa as duas
    /// formas de `for` — aqui `in`, sobre um array.
    #[test]
    fn for_in_sobre_array_liga_um_nome() {
        let stats = stats_da_primeira_funcao(
            "function f(): integer\n    for x in v do\n    end\n    return 0\nend",
        );
        let Stat::StatForIn { decls, exp, .. } = &stats[0] else {
            panic!("esperava StatForIn, obteve {:?}", stats[0]);
        };
        assert_eq!(decls.len(), 1);
        assert_eq!(decls[0].name, "x");
        assert_eq!(decls[0].r#type, None);
        assert!(matches!(**exp, Exp::ExpVar { .. }));
    }

    /// T71: a vírgula também abre a forma de iteração — é ela que o parser vê
    /// antes do `in`, e sem ela `for k, v in ...` cairia no erro do `=`.
    #[test]
    fn for_in_sobre_map_liga_dois_nomes_com_tipo_opcional() {
        let stats = stats_da_primeira_funcao(
            "function f(): integer\n    for k: string, n in m do\n    end\n    return 0\nend",
        );
        let Stat::StatForIn { decls, .. } = &stats[0] else {
            panic!("esperava StatForIn, obteve {:?}", stats[0]);
        };
        assert_eq!(decls.len(), 2);
        assert_eq!(decls[0].name, "k");
        assert!(matches!(decls[0].r#type, Some(Type::TypeString { .. })));
        assert_eq!(decls[1].name, "n");
        assert_eq!(decls[1].r#type, None);
    }

    /// T71: um `for` que não é nem numérico nem de iteração precisa citar as
    /// **duas** continuações possíveis — antes da T71 a mensagem só falava do
    /// `=`, e quem escrevesse `for x do` não descobria que `in` existia.
    #[test]
    fn for_sem_igual_nem_in_cita_as_duas_formas() {
        let err = parse_source("function f(): integer\n    for x do\n    end\n    return 0\nend")
            .unwrap_err();
        assert!(
            err.message
                .contains("'=' (for numérico) ou 'in' (for sobre array/map)"),
            "mensagem inesperada: {}",
            err.message
        );
    }

    /// T71: a aridade fica com o checker, mas o `in` em si é obrigatório —
    /// `for k, v do` é erro de sintaxe, não de tipo.
    #[test]
    fn for_com_virgula_sem_in_da_erro_de_sintaxe() {
        let err =
            parse_source("function f(): integer\n    for k, v do\n    end\n    return 0\nend")
                .unwrap_err();
        assert!(
            err.message
                .contains("Esperava 'in' após as variáveis do 'for'."),
            "mensagem inesperada: {}",
            err.message
        );
    }

    #[test]
    fn for_com_tipo_e_inc_explicitos() {
        let stats = stats_da_primeira_funcao(
            "function f(): integer\n    for i: integer = 1, 10, 2 do\n    end\n    return 0\nend",
        );
        let Stat::StatFor { decl, inc, .. } = &stats[0] else {
            panic!("esperava StatFor");
        };
        assert!(matches!(decl.r#type, Some(Type::TypeInteger { .. })));
        let Some(inc) = inc else {
            panic!("esperava inc presente");
        };
        assert!(matches!(**inc, Exp::ExpInteger { value: 2, .. }));
    }

    #[test]
    fn atribuicao_produz_statassign_single_target() {
        let stats = stats_da_primeira_funcao(
            "function f(): integer\n    local x: integer = 0\n    x = x + 1\n    return x\nend",
        );
        let Stat::StatAssign { vars, exps, .. } = &stats[1] else {
            panic!("esperava StatAssign, obteve {:?}", stats[1]);
        };
        assert_eq!(vars.len(), 1);
        assert!(matches!(&vars[0], Var::VarName { name, .. } if name == "x"));
        assert_eq!(exps.len(), 1);
        assert!(matches!(&exps[0], Exp::ExpBinop { op, .. } if op == "+"));
    }

    #[test]
    fn if_sem_then_produz_erro_claro() {
        let err = parse_source("function f(): integer\n    if true\n    end\nend").unwrap_err();
        assert!(err.message.contains("'then'"), "obteve: {}", err.message);
    }

    #[test]
    fn while_sem_do_produz_erro_claro() {
        let err = parse_source("function f(): integer\n    while true\n    end\nend").unwrap_err();
        assert!(err.message.contains("'do'"), "obteve: {}", err.message);
    }

    #[test]
    fn for_sem_limite_final_produz_erro_claro() {
        let err =
            parse_source("function f(): integer\n    for x = 1 do\n    end\nend").unwrap_err();
        assert!(err.message.contains("','"), "obteve: {}", err.message);
    }

    #[test]
    fn atribuir_a_chamada_produz_erro_claro() {
        let err =
            parse_source("function f(): integer\n    f() = 1\n    return 0\nend").unwrap_err();
        assert!(
            err.message.contains("atribuir a uma chamada de função"),
            "obteve: {}",
            err.message
        );
    }

    #[test]
    fn operador_sem_operando_produz_erro_claro() {
        let err =
            parse_source("function f(): integer\n    local x: integer = 1 + = 2\nend").unwrap_err();
        assert!(err.message.contains("expressão"), "obteve: {}", err.message);
    }

    // ---- Fase 1 (T11): precedência de expressões -----------------------

    /// Parseia `exp_src` como a expressão de um `return` e a devolve.
    fn exp_de_return(exp_src: &str) -> Exp {
        let source = format!("function f(): integer\n    return {exp_src}\nend");
        let stats = stats_da_primeira_funcao(&source);
        let Stat::StatReturn { exps, .. } = &stats[0] else {
            panic!("esperava StatReturn");
        };
        exps[0].clone()
    }

    /// Desestrutura um `ExpBinop`, falhando com mensagem clara se não for.
    fn como_binop(exp: &Exp) -> (&Exp, &str, &Exp) {
        let Exp::ExpBinop { lhs, op, rhs, .. } = exp else {
            panic!("esperava ExpBinop, obteve {exp:?}");
        };
        (lhs, op, rhs)
    }

    #[test]
    fn mul_associa_antes_de_add() {
        let exp = exp_de_return("1 + 2 * 3");
        let (lhs, op, rhs) = como_binop(&exp);
        assert_eq!(op, "+");
        assert!(matches!(lhs, Exp::ExpInteger { value: 1, .. }));
        let (l, op_interno, r) = como_binop(rhs);
        assert_eq!(op_interno, "*");
        assert!(matches!(l, Exp::ExpInteger { value: 2, .. }));
        assert!(matches!(r, Exp::ExpInteger { value: 3, .. }));
    }

    #[test]
    fn pow_associa_a_direita() {
        let exp = exp_de_return("2 ^ 3 ^ 2");
        let (lhs, op, rhs) = como_binop(&exp);
        assert_eq!(op, "^");
        assert!(matches!(lhs, Exp::ExpInteger { value: 2, .. }));
        let (l, op_interno, r) = como_binop(rhs);
        assert_eq!(op_interno, "^");
        assert!(matches!(l, Exp::ExpInteger { value: 3, .. }));
        assert!(matches!(r, Exp::ExpInteger { value: 2, .. }));
    }

    #[test]
    fn add_associa_a_esquerda() {
        let exp = exp_de_return("1 - 2 - 3");
        let (lhs, op, rhs) = como_binop(&exp);
        assert_eq!(op, "-");
        assert!(matches!(rhs, Exp::ExpInteger { value: 3, .. }));
        let (l, op_interno, r) = como_binop(lhs);
        assert_eq!(op_interno, "-");
        assert!(matches!(l, Exp::ExpInteger { value: 1, .. }));
        assert!(matches!(r, Exp::ExpInteger { value: 2, .. }));
    }

    #[test]
    fn relacional_associa_antes_de_and() {
        let exp = exp_de_return("a == b and c == d");
        let (lhs, op, rhs) = como_binop(&exp);
        assert_eq!(op, "and");
        let (_, op_esq, _) = como_binop(lhs);
        assert_eq!(op_esq, "==");
        let (_, op_dir, _) = como_binop(rhs);
        assert_eq!(op_dir, "==");
    }

    #[test]
    fn relacional_nao_encadeia() {
        let err = parse_source("function f(): boolean\n    return 1 < 2 < 3\nend").unwrap_err();
        assert!(!err.message.is_empty());
    }

    #[test]
    fn menos_unario_aninha() {
        let exp = exp_de_return("- -1");
        let Exp::ExpUnop { op, exp, .. } = &exp else {
            panic!("esperava ExpUnop, obteve {exp:?}");
        };
        assert_eq!(op, "-");
        let Exp::ExpUnop { op, exp, .. } = exp.as_ref() else {
            panic!("esperava ExpUnop aninhado");
        };
        assert_eq!(op, "-");
        assert!(matches!(**exp, Exp::ExpInteger { value: 1, .. }));
    }

    #[test]
    fn not_aninha() {
        let exp = exp_de_return("not not true");
        let Exp::ExpUnop { op, exp, .. } = &exp else {
            panic!("esperava ExpUnop, obteve {exp:?}");
        };
        assert_eq!(op, "not");
        let Exp::ExpUnop { op, exp, .. } = exp.as_ref() else {
            panic!("esperava ExpUnop aninhado");
        };
        assert_eq!(op, "not");
        assert!(matches!(**exp, Exp::ExpBool { value: true, .. }));
    }

    #[test]
    fn concat_aceita_operandos_aritmeticos() {
        let exp = exp_de_return(r#""x: " .. 1 + 2"#);
        let Exp::ExpConcat { exps, .. } = &exp else {
            panic!("esperava ExpConcat, obteve {exp:?}");
        };
        assert_eq!(exps.len(), 2);
        assert!(matches!(&exps[1], Exp::ExpBinop { op, .. } if op == "+"));
    }

    #[test]
    fn parenteses_vencem_precedencia() {
        let exp = exp_de_return("(1 + 2) * 3");
        let (lhs, op, rhs) = como_binop(&exp);
        assert_eq!(op, "*");
        assert!(matches!(rhs, Exp::ExpInteger { value: 3, .. }));
        let (_, op_interno, _) = como_binop(lhs);
        assert_eq!(op_interno, "+");
    }

    // ---- T22: parse_type completo (map, TypeName, value) -----------------

    /// Parseia `local x: <tipo> = nil` isoladamente e devolve o `Type` da
    /// declaração — a forma mais direta de exercitar `parse_type` sem
    /// precisar de uma função de topo inteira.
    fn parse_type_source(tipo: &str) -> Result<Type, ParseError> {
        let source =
            format!("local function f(): integer\n    local x: {tipo} = nil\n    return 0\nend");
        let program = parse_source(&source)?;
        let TopLevel::TopLevelFunc { block, .. } = &program[0] else {
            panic!("esperava TopLevelFunc");
        };
        let Stat::StatBlock { stats, .. } = block else {
            panic!("esperava StatBlock");
        };
        let Stat::StatDecl { decls, .. } = &stats[0] else {
            panic!("esperava StatDecl");
        };
        Ok(decls[0]
            .r#type
            .clone()
            .expect("tipo deveria estar presente"))
    }

    #[test]
    fn parse_type_aceita_value() {
        let ty = parse_type_source("value").unwrap_or_else(|e| panic!("esperava sucesso: {e}"));
        assert!(matches!(ty, Type::TypeValue { .. }));
    }

    #[test]
    fn parse_type_aceita_nome_de_record() {
        let ty = parse_type_source("Ponto").unwrap_or_else(|e| panic!("esperava sucesso: {e}"));
        assert!(matches!(ty, Type::TypeName { name, .. } if name == "Ponto"));
    }

    #[test]
    fn parse_type_aceita_array() {
        let ty = parse_type_source("{integer}").unwrap_or_else(|e| panic!("esperava sucesso: {e}"));
        let Type::TypeArray { subtype, .. } = ty else {
            panic!("esperava TypeArray, obteve {ty:?}");
        };
        assert!(matches!(*subtype, Type::TypeInteger { .. }));
    }

    #[test]
    fn parse_type_aceita_array_de_array() {
        let ty =
            parse_type_source("{{integer}}").unwrap_or_else(|e| panic!("esperava sucesso: {e}"));
        let Type::TypeArray { subtype, .. } = ty else {
            panic!("esperava TypeArray, obteve {ty:?}");
        };
        let Type::TypeArray { subtype, .. } = *subtype else {
            panic!("esperava TypeArray aninhado, obteve {subtype:?}");
        };
        assert!(matches!(*subtype, Type::TypeInteger { .. }));
    }

    #[test]
    fn parse_type_aceita_map() {
        let ty = parse_type_source("{string: integer}")
            .unwrap_or_else(|e| panic!("esperava sucesso: {e}"));
        let Type::TypeMap {
            keystype,
            valuestype,
            ..
        } = ty
        else {
            panic!("esperava TypeMap, obteve {ty:?}");
        };
        assert!(matches!(*keystype, Type::TypeString { .. }));
        assert!(matches!(*valuestype, Type::TypeInteger { .. }));
    }

    #[test]
    fn parse_type_map_com_chave_ausente_produz_erro_claro() {
        let err = parse_type_source("{: integer}").unwrap_err();
        assert!(!err.message.is_empty());
    }

    #[test]
    fn parse_type_map_com_valor_ausente_produz_erro_claro() {
        let err = parse_type_source("{integer:}").unwrap_err();
        assert!(!err.message.is_empty());
    }

    #[test]
    fn parse_type_array_sem_fechar_produz_erro_claro() {
        let err = parse_type_source("{integer").unwrap_err();
        assert!(err.message.contains('}'));
    }

    // ---- T23: loop de sufixos ([, ., () e `record` no topo ---------------

    /// Parseia `local x = <exp>` isoladamente e devolve a `Exp`.
    fn parse_exp_source(exp: &str) -> Result<Exp, ParseError> {
        let source = format!("local function f(): integer\n    local x = {exp}\n    return 0\nend");
        let program = parse_source(&source)?;
        let TopLevel::TopLevelFunc { block, .. } = &program[0] else {
            panic!("esperava TopLevelFunc");
        };
        let Stat::StatBlock { stats, .. } = block else {
            panic!("esperava StatBlock");
        };
        let Stat::StatDecl { exps, .. } = &stats[0] else {
            panic!("esperava StatDecl");
        };
        Ok(exps[0].clone())
    }

    /// Parseia um comando isolado dentro do corpo de uma função.
    fn parse_stat_source(stat: &str) -> Result<Stat, ParseError> {
        let source = format!("local function f(): integer\n    {stat}\n    return 0\nend");
        let program = parse_source(&source)?;
        let TopLevel::TopLevelFunc { block, .. } = &program[0] else {
            panic!("esperava TopLevelFunc");
        };
        let Stat::StatBlock { stats, .. } = block else {
            panic!("esperava StatBlock");
        };
        Ok(stats[0].clone())
    }

    #[test]
    fn parse_indexacao_simples() {
        let exp = parse_exp_source("v[1]").unwrap_or_else(|e| panic!("esperava sucesso: {e}"));
        let Exp::ExpVar { var, .. } = exp else {
            panic!("esperava ExpVar, obteve {exp:?}");
        };
        let Var::VarBracket { exp1, exp2, .. } = *var else {
            panic!("esperava VarBracket");
        };
        let Exp::ExpVar { var, .. } = *exp1 else {
            panic!("esperava ExpVar em exp1");
        };
        assert!(matches!(*var, Var::VarName { .. }));
        assert!(matches!(*exp2, Exp::ExpInteger { value: 1, .. }));
    }

    #[test]
    fn parse_indexacao_com_expressao() {
        let exp = parse_exp_source("v[i+1]").unwrap_or_else(|e| panic!("esperava sucesso: {e}"));
        let Exp::ExpVar { var, .. } = exp else {
            panic!("esperava ExpVar, obteve {exp:?}");
        };
        let Var::VarBracket { exp2, .. } = *var else {
            panic!("esperava VarBracket");
        };
        assert!(matches!(*exp2, Exp::ExpBinop { .. }));
    }

    #[test]
    fn parse_indexacao_aninhada() {
        let exp = parse_exp_source("a[1][2]").unwrap_or_else(|e| panic!("esperava sucesso: {e}"));
        let Exp::ExpVar { var, .. } = exp else {
            panic!("esperava ExpVar, obteve {exp:?}");
        };
        let Var::VarBracket { exp1, exp2, .. } = *var else {
            panic!("esperava VarBracket externo");
        };
        assert!(matches!(*exp2, Exp::ExpInteger { value: 2, .. }));
        let Exp::ExpVar { var, .. } = *exp1 else {
            panic!("esperava ExpVar interno");
        };
        assert!(matches!(*var, Var::VarBracket { .. }));
    }

    #[test]
    fn parse_acesso_a_campo() {
        let exp = parse_exp_source("p.x").unwrap_or_else(|e| panic!("esperava sucesso: {e}"));
        let Exp::ExpVar { var, .. } = exp else {
            panic!("esperava ExpVar, obteve {exp:?}");
        };
        let Var::VarDot { exp, name, .. } = *var else {
            panic!("esperava VarDot");
        };
        assert_eq!(name, "x");
        assert!(matches!(*exp, Exp::ExpVar { .. }));
    }

    #[test]
    fn parse_acesso_a_campo_encadeado() {
        let exp = parse_exp_source("p.a.b").unwrap_or_else(|e| panic!("esperava sucesso: {e}"));
        let Exp::ExpVar { var, .. } = exp else {
            panic!("esperava ExpVar, obteve {exp:?}");
        };
        let Var::VarDot { exp, name, .. } = *var else {
            panic!("esperava VarDot externo");
        };
        assert_eq!(name, "b");
        let Exp::ExpVar { var, .. } = *exp else {
            panic!("esperava ExpVar interno");
        };
        assert!(matches!(*var, Var::VarDot { .. }));
    }

    #[test]
    fn parse_chamada_seguida_de_indexacao_e_campo() {
        let exp = parse_exp_source("f()[1].c").unwrap_or_else(|e| panic!("esperava sucesso: {e}"));
        let Exp::ExpVar { var, .. } = exp else {
            panic!("esperava ExpVar, obteve {exp:?}");
        };
        let Var::VarDot { exp, name, .. } = *var else {
            panic!("esperava VarDot externo");
        };
        assert_eq!(name, "c");
        let Exp::ExpVar { var, .. } = *exp else {
            panic!("esperava ExpVar (indexação)");
        };
        let Var::VarBracket { exp1, .. } = *var else {
            panic!("esperava VarBracket");
        };
        assert!(matches!(*exp1, Exp::ExpCall { .. }));
    }

    #[test]
    fn parse_atribuicao_a_indexacao() {
        let stat =
            parse_stat_source("v[1] = 2").unwrap_or_else(|e| panic!("esperava sucesso: {e}"));
        let Stat::StatAssign { vars, exps, .. } = stat else {
            panic!("esperava StatAssign, obteve {stat:?}");
        };
        assert!(matches!(vars[0], Var::VarBracket { .. }));
        assert!(matches!(exps[0], Exp::ExpInteger { value: 2, .. }));
    }

    #[test]
    fn parse_atribuicao_a_campo() {
        let stat = parse_stat_source("p.x = 3").unwrap_or_else(|e| panic!("esperava sucesso: {e}"));
        let Stat::StatAssign { vars, exps, .. } = stat else {
            panic!("esperava StatAssign, obteve {stat:?}");
        };
        assert!(matches!(vars[0], Var::VarDot { .. }));
        assert!(matches!(exps[0], Exp::ExpInteger { value: 3, .. }));
    }

    #[test]
    fn parse_indexacao_sem_expressao_produz_erro_claro() {
        let err = parse_exp_source("v[]").unwrap_err();
        assert!(!err.message.is_empty());
    }

    #[test]
    fn parse_indexacao_sem_fechar_produz_erro_claro() {
        let err = parse_exp_source("v[1").unwrap_err();
        assert!(err.message.contains(']'));
    }

    #[test]
    fn parse_campo_sem_nome_produz_erro_claro() {
        let err = parse_exp_source("p.").unwrap_err();
        assert!(!err.message.is_empty());
    }

    #[test]
    fn parse_record_com_campos() {
        let program = parse_source("record Ponto\n    x: float\n    y: float\nend")
            .unwrap_or_else(|e| panic!("esperava sucesso: {e}"));
        assert_eq!(program.len(), 1);
        let TopLevel::TopLevelRecord { name, fields, .. } = &program[0] else {
            panic!("esperava TopLevelRecord, obteve {:?}", program[0]);
        };
        assert_eq!(name, "Ponto");
        assert_eq!(fields.len(), 2);
        assert_eq!(fields[0].name, "x");
        assert!(matches!(fields[0].r#type, Some(Type::TypeFloat { .. })));
        assert_eq!(fields[1].name, "y");
        assert!(matches!(fields[1].r#type, Some(Type::TypeFloat { .. })));
    }

    #[test]
    fn parse_record_sem_nome_produz_erro_claro() {
        let err = parse_source("record end").unwrap_err();
        assert!(!err.message.is_empty());
    }

    #[test]
    fn parse_record_com_campo_sem_tipo_produz_erro_claro() {
        let err = parse_source("record P\n    x\nend").unwrap_err();
        assert!(!err.message.is_empty());
    }

    // ---- T28: ExpInitList (literais de array, record e map) --------------

    #[test]
    fn parse_init_list_vazio() {
        let exp = parse_exp_source("{}").unwrap_or_else(|e| panic!("esperava sucesso: {e}"));
        let Exp::ExpInitList { fields, .. } = exp else {
            panic!("esperava ExpInitList, obteve {exp:?}");
        };
        assert!(fields.is_empty());
    }

    #[test]
    fn parse_init_list_posicional() {
        let exp = parse_exp_source("{1,2,3}").unwrap_or_else(|e| panic!("esperava sucesso: {e}"));
        let Exp::ExpInitList { fields, .. } = exp else {
            panic!("esperava ExpInitList, obteve {exp:?}");
        };
        assert_eq!(fields.len(), 3);
        for (field, esperado) in fields.iter().zip([1, 2, 3]) {
            assert_eq!(field.name, FieldName::None);
            assert!(matches!(field.exp, Exp::ExpInteger { value, .. } if value == esperado));
        }
    }

    #[test]
    fn parse_init_list_com_virgula_final() {
        let exp = parse_exp_source("{1,2,}").unwrap_or_else(|e| panic!("esperava sucesso: {e}"));
        let Exp::ExpInitList { fields, .. } = exp else {
            panic!("esperava ExpInitList, obteve {exp:?}");
        };
        assert_eq!(fields.len(), 2);
    }

    #[test]
    fn parse_init_list_nomeado() {
        let exp =
            parse_exp_source("{x=1, y=2}").unwrap_or_else(|e| panic!("esperava sucesso: {e}"));
        let Exp::ExpInitList { fields, .. } = exp else {
            panic!("esperava ExpInitList, obteve {exp:?}");
        };
        assert_eq!(fields.len(), 2);
        assert_eq!(fields[0].name, FieldName::Name("x".to_string()));
        assert!(matches!(fields[0].exp, Exp::ExpInteger { value: 1, .. }));
        assert_eq!(fields[1].name, FieldName::Name("y".to_string()));
        assert!(matches!(fields[1].exp, Exp::ExpInteger { value: 2, .. }));
    }

    #[test]
    fn parse_init_list_chave_expressao() {
        let exp =
            parse_exp_source(r#"{["a"]=1}"#).unwrap_or_else(|e| panic!("esperava sucesso: {e}"));
        let Exp::ExpInitList { fields, .. } = exp else {
            panic!("esperava ExpInitList, obteve {exp:?}");
        };
        assert_eq!(fields.len(), 1);
        let FieldName::Key(key) = &fields[0].name else {
            panic!("esperava FieldName::Key, obteve {:?}", fields[0].name);
        };
        assert!(matches!(**key, Exp::ExpString { ref value, .. } if value == "a"));
        assert!(matches!(fields[0].exp, Exp::ExpInteger { value: 1, .. }));
    }

    #[test]
    fn parse_init_list_aninhado() {
        let exp = parse_exp_source("{{1},{2}}").unwrap_or_else(|e| panic!("esperava sucesso: {e}"));
        let Exp::ExpInitList { fields, .. } = exp else {
            panic!("esperava ExpInitList, obteve {exp:?}");
        };
        assert_eq!(fields.len(), 2);
        for field in &fields {
            assert!(matches!(field.exp, Exp::ExpInitList { .. }));
        }
    }

    #[test]
    fn parse_init_list_misto_parseia_checker_rejeita_depois() {
        // O parser não desambigua array/record/map — só o checker (T29).
        let exp = parse_exp_source("{1, x=2}").unwrap_or_else(|e| panic!("esperava sucesso: {e}"));
        let Exp::ExpInitList { fields, .. } = exp else {
            panic!("esperava ExpInitList, obteve {exp:?}");
        };
        assert_eq!(fields.len(), 2);
        assert_eq!(fields[0].name, FieldName::None);
        assert_eq!(fields[1].name, FieldName::Name("x".to_string()));
    }

    #[test]
    fn parse_init_list_virgula_dupla_produz_erro_claro() {
        let err = parse_exp_source("{1,,2}").unwrap_err();
        assert!(!err.message.is_empty());
    }

    #[test]
    fn parse_init_list_campo_nomeado_sem_valor_produz_erro_claro() {
        let err = parse_exp_source("{x=}").unwrap_err();
        assert!(!err.message.is_empty());
    }

    #[test]
    fn parse_init_list_sem_fechar_produz_erro_claro() {
        let err = parse_exp_source("{1").unwrap_err();
        assert!(!err.message.is_empty());
    }

    // ---- T35: `import data` e `parse_type` qualificado -------------------

    #[test]
    fn parse_import_produz_toplevelimport_com_localname_igual_modname() {
        let program =
            parse_source("import data").unwrap_or_else(|e| panic!("esperava sucesso: {e}"));
        assert_eq!(program.len(), 1);
        let TopLevel::TopLevelImport {
            localname, modname, ..
        } = &program[0]
        else {
            panic!("esperava TopLevelImport, obteve {:?}", program[0]);
        };
        assert_eq!(localname, "data");
        assert_eq!(modname, "data");
    }

    #[test]
    fn parse_type_aceita_qualificado_de_modulo_importado() {
        let ty =
            parse_type_source("data.DataFrame").unwrap_or_else(|e| panic!("esperava sucesso: {e}"));
        let Type::TypeQualName { module, name, .. } = ty else {
            panic!("esperava TypeQualName, obteve {ty:?}");
        };
        assert_eq!(module, "data");
        assert_eq!(name, "DataFrame");
    }

    #[test]
    fn parse_type_sem_dot_continua_typename() {
        let ty = parse_type_source("DataFrame").unwrap_or_else(|e| panic!("esperava sucesso: {e}"));
        assert!(matches!(ty, Type::TypeName { name, .. } if name == "DataFrame"));
    }

    #[test]
    fn parse_import_sem_nome_produz_erro_claro() {
        let err = parse_source("import").unwrap_err();
        assert!(!err.message.is_empty());
    }

    #[test]
    fn parse_import_com_string_produz_erro_claro() {
        let err = parse_source(r#"import "data""#).unwrap_err();
        assert!(!err.message.is_empty());
    }

    // ---- T72: `import` com alias e `df:metodo()` -------------------------

    #[test]
    fn parse_import_com_as_produz_localname_diferente_de_modname() {
        let program =
            parse_source("import data as d").unwrap_or_else(|e| panic!("esperava sucesso: {e}"));
        assert_eq!(program.len(), 1);
        let TopLevel::TopLevelImport {
            localname, modname, ..
        } = &program[0]
        else {
            panic!("esperava TopLevelImport, obteve {:?}", program[0]);
        };
        assert_eq!(localname, "d");
        assert_eq!(modname, "data");
    }

    #[test]
    fn parse_import_com_as_sem_nome_produz_erro_claro() {
        let err = parse_source("import data as").unwrap_err();
        assert!(
            err.message.contains("nome local"),
            "obteve: {}",
            err.message
        );
    }

    /// `df:soma("valor")` produz `ExpCall` com `Args::ArgsMethod` — o
    /// receptor fica no `exp` da chamada e o nome do método nos argumentos.
    #[test]
    fn parse_chamada_com_dois_pontos_produz_argsmethod() {
        let exp = parse_exp_source(r#"df:soma("valor")"#)
            .unwrap_or_else(|e| panic!("esperava sucesso: {e}"));
        let Exp::ExpCall { exp, args, .. } = exp else {
            panic!("esperava ExpCall, obteve {exp:?}");
        };
        let Exp::ExpVar { var, .. } = exp.as_ref() else {
            panic!("esperava ExpVar como receptor");
        };
        assert!(matches!(var.as_ref(), Var::VarName { name, .. } if name == "df"));
        let Args::ArgsMethod { method, args, .. } = args else {
            panic!("esperava ArgsMethod, obteve {args:?}");
        };
        assert_eq!(method, "soma");
        assert_eq!(args.len(), 1);
        assert!(matches!(&args[0], Exp::ExpString { value, .. } if value == "valor"));
    }

    /// O `:` é sufixo como `.` e `[`, então encadeia com eles.
    #[test]
    fn parse_dois_pontos_encadeia_com_outros_sufixos() {
        let exp =
            parse_exp_source("a.b:c(1).d").unwrap_or_else(|e| panic!("esperava sucesso: {e}"));
        let Exp::ExpVar { var, .. } = &exp else {
            panic!("esperava ExpVar (o `.d` final), obteve {exp:?}");
        };
        let Var::VarDot { exp, name, .. } = var.as_ref() else {
            panic!("esperava VarDot");
        };
        assert_eq!(name, "d");
        let Exp::ExpCall { args, .. } = exp.as_ref() else {
            panic!("esperava ExpCall sob o `.d`");
        };
        assert!(matches!(args, Args::ArgsMethod { method, .. } if method == "c"));
    }

    #[test]
    fn parse_dois_pontos_sem_nome_de_metodo_produz_erro_claro() {
        let err = parse_exp_source("df:(1)").unwrap_err();
        assert!(
            err.message.contains("nome de método"),
            "obteve: {}",
            err.message
        );
    }

    #[test]
    fn erro_de_toplevel_menciona_import() {
        let err = parse_source("42").unwrap_err();
        assert!(err.message.contains("import"), "obteve: {}", err.message);
    }

    // ---- T73: `foreign function` -----------------------------------------

    #[test]
    fn parse_foreign_function_produz_toplevelforeignfunc() {
        let program = parse_source("foreign function abs(n: integer): integer")
            .unwrap_or_else(|e| panic!("esperava sucesso: {e}"));
        assert_eq!(program.len(), 1);
        let TopLevel::TopLevelForeignFunc {
            name,
            params,
            rettypes,
            ..
        } = &program[0]
        else {
            panic!("esperava TopLevelForeignFunc, obteve {:?}", program[0]);
        };
        assert_eq!(name, "abs");
        assert_eq!(params.len(), 1);
        assert_eq!(params[0].name, "n");
        assert!(matches!(params[0].r#type, Some(Type::TypeInteger { .. })));
        assert_eq!(rettypes.len(), 1);
        assert!(matches!(rettypes[0], Type::TypeInteger { .. }));
    }

    /// Sem parâmetros e sem retorno — retorno omitido vira `TypeNil`, exatamente
    /// como numa `function` comum (`parse_rettypes_opt`).
    #[test]
    fn parse_foreign_function_sem_params_nem_retorno() {
        let program = parse_source("foreign function sync()")
            .unwrap_or_else(|e| panic!("esperava sucesso: {e}"));
        let TopLevel::TopLevelForeignFunc {
            params, rettypes, ..
        } = &program[0]
        else {
            panic!("esperava TopLevelForeignFunc, obteve {:?}", program[0]);
        };
        assert!(params.is_empty());
        assert_eq!(rettypes.len(), 1);
        assert!(matches!(rettypes[0], Type::TypeNil { .. }));
    }

    #[test]
    fn parse_foreign_function_convive_com_function_no_mesmo_arquivo() {
        let program = parse_source(
            "foreign function abs(n: integer): integer\n\n\
             function main(args: {string}): integer\n\
             \x20   return abs(-1)\n\
             end",
        )
        .unwrap_or_else(|e| panic!("esperava sucesso: {e}"));
        assert_eq!(program.len(), 2);
        assert!(matches!(program[0], TopLevel::TopLevelForeignFunc { .. }));
        assert!(matches!(program[1], TopLevel::TopLevelFunc { .. }));
    }

    /// A forma do Titan original tem erro **próprio**, que aponta a grafia
    /// que existe — não o genérico "esperava 'function' após 'foreign'".
    #[test]
    fn parse_foreign_import_do_original_produz_erro_que_aponta_a_grafia_nova() {
        let err = parse_source(r#"foreign import stdio "stdio.h""#).unwrap_err();
        assert!(
            err.message
                .contains("foreign function abs(n: integer): integer"),
            "obteve: {}",
            err.message
        );
    }

    #[test]
    fn parse_foreign_sem_function_produz_erro_claro() {
        let err = parse_source("foreign abs(n: integer): integer").unwrap_err();
        assert!(
            err.message.contains("'function' após 'foreign'"),
            "obteve: {}",
            err.message
        );
    }

    #[test]
    fn parse_foreign_function_sem_nome_produz_erro_claro() {
        let err = parse_source("foreign function (n: integer): integer").unwrap_err();
        assert!(
            err.message.contains("nome de função"),
            "obteve: {}",
            err.message
        );
    }

    /// `end` é o erro de quem copiou o molde de `function`; a mensagem diz o
    /// que fazer, em vez de só recusar o token.
    #[test]
    fn parse_foreign_function_com_end_produz_erro_que_manda_remover_o_end() {
        let err = parse_source("foreign function abs(n: integer): integer\nend").unwrap_err();
        assert!(
            err.message.contains("não tem corpo") && err.message.contains("remova o 'end'"),
            "obteve: {}",
            err.message
        );
    }

    #[test]
    fn parse_foreign_function_sem_parenteses_produz_erro_claro() {
        let err = parse_source("foreign function abs: integer").unwrap_err();
        assert!(err.message.contains("'('"), "obteve: {}", err.message);
    }

    /// Parâmetro sem tipo para no parser (`parse_decl` exige o `:`), não no
    /// checker — a assinatura de uma função externa nunca é inferida.
    #[test]
    fn parse_foreign_function_com_param_sem_tipo_produz_erro_claro() {
        let err = parse_source("foreign function abs(n): integer").unwrap_err();
        assert!(!err.message.is_empty());
    }

    #[test]
    fn erro_de_toplevel_menciona_foreign_function() {
        let err = parse_source("42").unwrap_err();
        assert!(
            err.message.contains("foreign function"),
            "obteve: {}",
            err.message
        );
    }

    // ---- T60: bitwise e `//` na cascata de precedência -------------------

    /// Devolve `(op, lhs, rhs)` de um `ExpBinop`, falhando com a forma real
    /// quando a expressão não é binária — o que torna o erro de um teste de
    /// precedência legível sem depurador.
    fn binop_parts(exp: &Exp) -> (&str, &Exp, &Exp) {
        let Exp::ExpBinop { op, lhs, rhs, .. } = exp else {
            panic!("esperava ExpBinop, obteve {exp:?}");
        };
        (op.as_str(), lhs, rhs)
    }

    /// Devolve `(op, exp)` de um `ExpUnop`.
    fn unop_parts(exp: &Exp) -> (&str, &Exp) {
        let Exp::ExpUnop { op, exp, .. } = exp else {
            panic!("esperava ExpUnop, obteve {exp:?}");
        };
        (op.as_str(), exp)
    }

    #[test]
    fn parse_or_bitwise_produz_binop_com_barra_vertical() {
        let exp = parse_exp_source("1 | 2").unwrap_or_else(|e| panic!("esperava sucesso: {e}"));
        let (op, _, _) = binop_parts(&exp);
        assert_eq!(op, "|");
    }

    #[test]
    fn parse_xor_binario_produz_binop_com_til() {
        let exp = parse_exp_source("a ~ b").unwrap_or_else(|e| panic!("esperava sucesso: {e}"));
        let (op, _, _) = binop_parts(&exp);
        assert_eq!(op, "~");
    }

    #[test]
    fn parse_and_bitwise_produz_binop_com_e_comercial() {
        let exp = parse_exp_source("1 & 2").unwrap_or_else(|e| panic!("esperava sucesso: {e}"));
        let (op, _, _) = binop_parts(&exp);
        assert_eq!(op, "&");
    }

    #[test]
    fn parse_shifts_produzem_binop_com_as_grafias_do_original() {
        let exp = parse_exp_source("1 << 2").unwrap_or_else(|e| panic!("esperava sucesso: {e}"));
        assert_eq!(binop_parts(&exp).0, "<<");
        let exp = parse_exp_source("1 >> 2").unwrap_or_else(|e| panic!("esperava sucesso: {e}"));
        assert_eq!(binop_parts(&exp).0, ">>");
    }

    #[test]
    fn parse_divisao_inteira_produz_binop_com_barra_dupla() {
        let exp = parse_exp_source("5 // 2").unwrap_or_else(|e| panic!("esperava sucesso: {e}"));
        let (op, lhs, rhs) = binop_parts(&exp);
        assert_eq!(op, "//");
        assert!(matches!(lhs, Exp::ExpInteger { value: 5, .. }));
        assert!(matches!(rhs, Exp::ExpInteger { value: 2, .. }));
    }

    #[test]
    fn precedencia_or_bitwise_e_mais_fraca_que_and_bitwise() {
        // `1 | 2 & 3` ≡ `1 | (2 & 3)`
        let exp = parse_exp_source("1 | 2 & 3").unwrap_or_else(|e| panic!("esperava sucesso: {e}"));
        let (op, lhs, rhs) = binop_parts(&exp);
        assert_eq!(op, "|");
        assert!(matches!(lhs, Exp::ExpInteger { value: 1, .. }));
        assert_eq!(binop_parts(rhs).0, "&");
    }

    #[test]
    fn precedencia_xor_fica_entre_or_e_and_bitwise() {
        // `1 | 2 ~ 3 & 4` ≡ `1 | (2 ~ (3 & 4))`
        let exp =
            parse_exp_source("1 | 2 ~ 3 & 4").unwrap_or_else(|e| panic!("esperava sucesso: {e}"));
        let (op, _, rhs) = binop_parts(&exp);
        assert_eq!(op, "|");
        let (op, _, rhs) = binop_parts(rhs);
        assert_eq!(op, "~");
        assert_eq!(binop_parts(rhs).0, "&");
    }

    #[test]
    fn precedencia_shift_e_mais_fraca_que_soma() {
        // `1 << 2 + 3` ≡ `1 << (2 + 3)`
        let exp = parse_exp_source("1 << 2 + 3").unwrap_or_else(|e| panic!("esperava sucesso: {e}"));
        let (op, lhs, rhs) = binop_parts(&exp);
        assert_eq!(op, "<<");
        assert!(matches!(lhs, Exp::ExpInteger { value: 1, .. }));
        assert_eq!(binop_parts(rhs).0, "+");
    }

    #[test]
    fn precedencia_shift_e_mais_forte_que_and_bitwise() {
        // `1 & 2 << 3` ≡ `1 & (2 << 3)`
        let exp = parse_exp_source("1 & 2 << 3").unwrap_or_else(|e| panic!("esperava sucesso: {e}"));
        let (op, _, rhs) = binop_parts(&exp);
        assert_eq!(op, "&");
        assert_eq!(binop_parts(rhs).0, "<<");
    }

    #[test]
    fn precedencia_shift_e_mais_fraca_que_concatenacao() {
        // `op7` (shift) vem antes de `op8` (concat) no original, então o
        // `..` liga mais forte: `"s" .. 1 << 2` ≡ `("s" .. 1) << 2`.
        let exp =
            parse_exp_source(r#""s" .. 1 << 2"#).unwrap_or_else(|e| panic!("esperava sucesso: {e}"));
        let (op, lhs, _) = binop_parts(&exp);
        assert_eq!(op, "<<");
        let Exp::ExpConcat { exps, .. } = lhs else {
            panic!("esperava ExpConcat à esquerda, obteve {lhs:?}");
        };
        assert_eq!(exps.len(), 2);
    }

    #[test]
    fn precedencia_relacional_e_mais_fraca_que_or_bitwise() {
        // `1 | 2 == 3` ≡ `(1 | 2) == 3`
        let exp = parse_exp_source("1 | 2 == 3").unwrap_or_else(|e| panic!("esperava sucesso: {e}"));
        let (op, lhs, _) = binop_parts(&exp);
        assert_eq!(op, "==");
        assert_eq!(binop_parts(lhs).0, "|");
    }

    #[test]
    fn precedencia_divisao_inteira_e_igual_a_multiplicacao() {
        // Mesmo nível, associativo à esquerda: `8 // 2 * 3` ≡ `(8 // 2) * 3`.
        let exp = parse_exp_source("8 // 2 * 3").unwrap_or_else(|e| panic!("esperava sucesso: {e}"));
        let (op, lhs, _) = binop_parts(&exp);
        assert_eq!(op, "*");
        assert_eq!(binop_parts(lhs).0, "//");
    }

    #[test]
    fn precedencia_divisao_inteira_e_mais_forte_que_soma() {
        // `1 + 8 // 2` ≡ `1 + (8 // 2)`
        let exp = parse_exp_source("1 + 8 // 2").unwrap_or_else(|e| panic!("esperava sucesso: {e}"));
        let (op, _, rhs) = binop_parts(&exp);
        assert_eq!(op, "+");
        assert_eq!(binop_parts(rhs).0, "//");
    }

    #[test]
    fn til_unario_vira_unop_e_nao_binop() {
        let exp = parse_exp_source("~x").unwrap_or_else(|e| panic!("esperava sucesso: {e}"));
        let (op, operando) = unop_parts(&exp);
        assert_eq!(op, "~");
        assert!(matches!(operando, Exp::ExpVar { .. }));
    }

    #[test]
    fn til_unario_e_binario_convivem_na_mesma_expressao() {
        // `~a ~ b` ≡ `(~a) ~ b`: prefixo é NOT, infixo é XOR.
        let exp = parse_exp_source("~a ~ b").unwrap_or_else(|e| panic!("esperava sucesso: {e}"));
        let (op, lhs, rhs) = binop_parts(&exp);
        assert_eq!(op, "~");
        assert_eq!(unop_parts(lhs).0, "~");
        assert!(matches!(rhs, Exp::ExpVar { .. }));
    }

    #[test]
    fn til_unario_se_aplica_antes_do_xor_binario() {
        // `a ~ ~b` ≡ `a ~ (~b)`
        let exp = parse_exp_source("a ~ ~b").unwrap_or_else(|e| panic!("esperava sucesso: {e}"));
        let (op, _, rhs) = binop_parts(&exp);
        assert_eq!(op, "~");
        assert_eq!(unop_parts(rhs).0, "~");
    }

    #[test]
    fn til_unario_repetido_aninha_unops() {
        let exp = parse_exp_source("~ ~x").unwrap_or_else(|e| panic!("esperava sucesso: {e}"));
        let (op, dentro) = unop_parts(&exp);
        assert_eq!(op, "~");
        assert_eq!(unop_parts(dentro).0, "~");
    }

    #[test]
    fn til_unario_e_mais_forte_que_and_bitwise() {
        // `~a & b` ≡ `(~a) & b`
        let exp = parse_exp_source("~a & b").unwrap_or_else(|e| panic!("esperava sucesso: {e}"));
        let (op, lhs, _) = binop_parts(&exp);
        assert_eq!(op, "&");
        assert_eq!(unop_parts(lhs).0, "~");
    }

    #[test]
    fn diferente_continua_sendo_relacional_e_nao_xor() {
        // O lexer resolve `~=` por lookahead; o parser precisa vê-lo no
        // nível relacional, não no de XOR.
        let exp = parse_exp_source("a ~= b").unwrap_or_else(|e| panic!("esperava sucesso: {e}"));
        assert_eq!(binop_parts(&exp).0, "~=");
    }

    #[test]
    fn parentese_vence_a_cascata_bitwise() {
        // `(1 | 2) & 3` inverte o agrupamento natural de `1 | 2 & 3`.
        let exp =
            parse_exp_source("(1 | 2) & 3").unwrap_or_else(|e| panic!("esperava sucesso: {e}"));
        let (op, lhs, _) = binop_parts(&exp);
        assert_eq!(op, "&");
        assert_eq!(binop_parts(lhs).0, "|");
    }

    #[test]
    fn operador_bitwise_sem_operando_direito_produz_erro_claro() {
        let err = parse_exp_source("1 |").unwrap_err();
        assert!(!err.message.is_empty());
    }

    /// T63: `continue` deixou de ser rejeitado no parser (a mensagem do
    /// ADR 0017 explicava que ele pularia o incremento do `for`, o que a T62
    /// tornou falso) e passou a produzir `StatContinue`, exatamente como
    /// `break` produz `StatBreak`.
    #[test]
    fn continue_produz_stat_continue() {
        let source = r#"function main(args: {string}): integer
    for i = 1, 5 do
        continue
    end
    return 0
end"#;
        let program = parse_source(source).unwrap_or_else(|e| panic!("esperava sucesso: {e}"));
        let TopLevel::TopLevelFunc { block, .. } = &program[0] else {
            panic!("esperava função");
        };
        let Stat::StatBlock { stats, .. } = block else {
            panic!("esperava bloco");
        };
        let Stat::StatFor { block, .. } = &stats[0] else {
            panic!("esperava for");
        };
        let Stat::StatBlock { stats, .. } = block.as_ref() else {
            panic!("esperava bloco do for");
        };
        assert!(matches!(stats[0], Stat::StatContinue { .. }), "{stats:?}");
    }

    /// T64: `repeat` deixou de morrer em `parse_primary_exp` (onde caía
    /// desde que a T59 o tornou keyword) e passou a produzir o
    /// `StatRepeat` que a AST carrega desde a Fase 0 sem nunca ter sido
    /// construído.
    #[test]
    fn repeat_produz_stat_repeat() {
        let source = r#"function main(args: {string}): integer
    repeat
        print("x")
    until true
    return 0
end"#;
        let program = parse_source(source).unwrap_or_else(|e| panic!("esperava sucesso: {e}"));
        let TopLevel::TopLevelFunc { block, .. } = &program[0] else {
            panic!("esperava função");
        };
        let Stat::StatBlock { stats, .. } = block else {
            panic!("esperava bloco");
        };
        let Stat::StatRepeat {
            block, condition, ..
        } = &stats[0]
        else {
            panic!("esperava StatRepeat, obteve {:?}", stats[0]);
        };
        let Stat::StatBlock { stats: corpo, .. } = block.as_ref() else {
            panic!("esperava bloco do repeat");
        };
        assert_eq!(corpo.len(), 1);
        assert!(matches!(corpo[0], Stat::StatCall { .. }), "{corpo:?}");
        assert!(matches!(condition, Exp::ExpBool { value: true, .. }));
    }

    /// `until` fecha o bloco no lugar do `end`: um `repeat` de uma linha só,
    /// como o do caso negativo que a T64 aposentou, precisa parsear igual.
    #[test]
    fn repeat_de_uma_linha_parseia() {
        let stats = stats_da_primeira_funcao(
            "function f(): integer\n    repeat print(\"x\") until true\n    return 0\nend",
        );
        assert!(matches!(stats[0], Stat::StatRepeat { .. }), "{stats:?}");
    }

    /// O corpo do `repeat` é um bloco como qualquer outro: `break`,
    /// `continue` e laços aninhados entram sem sintaxe própria.
    #[test]
    fn repeat_aninhado_e_com_break_continue() {
        let stats = stats_da_primeira_funcao(
            "function f(): integer\n\
             \x20   repeat\n\
             \x20       repeat\n\
             \x20           continue\n\
             \x20       until true\n\
             \x20       break\n\
             \x20   until false\n\
             \x20   return 0\n\
             end",
        );
        let Stat::StatRepeat { block, .. } = &stats[0] else {
            panic!("esperava StatRepeat externo, obteve {:?}", stats[0]);
        };
        let Stat::StatBlock { stats: corpo, .. } = block.as_ref() else {
            panic!("esperava bloco");
        };
        assert!(matches!(corpo[0], Stat::StatRepeat { .. }), "{corpo:?}");
        assert!(matches!(corpo[1], Stat::StatBreak { .. }), "{corpo:?}");
    }

    /// `repeat` sem `until` não pode consumir o resto da função em silêncio:
    /// o bloco para no `end` e o `expect` acusa o `until` que falta.
    #[test]
    fn repeat_sem_until_produz_erro_claro() {
        let err = parse_source(
            "function f(): integer\n    repeat\n        print(\"x\")\n    end\n    return 0\nend",
        )
        .unwrap_err();
        assert!(err.message.contains("until"), "{}", err.message);
    }

    /// A contrapartida de `until` terminar bloco (T64): fora de um `repeat`
    /// ele para o bloco cedo, e quem estava esperando `end` — o corpo da
    /// função, o `if`, o `while` — acusa a falta do `end` na posição do
    /// `until`. O erro é claro e aponta a linha certa, que é o que importa;
    /// o `until` órfão não passa em silêncio.
    #[test]
    fn until_fora_de_repeat_produz_erro_claro() {
        let err =
            parse_source("function f(): integer\n    until true\n    return 0\nend").unwrap_err();
        assert!(err.message.contains("end"), "{}", err.message);
        assert_eq!(err.loc.line, 2);
    }

    /// Fora de laço o parser **aceita** `continue` — quem rejeita é o
    /// checker, pela profundidade de laço. Mesma divisão de camadas que
    /// `break` tem desde a T55.
    #[test]
    fn continue_fora_de_laco_passa_pelo_parser() {
        let source = r#"function main(args: {string}): integer
    continue
    return 0
end"#;
        assert!(parse_source(source).is_ok());
    }

    // ---- T65: retornos múltiplos na sintaxe ----------------------------

    /// `: integer, integer` na assinatura vira uma lista de dois tipos.
    #[test]
    fn assinatura_aceita_lista_de_tipos_de_retorno() {
        let program = parse_source(
            "function divmod(a: integer, b: integer): integer, integer\n\
             \x20   return a, b\n\
             end",
        )
        .unwrap_or_else(|e| panic!("esperava sucesso: {e}"));

        let TopLevel::TopLevelFunc { rettypes, .. } = &program[0] else {
            panic!("esperava TopLevelFunc");
        };
        assert_eq!(rettypes.len(), 2);
        assert!(matches!(rettypes[0], Type::TypeInteger { .. }));
        assert!(matches!(rettypes[1], Type::TypeInteger { .. }));
    }

    /// Um retorno só continua produzindo lista de um — nada de tupla de 1.
    #[test]
    fn assinatura_de_retorno_unico_continua_com_um_tipo() {
        let program = parse_source(
            "function f(): integer\n\
             \x20   return 1\n\
             end",
        )
        .unwrap_or_else(|e| panic!("esperava sucesso: {e}"));
        let TopLevel::TopLevelFunc { rettypes, .. } = &program[0] else {
            panic!("esperava TopLevelFunc");
        };
        assert_eq!(rettypes.len(), 1);
        assert!(matches!(rettypes[0], Type::TypeInteger { .. }));
    }

    /// `return a, b` produz um `StatReturn` com duas expressões.
    #[test]
    fn return_aceita_lista_de_expressoes() {
        let program = parse_source(
            "function divmod(a: integer, b: integer): integer, integer\n\
             \x20   return a // b, a % b\n\
             end",
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
        assert_eq!(exps.len(), 2);
        assert!(matches!(&exps[0], Exp::ExpBinop { op, .. } if op == "//"));
        assert!(matches!(&exps[1], Exp::ExpBinop { op, .. } if op == "%"));
    }

    /// `return` sem valor continua sendo lista vazia — a vírgula é opcional,
    /// não obrigatória.
    #[test]
    fn return_sem_valor_continua_sem_expressoes() {
        let program = parse_source(
            "function f()\n\
             \x20   return\n\
             end",
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
        assert!(exps.is_empty());
    }

    /// Vírgula pendurada na assinatura é erro de sintaxe, não silêncio.
    #[test]
    fn lista_de_tipos_de_retorno_com_virgula_pendurada_produz_erro() {
        let err = parse_source(
            "function f(): integer,\n\
             \x20   return 1\n\
             end",
        )
        .unwrap_err();
        assert!(err.message.contains("tipo"), "{}", err.message);
    }
    // ---- T67: multi-assign e declaração múltipla na sintaxe ------------

    /// `local a, b = f()` produz um `StatDecl` com dois `Decl` e uma
    /// expressão — a desestruturação é do checker, não do parser.
    #[test]
    fn local_aceita_lista_de_nomes() {
        let program = parse_source(
            "function main(args: {string}): integer\n\
             \x20   local q, r = divmod(7, 2)\n\
             \x20   return 0\n\
             end",
        )
        .unwrap_or_else(|e| panic!("esperava sucesso: {e}"));
        let TopLevel::TopLevelFunc { block, .. } = &program[0] else {
            panic!("esperava TopLevelFunc");
        };
        let Stat::StatBlock { stats, .. } = block else {
            panic!("esperava StatBlock");
        };
        let Stat::StatDecl { decls, exps, .. } = &stats[0] else {
            panic!("esperava StatDecl");
        };
        assert_eq!(decls.len(), 2);
        assert_eq!(decls[0].name, "q");
        assert_eq!(decls[1].name, "r");
        assert_eq!(exps.len(), 1);
    }

    /// Cada nome da lista pode trazer sua própria anotação de tipo.
    #[test]
    fn local_multiplo_aceita_anotacao_em_cada_nome() {
        let program = parse_source(
            "function main(args: {string}): integer\n\
             \x20   local a: integer, b: string = 1, \"x\"\n\
             \x20   return 0\n\
             end",
        )
        .unwrap_or_else(|e| panic!("esperava sucesso: {e}"));
        let TopLevel::TopLevelFunc { block, .. } = &program[0] else {
            panic!("esperava TopLevelFunc");
        };
        let Stat::StatBlock { stats, .. } = block else {
            panic!("esperava StatBlock");
        };
        let Stat::StatDecl { decls, exps, .. } = &stats[0] else {
            panic!("esperava StatDecl");
        };
        assert!(matches!(decls[0].r#type, Some(Type::TypeInteger { .. })));
        assert!(matches!(decls[1].r#type, Some(Type::TypeString { .. })));
        assert_eq!(exps.len(), 2);
    }

    /// `a, b = b, a` produz dois alvos e dois valores. A vírgula entra na
    /// desambiguação com a chamada — sem backtracking, como o `=` sempre
    /// fez.
    #[test]
    fn atribuicao_aceita_lista_de_alvos() {
        let program = parse_source(
            "function main(args: {string}): integer\n\
             \x20   a, b = b, a\n\
             \x20   return 0\n\
             end",
        )
        .unwrap_or_else(|e| panic!("esperava sucesso: {e}"));
        let TopLevel::TopLevelFunc { block, .. } = &program[0] else {
            panic!("esperava TopLevelFunc");
        };
        let Stat::StatBlock { stats, .. } = block else {
            panic!("esperava StatBlock");
        };
        let Stat::StatAssign { vars, exps, .. } = &stats[0] else {
            panic!("esperava StatAssign");
        };
        assert_eq!(vars.len(), 2);
        assert_eq!(exps.len(), 2);
    }

    /// Alvos compostos (`v[i]`, `p.campo`) entram na lista como qualquer
    /// outro — cada um passa pela mesma conversão de expressão para `Var`.
    #[test]
    fn atribuicao_multipla_aceita_alvos_compostos() {
        let program = parse_source(
            "function main(args: {string}): integer\n\
             \x20   v[1], p.x = p.x, v[1]\n\
             \x20   return 0\n\
             end",
        )
        .unwrap_or_else(|e| panic!("esperava sucesso: {e}"));
        let TopLevel::TopLevelFunc { block, .. } = &program[0] else {
            panic!("esperava TopLevelFunc");
        };
        let Stat::StatBlock { stats, .. } = block else {
            panic!("esperava StatBlock");
        };
        let Stat::StatAssign { vars, .. } = &stats[0] else {
            panic!("esperava StatAssign");
        };
        assert!(matches!(vars[0], Var::VarBracket { .. }));
        assert!(matches!(vars[1], Var::VarDot { .. }));
    }

    /// Uma chamada no meio da lista de alvos é erro claro — e a mensagem é
    /// a mesma que o single-target sempre deu.
    #[test]
    fn chamada_como_alvo_da_lista_produz_erro() {
        let err = parse_source(
            "function main(args: {string}): integer\n\
             \x20   a, f() = 1, 2\n\
             \x20   return 0\n\
             end",
        )
        .unwrap_err();
        assert!(err.message.contains("chamada de função"), "{}", err.message);
    }

    /// Uma chamada seguida de vírgula, sem `=`, continua sendo erro — a
    /// vírgula abre a lista de alvos, não uma lista de chamadas.
    #[test]
    fn chamadas_separadas_por_virgula_produzem_erro() {
        let err = parse_source(
            "function main(args: {string}): integer\n\
             \x20   f(), g()\n\
             \x20   return 0\n\
             end",
        )
        .unwrap_err();
        assert!(err.message.contains("chamada de função"), "{}", err.message);
    }

    // ---- T68: o `?` sufixo de tipo e o `?` do nome ----------------------

    #[test]
    fn parse_type_aceita_sufixo_de_interrogacao() {
        let ty = parse_type_source("integer?").expect("`integer?` deveria parsear");
        let Type::TypeOption { basetype, .. } = ty else {
            panic!("esperava TypeOption, obteve {ty:?}");
        };
        assert!(matches!(*basetype, Type::TypeInteger { .. }));
    }

    /// O `?` é sufixo de **todo** o tipo à esquerda: `{integer}?` é um array
    /// opcional, e `{integer?}` é um array de inteiros opcionais.
    #[test]
    fn parse_type_distingue_array_opcional_de_array_de_opcionais() {
        let ty = parse_type_source("{integer}?").expect("`{integer}?` deveria parsear");
        let Type::TypeOption { basetype, .. } = ty else {
            panic!("esperava TypeOption, obteve {ty:?}");
        };
        assert!(matches!(*basetype, Type::TypeArray { .. }));

        let ty = parse_type_source("{integer?}").expect("`{integer?}` deveria parsear");
        let Type::TypeArray { subtype, .. } = ty else {
            panic!("esperava TypeArray, obteve {ty:?}");
        };
        assert!(matches!(*subtype, Type::TypeOption { .. }));
    }

    #[test]
    fn parse_type_aceita_interrogacao_em_nome_de_record() {
        let ty = parse_type_source("Ponto?").expect("`Ponto?` deveria parsear");
        let Type::TypeOption { basetype, .. } = ty else {
            panic!("esperava TypeOption, obteve {ty:?}");
        };
        assert!(matches!(*basetype, Type::TypeName { ref name, .. } if name == "Ponto"));
    }

    #[test]
    fn parse_type_recusa_interrogacao_dupla() {
        let err = parse_type_source("integer??").unwrap_err();
        assert!(err.message.contains("`?` duplicado"), "{}", err.message);
    }

    #[test]
    fn parse_aceita_interrogacao_depois_do_nome_no_local() {
        let program = parse_source(
            "function main(args: {string}): integer\n\
             \x20   local x? = 10\n\
             \x20   return 0\n\
             end",
        )
        .expect("`local x? = 10` deveria parsear");
        let TopLevel::TopLevelFunc { block, .. } = &program[0] else {
            panic!("esperava TopLevelFunc");
        };
        let Stat::StatBlock { stats, .. } = block else {
            panic!("esperava StatBlock");
        };
        let Stat::StatDecl { decls, .. } = &stats[0] else {
            panic!("esperava StatDecl");
        };
        assert!(decls[0].option);
        assert!(decls[0].r#type.is_none());
    }

    /// `?` no nome **e** anotação de tipo na mesma declaração é ambíguo:
    /// erro claro apontando as duas grafias canônicas.
    #[test]
    fn parse_recusa_interrogacao_no_nome_junto_com_anotacao() {
        let err = parse_source(
            "function main(args: {string}): integer\n\
             \x20   local x?: integer = 10\n\
             \x20   return 0\n\
             end",
        )
        .unwrap_err();
        assert!(err.message.contains("local nome: T?"), "{}", err.message);
    }

    /// O `?` de tipo vale também na assinatura — parâmetro e retorno.
    #[test]
    fn parse_aceita_interrogacao_em_parametro_e_retorno() {
        let program = parse_source(
            "function f(x: integer?): string?\n\
             \x20   return nil\n\
             end",
        )
        .expect("assinatura com `?` deveria parsear");
        let TopLevel::TopLevelFunc {
            params, rettypes, ..
        } = &program[0]
        else {
            panic!("esperava TopLevelFunc");
        };
        assert!(matches!(params[0].r#type, Some(Type::TypeOption { .. })));
        assert!(matches!(rettypes[0], Type::TypeOption { .. }));
    }

    // ---- T70: cast `as` --------------------------------------------------

    #[test]
    fn parse_cast_simples() {
        let exp = parse_exp_source("1 as float").unwrap_or_else(|e| panic!("esperava sucesso: {e}"));
        let Exp::ExpCast { exp, target, .. } = exp else {
            panic!("esperava ExpCast, obteve {exp:?}");
        };
        assert!(matches!(*exp, Exp::ExpInteger { value: 1, .. }));
        assert!(matches!(target, Type::TypeFloat { .. }));
    }

    /// O cast é mais forte que qualquer binário: `a + b as float` converte
    /// só o `b`.
    #[test]
    fn parse_cast_morde_antes_do_binario() {
        let exp =
            parse_exp_source("a + b as float").unwrap_or_else(|e| panic!("esperava sucesso: {e}"));
        let Exp::ExpBinop { op, rhs, .. } = exp else {
            panic!("esperava ExpBinop, obteve {exp:?}");
        };
        assert_eq!(op, "+");
        assert!(matches!(*rhs, Exp::ExpCast { .. }));
    }

    /// E mais forte que o unário: `-x as float` é `-(x as float)`.
    #[test]
    fn parse_cast_fica_abaixo_do_unario() {
        let exp =
            parse_exp_source("-x as float").unwrap_or_else(|e| panic!("esperava sucesso: {e}"));
        let Exp::ExpUnop { op, exp, .. } = exp else {
            panic!("esperava ExpUnop, obteve {exp:?}");
        };
        assert_eq!(op, "-");
        assert!(matches!(*exp, Exp::ExpCast { .. }));
    }

    /// Mas mais fraco que os sufixos: `f() as integer` converte o retorno da
    /// chamada, não chama o resultado do cast.
    #[test]
    fn parse_cast_recebe_a_chamada_ja_sufixada() {
        let exp =
            parse_exp_source("f() as integer").unwrap_or_else(|e| panic!("esperava sucesso: {e}"));
        let Exp::ExpCast { exp, .. } = exp else {
            panic!("esperava ExpCast, obteve {exp:?}");
        };
        assert!(matches!(*exp, Exp::ExpCall { .. }));
    }

    /// `x as float as value` encadeia sem parênteses, associando à esquerda.
    #[test]
    fn parse_cast_encadeia_associando_a_esquerda() {
        let exp = parse_exp_source("x as float as value")
            .unwrap_or_else(|e| panic!("esperava sucesso: {e}"));
        let Exp::ExpCast { exp, target, .. } = exp else {
            panic!("esperava ExpCast, obteve {exp:?}");
        };
        assert!(matches!(target, Type::TypeValue { .. }));
        let Exp::ExpCast { target: dentro, .. } = *exp else {
            panic!("esperava ExpCast aninhado");
        };
        assert!(matches!(dentro, Type::TypeFloat { .. }));
    }

    /// O alvo é um tipo completo, não só uma primitiva: `as {integer}` e
    /// `as integer?` parseiam pelo mesmo `parse_type` de sempre.
    #[test]
    fn parse_cast_aceita_tipo_composto_como_alvo() {
        let exp =
            parse_exp_source("x as {integer}").unwrap_or_else(|e| panic!("esperava sucesso: {e}"));
        let Exp::ExpCast { target, .. } = exp else {
            panic!("esperava ExpCast, obteve {exp:?}");
        };
        assert!(matches!(target, Type::TypeArray { .. }));
    }

    #[test]
    fn parse_cast_sem_tipo_erra() {
        let err = parse_exp_source("x as").unwrap_err();
        assert!(!err.message.is_empty());
    }

    // ------------------------------------------------------------------
    // T75: `enum` e `match`.
    // ------------------------------------------------------------------

    fn enum_da_fonte(source: &str) -> (String, Vec<crate::ast::Variant>) {
        let program = parse_source(source).unwrap_or_else(|e| panic!("esperava sucesso: {e}"));
        let TopLevel::TopLevelEnum { name, variants, .. } = &program[0] else {
            panic!("esperava TopLevelEnum, obteve {:?}", program[0]);
        };
        (name.clone(), variants.clone())
    }

    /// A declaração central da fase: variante sem payload, com um campo e
    /// com três — a última **recursiva**, que é o motivo de ser do `enum`.
    #[test]
    fn enum_com_e_sem_payload_produz_variantes_posicionais() {
        let (name, variants) = enum_da_fonte(
            "enum Exp\n\
             \x20   ExpNil\n\
             \x20   ExpInteger(integer)\n\
             \x20   ExpBinop(string, Exp, Exp)\n\
             end",
        );

        assert_eq!(name, "Exp");
        assert_eq!(variants.len(), 3);

        assert_eq!(variants[0].name, "ExpNil");
        assert!(
            variants[0].fields.is_empty(),
            "variante sem parênteses não tem campo"
        );

        assert_eq!(variants[1].name, "ExpInteger");
        assert!(matches!(
            variants[1].fields.as_slice(),
            [Type::TypeInteger { .. }]
        ));

        assert_eq!(variants[2].name, "ExpBinop");
        assert_eq!(variants[2].fields.len(), 3);
        assert!(matches!(variants[2].fields[0], Type::TypeString { .. }));
        // A recursão é só um `TypeName` para o parser: quem a permite de
        // propósito é o checker (decisão técnica 6 do PRD.md).
        assert!(
            matches!(&variants[2].fields[1], Type::TypeName { name, .. } if name == "Exp"),
            "campo recursivo deveria ser TypeName(\"Exp\"), obteve {:?}",
            variants[2].fields[1]
        );
    }

    /// Campos de variante são posicionais — só tipos compostos também valem,
    /// pelo mesmo `parse_type` de sempre.
    #[test]
    fn variante_aceita_tipo_composto_no_campo() {
        let (_, variants) = enum_da_fonte("enum Lista\n    Cheia({integer})\nend");
        assert!(matches!(
            variants[0].fields.as_slice(),
            [Type::TypeArray { .. }]
        ));
    }

    #[test]
    fn enum_sem_variante_produz_erro_claro() {
        let err = parse_source("enum Vazio\nend").unwrap_err();
        assert!(
            err.message.contains("pelo menos uma variante"),
            "mensagem inesperada: {}",
            err.message
        );
    }

    /// Parênteses vazios não são sinônimo de variante sem payload: a
    /// mensagem diz o que fazer, não só que está errado.
    #[test]
    fn variante_com_parenteses_vazios_produz_erro_claro() {
        let err = parse_source("enum Exp\n    ExpNil()\nend").unwrap_err();
        assert!(
            err.message.contains("sem parênteses"),
            "mensagem inesperada: {}",
            err.message
        );
    }

    #[test]
    fn enum_sem_end_produz_erro_sem_panic() {
        let err = parse_source("enum Exp\n    ExpNil\n").unwrap_err();
        assert!(
            err.message.contains("'end'"),
            "mensagem inesperada: {}",
            err.message
        );
    }

    /// A decisão de design mais consequente da T75: `ExpInteger(42)` colide
    /// sintaticamente com uma chamada de função, e o parser **não** tenta
    /// desambiguar — produz `ExpCall` e deixa a decisão para o checker
    /// (T76), como `{...}` já é desambiguado por contexto em
    /// `check_init_list`. É o que evita backtracking.
    #[test]
    fn construcao_de_variante_produz_expcall_como_qualquer_chamada() {
        let exp =
            parse_exp_source("ExpInteger(42)").unwrap_or_else(|e| panic!("esperava sucesso: {e}"));

        let Exp::ExpCall {
            exp: callee, args, ..
        } = &exp
        else {
            panic!("esperava ExpCall, obteve {exp:?}");
        };
        assert!(
            matches!(&**callee, Exp::ExpVar { var, .. }
                if matches!(&**var, Var::VarName { name, .. } if name == "ExpInteger")),
            "chamado inesperado: {callee:?}"
        );
        let Args::ArgsFunc { args, .. } = args else {
            panic!("esperava ArgsFunc");
        };
        assert!(matches!(
            args.as_slice(),
            [Exp::ExpInteger { value: 42, .. }]
        ));
    }

    fn match_stat_da_fonte(source: &str) -> (Exp, Vec<MatchArm<Stat>>) {
        let stats = stats_da_primeira_funcao(source);
        let Stat::StatMatch { exp, arms, .. } = &stats[0] else {
            panic!("esperava StatMatch, obteve {:?}", stats[0]);
        };
        ((**exp).clone(), arms.clone())
    }

    /// `match` como comando, com os campos ligados a nomes locais e sem
    /// braço curinga.
    #[test]
    fn match_statement_liga_campos_e_dispensa_curinga() {
        let (escrutinado, arms) = match_stat_da_fonte(
            "function f(e: Exp): integer\n\
             \x20   match e with\n\
             \x20       ExpNil then\n\
             \x20           imprima(0)\n\
             \x20       ExpBinop(op, l, r) then\n\
             \x20           imprima(1)\n\
             \x20   end\n\
             \x20   return 0\n\
             end",
        );

        assert!(
            matches!(&escrutinado, Exp::ExpVar { var, .. }
                if matches!(&**var, Var::VarName { name, .. } if name == "e")),
            "escrutinado inesperado: {escrutinado:?}"
        );
        assert_eq!(arms.len(), 2);

        let Pattern::Variant { name, fields, .. } = &arms[0].pattern else {
            panic!("esperava Pattern::Variant, obteve {:?}", arms[0].pattern);
        };
        assert_eq!(name, "ExpNil");
        assert!(fields.is_empty());

        let Pattern::Variant { name, fields, .. } = &arms[1].pattern else {
            panic!("esperava Pattern::Variant, obteve {:?}", arms[1].pattern);
        };
        assert_eq!(name, "ExpBinop");
        let ligados: Vec<&str> = fields.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(ligados, vec!["op", "l", "r"]);
        // O tipo do campo vem da declaração do `enum`, nunca do padrão.
        assert!(fields.iter().all(|d| d.r#type.is_none()));

        // O corpo de cada braço é um bloco, e a fronteira entre eles caiu
        // no lugar certo: um comando em cada.
        for arm in &arms {
            let Stat::StatBlock { stats, .. } = &arm.body else {
                panic!("esperava StatBlock no corpo do braço");
            };
            assert_eq!(stats.len(), 1);
        }
    }

    /// O braço curinga `_` chega ao lexer como `Name("_")` e só o parser de
    /// padrão lhe dá o sentido de curinga.
    #[test]
    fn match_com_curinga_produz_pattern_wildcard() {
        let (_, arms) = match_stat_da_fonte(
            "function f(e: Exp): integer\n\
             \x20   match e with\n\
             \x20       ExpNil then\n\
             \x20           imprima(0)\n\
             \x20       _ then\n\
             \x20           imprima(1)\n\
             \x20   end\n\
             \x20   return 0\n\
             end",
        );

        assert_eq!(arms.len(), 2);
        assert!(matches!(arms[1].pattern, Pattern::Wildcard { .. }));
    }

    /// A armadilha do corpo de braço: um padrão começa com `Name`, como uma
    /// chamada de função. `g(x)` na última linha do braço é comando, e
    /// `ExpBinop(op, l, r) then` é braço novo — o lookahead de
    /// `inicia_braco_de_match` é o que separa os dois.
    #[test]
    fn chamada_no_fim_do_braco_nao_vira_braco_novo() {
        let (_, arms) = match_stat_da_fonte(
            "function f(e: Exp): integer\n\
             \x20   match e with\n\
             \x20       ExpNil then\n\
             \x20           g(x)\n\
             \x20           h(y)\n\
             \x20       ExpBinop(op, l, r) then\n\
             \x20           g(x)\n\
             \x20   end\n\
             \x20   return 0\n\
             end",
        );

        assert_eq!(arms.len(), 2, "braços: {arms:?}");
        let Stat::StatBlock { stats, .. } = &arms[0].body else {
            panic!("esperava StatBlock");
        };
        assert_eq!(stats.len(), 2, "as duas chamadas ficam no primeiro braço");
        assert!(stats.iter().all(|s| matches!(s, Stat::StatCall { .. })));
    }

    /// A vizinhança da armadilha anterior: uma **atribuição** no fim do
    /// braço também começa com `Name`, e o token seguinte (`=`, `[`, `.`)
    /// é o que a separa de um padrão — sem precisar da varredura.
    #[test]
    fn atribuicao_no_fim_do_braco_nao_vira_braco_novo() {
        let (_, arms) = match_stat_da_fonte(
            "function f(e: Exp, v: {integer}): integer\n\
             \x20   match e with\n\
             \x20       ExpNil then\n\
             \x20           v[1] = 0\n\
             \x20       ExpInteger(n) then\n\
             \x20           imprima(1)\n\
             \x20   end\n\
             \x20   return 0\n\
             end",
        );

        assert_eq!(arms.len(), 2, "braços: {arms:?}");
        let Stat::StatBlock { stats, .. } = &arms[0].body else {
            panic!("esperava StatBlock");
        };
        assert!(
            matches!(stats.as_slice(), [Stat::StatAssign { .. }]),
            "a atribuição fica no primeiro braço: {stats:?}"
        );
    }

    /// `match` como expressão: o corpo de cada braço é uma expressão, e a
    /// fronteira entre braços não precisa de lookahead nenhum.
    #[test]
    fn match_como_expressao_produz_expmatch_com_corpo_de_expressao() {
        let exp = parse_exp_source(
            "match e with\n\
             \x20   ExpInteger(n) then n\n\
             \x20   _ then 0\n\
             end",
        )
        .unwrap_or_else(|e| panic!("esperava sucesso: {e}"));

        let Exp::ExpMatch { arms, .. } = &exp else {
            panic!("esperava ExpMatch, obteve {exp:?}");
        };
        assert_eq!(arms.len(), 2);

        let Pattern::Variant { name, fields, .. } = &arms[0].pattern else {
            panic!("esperava Pattern::Variant");
        };
        assert_eq!(name, "ExpInteger");
        assert_eq!(fields.len(), 1);
        assert!(
            matches!(&arms[0].body, Exp::ExpVar { var, .. }
                if matches!(&**var, Var::VarName { name, .. } if name == "n")),
            "corpo inesperado: {:?}",
            arms[0].body
        );

        assert!(matches!(arms[1].pattern, Pattern::Wildcard { .. }));
        assert!(matches!(arms[1].body, Exp::ExpInteger { value: 0, .. }));
    }

    #[test]
    fn match_sem_with_produz_erro_claro() {
        let err = parse_source(
            "function f(e: Exp): integer\n\
             \x20   match e\n\
             \x20       ExpNil then\n\
             \x20           imprima(0)\n\
             \x20   end\n\
             \x20   return 0\n\
             end",
        )
        .unwrap_err();
        assert!(
            err.message.contains("'with'"),
            "mensagem inesperada: {}",
            err.message
        );
    }

    #[test]
    fn braco_sem_then_produz_erro_claro() {
        let err = parse_source(
            "function f(e: Exp): integer\n\
             \x20   match e with\n\
             \x20       ExpNil\n\
             \x20           imprima(0)\n\
             \x20   end\n\
             \x20   return 0\n\
             end",
        )
        .unwrap_err();
        assert!(
            err.message.contains("'then'"),
            "mensagem inesperada: {}",
            err.message
        );
    }

    #[test]
    fn match_sem_braco_produz_erro_claro() {
        let err = parse_source(
            "function f(e: Exp): integer\n    match e with\n    end\n    return 0\nend",
        )
        .unwrap_err();
        assert!(
            err.message.contains("pelo menos um braço"),
            "mensagem inesperada: {}",
            err.message
        );
    }

    /// O tipo de um campo ligado vem do `enum`; escrevê-lo no padrão seria
    /// repetição que poderia divergir — e a mensagem diz a razão.
    #[test]
    fn campo_de_padrao_com_anotacao_de_tipo_produz_erro_claro() {
        let err = parse_source(
            "function f(e: Exp): integer\n\
             \x20   match e with\n\
             \x20       ExpInteger(n: integer) then\n\
             \x20           imprima(0)\n\
             \x20   end\n\
             \x20   return 0\n\
             end",
        )
        .unwrap_err();
        assert!(
            err.message.contains("não leva anotação de tipo"),
            "mensagem inesperada: {}",
            err.message
        );
    }

    /// `match` aninhado dentro de um braço: o `end` interno fecha o `match`
    /// de dentro, não o de fora.
    #[test]
    fn match_aninhado_em_braco_fecha_no_end_certo() {
        let (_, arms) = match_stat_da_fonte(
            "function f(e: Exp, o: Exp): integer\n\
             \x20   match e with\n\
             \x20       ExpBinop(op, l, r) then\n\
             \x20           match o with\n\
             \x20               ExpNil then\n\
             \x20                   imprima(0)\n\
             \x20           end\n\
             \x20       _ then\n\
             \x20           imprima(1)\n\
             \x20   end\n\
             \x20   return 0\n\
             end",
        );

        assert_eq!(arms.len(), 2);
        let Stat::StatBlock { stats, .. } = &arms[0].body else {
            panic!("esperava StatBlock");
        };
        assert!(
            matches!(stats.as_slice(), [Stat::StatMatch { .. }]),
            "o braço externo contém só o `match` interno: {stats:?}"
        );
    }
}
