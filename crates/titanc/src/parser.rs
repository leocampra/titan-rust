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
//! Statements: `StatCall`, `StatReturn`, `StatDecl`, `StatIf`, `StatWhile`,
//! `StatFor` (numérico), `StatAssign` (single-target).
//! Expressões: literais, `ExpVar`, `ExpCall`, `ExpConcat` (`..`) e
//! `ExpBinop`/`ExpUnop` numa cascata de precedência que espelha
//! `parser.lua:369-395` (níveis bitwise fora de escopo).
//! Tipos: `integer`, `float`, `boolean`, `string`, `nil`, `{T}`.
//!
//! Tudo fora desse subconjunto (records, maps, arrays manipuláveis,
//! `import`, retornos múltiplos, `repeat`/`until`, ...) produz um erro
//! sintático claro — nunca panic.

use crate::ast::{
    Args, Decl, Exp, Field, FieldName, Loc, Program, Stat, Then, TopLevel, Type, Var,
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

        let islocal = self.eat(&TokenKind::Local);

        if self.eat(&TokenKind::Function) {
            return self.parse_toplevel_func(loc, islocal);
        }

        if islocal {
            return self.parse_toplevel_var(loc);
        }

        Err(self.erro(
            "Esperava uma declaração de topo (`function`, `local` ou `record`) em vez disso.",
        ))
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

    /// `nome [: Tipo]` — usado em `local` e no `for`, onde a anotação é
    /// opcional. `mensagem_nome` é o erro caso o nome não esteja presente.
    fn parse_decl_opt_type(&mut self, mensagem_nome: &str) -> Result<Decl, ParseError> {
        let (name, loc) = self.expect_name(mensagem_nome)?;
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
            TokenKind::KwValue => {
                self.advance();
                Ok(Type::TypeValue { loc })
            }
            TokenKind::Name(_) => {
                let (name, _) = self.expect_name("Esperava um nome de tipo.")?;
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
                 um nome de record, `{T}` ou `{K: V}`).",
            )),
        }
    }

    // ---- Statements --------------------------------------------------

    fn parse_block(&mut self) -> Result<Stat, ParseError> {
        let loc = self.loc();
        let mut stats = Vec::new();
        // `elseif`/`else` também terminam um bloco — quem os consome (ou
        // rejeita, no caso de um bloco de função) é o chamador.
        while !self.check(&TokenKind::End)
            && !self.check(&TokenKind::Elseif)
            && !self.check(&TokenKind::Else)
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

        if self.eat(&TokenKind::For) {
            return self.parse_stat_for(loc);
        }

        // Chamada ou atribuição — desambiguadas sem backtracking, como no
        // original (`suffixedexp` + checar `ASSIGN`, `parser.lua:354-358`):
        // parseia a expressão sufixada e o token seguinte decide.
        let exp = self.parse_suffixed_exp()?;
        if self.check(&TokenKind::Assign) {
            return self.parse_stat_assign(loc, exp);
        }
        if !matches!(exp, Exp::ExpCall { .. }) {
            return Err(ParseError {
                message: "Esperava um comando (`local`, `return`, `if`, `while`, `for`, \
                          uma atribuição ou uma chamada de função)."
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

    /// `for nome [: T] = exp, exp [, exp] do block end` — só a forma numérica
    /// (sem for-in nesta fase).
    fn parse_stat_for(&mut self, loc: Loc) -> Result<Stat, ParseError> {
        let decl = self.parse_decl_opt_type("Esperava um nome de variável após 'for'.")?;
        self.expect(&TokenKind::Assign, "Esperava '=' após a variável do 'for'.")?;
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

    /// `nome = exp` — atribuição single-target. `target` é a expressão
    /// sufixada já parseada pelo chamador; o `=` ainda não foi consumido.
    fn parse_stat_assign(&mut self, loc: Loc, target: Exp) -> Result<Stat, ParseError> {
        let var = match target {
            Exp::ExpVar { var, .. } => *var,
            Exp::ExpCall { .. } => {
                return Err(self.erro("Não é possível atribuir a uma chamada de função."));
            }
            _ => return Err(self.erro("Esperava uma variável do lado esquerdo de '='.")),
        };
        self.expect(&TokenKind::Assign, "Esperava '=' na atribuição.")?;
        let value = self.parse_exp()?;
        self.eat(&TokenKind::Semicolon);
        Ok(Stat::StatAssign {
            loc,
            vars: vec![var],
            exps: vec![value],
        })
    }

    fn parse_stat_decl(&mut self, loc: Loc) -> Result<Stat, ParseError> {
        let decl = self.parse_decl_opt_type("Esperava um nome de variável após 'local'.")?;
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
        // Os terminadores de bloco (`end`/`elseif`/`else`) e o `;` indicam
        // `return` sem valor.
        if !self.check(&TokenKind::End)
            && !self.check(&TokenKind::Elseif)
            && !self.check(&TokenKind::Else)
            && !self.check(&TokenKind::Semicolon)
            && !self.check(&TokenKind::Eof)
        {
            exps.push(self.parse_exp()?);
        }
        self.eat(&TokenKind::Semicolon);
        Ok(Stat::StatReturn { loc, exps })
    }

    // ---- Expressões ----------------------------------------------------
    //
    // Cascata de níveis de precedência (do mais fraco ao mais forte),
    // espelhando `parser.lua:369-395` com os níveis bitwise omitidos:
    //
    // ```text
    // parse_exp → parse_or_exp
    // or_exp     : and_exp (or and_exp)*                       — assoc. esquerda
    // and_exp    : rel_exp (and rel_exp)*                      — assoc. esquerda
    // rel_exp    : concat_exp ((== ~= < > <= >=) concat_exp)?  — sem encadear
    // concat_exp : add_exp (.. add_exp)*                       — assoc. direita
    // add_exp    : mul_exp ((+ -) mul_exp)*                    — assoc. esquerda
    // mul_exp    : unary_exp ((* / %) unary_exp)*              — assoc. esquerda
    // unary_exp  : (not | -)* pow_exp
    // pow_exp    : simple_exp (^ unary_exp)?                   — assoc. direita
    // ```

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
        let lhs = self.parse_concat_exp()?;
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
        let rhs = self.parse_concat_exp()?;
        Ok(Exp::ExpBinop {
            loc,
            lhs: Box::new(lhs),
            op: op.to_string(),
            rhs: Box::new(rhs),
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
            TokenKind::Percent => Some("%"),
            _ => None,
        })
    }

    /// `(not | -)* pow_exp` — a repetição vira recursão: `- -1` e
    /// `not not true` produzem `ExpUnop` aninhados.
    fn parse_unary_exp(&mut self) -> Result<Exp, ParseError> {
        let loc = self.loc();
        let op = match &self.peek().kind {
            TokenKind::Not => "not",
            TokenKind::Minus => "-",
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
        let base = self.parse_simple_exp()?;
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
    /// (`VarDot`). `VarBracket`/`VarDot` são embrulhados em `ExpVar` para
    /// poderem seguir sendo sufixados (`a[1].campo[2]`).
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
                let loc = self.loc();
                self.advance();
                let (name, _) = self.expect_name("Esperava um nome de campo após '.'.")?;
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
}
