//! Análise semântica e verificação de tipos do Titan.
//!
//! Espelha a estratégia de `titan/titan-compiler/checker.lua` (1662 linhas),
//! reduzida ao subconjunto das Fases 0 e 1 (T5 e T12 do PRD.md):
//!
//! - **Duas passadas**, como o Titan: (1) coleta as assinaturas top-level,
//!   permitindo chamada antes da declaração; (2) verifica os corpos,
//!   produzindo uma AST anotada com o tipo resolvido de cada `Exp`.
//! - Símbolos com escopo em pilha, espelhando `titan/titan-compiler/symtab.lua`.
//! - `print` é registrado no escopo global como
//!   `Function{params:[String], rettypes:[Nil]}`, originado do runtime — não é
//!   palavra-chave.
//! - A assinatura de `main` é validada: `main(args: {string}): integer`
//!   (`checker.lua:1593-1607`, `checker.has_main`).
//! - Statements da Fase 1: `if`/`while` com condição `boolean`
//!   (`checker.lua:365-368` e `447-457`), `for` numérico espelhando
//!   `checkfor` (`checker.lua:239-288`) e atribuição single-target
//!   (`checker.lua:378-410`).
//! - **Rastreio de mutabilidade** (decisão 6 da Fase 1): cada `local` recebe
//!   um id; atribuições registram o id do símbolo resolvido (mesmo espírito
//!   do `var._decl._assigned = true` do original) e um fix-up ao final do
//!   corpo da função seta `mutable` nos `TypedStat::Decl` correspondentes —
//!   o codegen (T14) emite `let mut` só quando há reatribuição.
//!
//! Como `ast::Exp` é um valor imutável (ao contrário do Lua, que anexa
//! `_type` dinamicamente ao nó), o checker produz uma **AST tipada paralela**
//! (`TypedProgram` e companhia) em vez de mutar a árvore original — é o que
//! `codegen.rs` (T6) vai consumir.
//!
//! Tudo fora do subconjunto (records, maps, arrays manipuláveis, `import`,
//! `foreign import`, métodos, retornos múltiplos, `repeat`, `Option`/`?`)
//! produz um erro semântico claro — nunca panic.

use std::collections::{HashMap, HashSet};

use crate::ast::{self, Args, Exp, Loc, Program, Stat, TopLevel, Var};
use crate::types::Type;

/// Erro semântico com posição (no espírito de `checker.typeerror`).
#[derive(Debug, Clone, PartialEq)]
pub struct CheckError {
    pub message: String,
    pub loc: Loc,
}

impl std::fmt::Display for CheckError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "erro de tipo (linha {}, coluna {}): {}",
            self.loc.line, self.loc.col, self.message
        )
    }
}

impl std::error::Error for CheckError {}

// ---- Símbolos em pilha (`symtab.lua`) ----------------------------------

/// Identificador único de uma declaração `local`, usado pelo rastreio de
/// mutabilidade (decisão 6 da Fase 1).
type DeclId = usize;

/// Como um nome foi introduzido no escopo — decide o que uma atribuição a
/// ele significa.
#[derive(Debug, Clone, Copy, PartialEq)]
enum SymbolKind {
    /// Função top-level ou do runtime (`print`). Atribuição é rejeitada
    /// ("trying to assign to a function", `checker.lua:401`).
    Global,
    /// Parâmetro de função. O original permite atribuir, mas aqui não há
    /// rastreio de `mut` para parâmetros (o fix-up só alcança
    /// `TypedStat::Decl`), e o Rust gerado não compilaria — rejeitado com
    /// erro claro até uma fase futura rastrear parâmetros também.
    Param,
    /// Variável de controle de `for`. Atribuição é permitida sem rastreio:
    /// ela é sempre `mut` no template desaçucarado do T15.
    ForVar,
    /// Local declarada com `local`; atribuições registram o `DeclId` para o
    /// fix-up de mutabilidade ao final do corpo da função.
    Local { decl_id: DeclId },
}

/// Entrada da tabela de símbolos: o tipo e a origem do nome.
#[derive(Debug, Clone, PartialEq)]
struct Symbol {
    ty: Type,
    kind: SymbolKind,
}

/// Pilha de escopos léxicos. Cada bloco é um `HashMap` de nome → símbolo.
struct SymTab {
    blocks: Vec<HashMap<String, Symbol>>,
}

impl SymTab {
    fn new() -> Self {
        SymTab {
            blocks: vec![HashMap::new()],
        }
    }

    fn open_block(&mut self) {
        self.blocks.push(HashMap::new());
    }

    fn close_block(&mut self) {
        self.blocks.pop();
    }

    fn add_symbol(&mut self, name: &str, ty: Type, kind: SymbolKind) {
        self.blocks
            .last_mut()
            .expect("symtab sempre tem pelo menos um bloco")
            .insert(name.to_string(), Symbol { ty, kind });
    }

    fn find_symbol(&self, name: &str) -> Option<&Symbol> {
        self.blocks.iter().rev().find_map(|block| block.get(name))
    }
}

// ---- AST tipada ---------------------------------------------------------

/// Programa já verificado: cada função top-level com seu tipo resolvido.
pub type TypedProgram = Vec<TypedTopLevel>;

#[derive(Debug, Clone, PartialEq)]
pub enum TypedTopLevel {
    Func {
        loc: Loc,
        islocal: bool,
        name: String,
        params: Vec<(String, Type)>,
        rettypes: Vec<Type>,
        body: TypedStat,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum TypedStat {
    Block {
        loc: Loc,
        stats: Vec<TypedStat>,
    },
    Decl {
        loc: Loc,
        name: String,
        ty: Type,
        value: TypedExp,
        /// Id interno da declaração, usado só pelo fix-up de mutabilidade —
        /// torna a correspondência atribuição → declaração explícita (e
        /// robusta a shadowing) em vez de depender da ordem de travessia.
        decl_id: usize,
        /// `true` quando alguma atribuição alcança esta declaração; o
        /// codegen (T14) emite `let mut` somente nesse caso.
        mutable: bool,
    },
    Call {
        loc: Loc,
        call: TypedExp,
    },
    Return {
        loc: Loc,
        exps: Vec<TypedExp>,
    },
    If {
        loc: Loc,
        thens: Vec<TypedThen>,
        elsestat: Option<Box<TypedStat>>,
    },
    While {
        loc: Loc,
        condition: TypedExp,
        block: Box<TypedStat>,
    },
    For {
        loc: Loc,
        name: String,
        ty: Type,
        start: TypedExp,
        finish: TypedExp,
        /// Sempre presente: quando omitido no fonte, vira `1`/`1.0` conforme
        /// o tipo da variável (como `checkfor`, `checker.lua:258-268`).
        inc: TypedExp,
        block: Box<TypedStat>,
    },
    Assign {
        loc: Loc,
        name: String,
        value: TypedExp,
    },
}

/// Ramo `then` já verificado de um `TypedStat::If`.
#[derive(Debug, Clone, PartialEq)]
pub struct TypedThen {
    pub loc: Loc,
    pub condition: TypedExp,
    pub block: TypedStat,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypedExp {
    pub loc: Loc,
    pub ty: Type,
    pub kind: TypedExpKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TypedExpKind {
    Nil,
    Bool(bool),
    Integer(i64),
    Float(f64),
    String(String),
    Var(String),
    Call { callee: String, args: Vec<TypedExp> },
    Concat(Vec<TypedExp>),
}

// ---- Checker -------------------------------------------------------------

struct Checker {
    st: SymTab,
    errors: Vec<CheckError>,
    /// Próximo id de declaração `local` — os ids são globais e únicos, então
    /// não precisam de reset entre funções.
    next_decl_id: DeclId,
    /// Ids das declarações que receberam alguma atribuição (mesmo espírito
    /// do `var._decl._assigned = true` do original, `checker.lua:404`).
    assigned: HashSet<DeclId>,
}

impl Checker {
    fn new() -> Self {
        let mut st = SymTab::new();
        // `print` vem do runtime, registrado no escopo global — não é
        // palavra-chave (PRD.md, T5).
        st.add_symbol(
            "print",
            Type::Function {
                params: vec![Type::String],
                rettypes: vec![Type::Nil],
            },
            SymbolKind::Global,
        );
        Checker {
            st,
            errors: Vec::new(),
            next_decl_id: 0,
            assigned: HashSet::new(),
        }
    }

    fn error(&mut self, loc: Loc, message: impl Into<String>) {
        self.errors.push(CheckError {
            message: message.into(),
            loc,
        });
    }

    // ---- Passada 1: assinaturas top-level ------------------------------

    fn collect_signature(&mut self, node: &TopLevel) {
        match node {
            TopLevel::TopLevelFunc {
                loc,
                name,
                params,
                rettypes,
                ..
            } => {
                if self.st.find_symbol(name).is_some() {
                    self.error(*loc, format!("'{name}' já foi declarado antes."));
                    return;
                }
                let param_types = match self.resolve_param_types(params) {
                    Some(types) => types,
                    None => return,
                };
                let ret_types = match self.resolve_types(rettypes) {
                    Some(types) => types,
                    None => return,
                };
                self.st.add_symbol(
                    name,
                    Type::Function {
                        params: param_types,
                        rettypes: ret_types,
                    },
                    SymbolKind::Global,
                );
            }
            TopLevel::TopLevelVar { loc, .. } => {
                self.error(
                    *loc,
                    "declaração de variável no nível de topo não é suportada nesta fase.",
                );
            }
            TopLevel::TopLevelRecord { loc, .. } => {
                self.error(*loc, "`record` não é suportado nesta fase.");
            }
            TopLevel::TopLevelImport { loc, .. } => {
                self.error(*loc, "`import` não é suportado nesta fase.");
            }
            TopLevel::TopLevelForeignImport { loc, .. } => {
                self.error(*loc, "`foreign import` não é suportado nesta fase.");
            }
            TopLevel::TopLevelMethod { loc, .. } | TopLevel::TopLevelStatic { loc, .. } => {
                self.error(*loc, "métodos não são suportados nesta fase.");
            }
        }
    }

    fn resolve_param_types(&mut self, params: &[ast::Decl]) -> Option<Vec<Type>> {
        let mut result = Vec::with_capacity(params.len());
        for param in params {
            let Some(annotated) = &param.r#type else {
                self.error(
                    param.loc,
                    format!("parâmetro '{}' precisa de um tipo explícito.", param.name),
                );
                return None;
            };
            result.push(self.resolve_type(annotated)?);
        }
        Some(result)
    }

    fn resolve_types(&mut self, types: &[ast::Type]) -> Option<Vec<Type>> {
        let mut result = Vec::with_capacity(types.len());
        for t in types {
            result.push(self.resolve_type(t)?);
        }
        Some(result)
    }

    /// Converte uma anotação de tipo escrita no código-fonte (`ast::Type`) no
    /// tipo semântico correspondente (`types::Type`).
    fn resolve_type(&mut self, t: &ast::Type) -> Option<Type> {
        match t {
            ast::Type::TypeNil { .. } => Some(Type::Nil),
            ast::Type::TypeBoolean { .. } => Some(Type::Boolean),
            ast::Type::TypeInteger { .. } => Some(Type::Integer),
            ast::Type::TypeFloat { .. } => Some(Type::Float),
            ast::Type::TypeString { .. } => Some(Type::String),
            ast::Type::TypeValue { .. } => Some(Type::Value),
            ast::Type::TypeArray { subtype, .. } => {
                let elem = self.resolve_type(subtype)?;
                Some(Type::Array {
                    elem: Box::new(elem),
                })
            }
            ast::Type::TypeMap { loc, .. } => {
                self.error(*loc, "tipo `map` não é suportado nesta fase.");
                None
            }
            ast::Type::TypeFunction { loc, .. } => {
                self.error(
                    *loc,
                    "tipo de função como anotação não é suportado nesta fase.",
                );
                None
            }
            ast::Type::TypeOption { loc, .. } => {
                self.error(*loc, "tipo opcional (`?`) não é suportado nesta fase.");
                None
            }
            ast::Type::TypeName { loc, name } => {
                self.error(*loc, format!("tipo '{name}' desconhecido."));
                None
            }
            ast::Type::TypeQualName { loc, .. } => {
                self.error(
                    *loc,
                    "tipo qualificado por módulo não é suportado nesta fase.",
                );
                None
            }
        }
    }

    // ---- Validação de `main` -------------------------------------------

    fn check_has_main(&mut self, program: &Program) {
        let has_valid_main = program.iter().any(|node| match node {
            TopLevel::TopLevelFunc {
                name,
                params,
                rettypes,
                ..
            } if name == "main" => {
                params.len() == 1
                    && matches!(&params[0].r#type, Some(ast::Type::TypeArray { subtype, .. })
                        if matches!(**subtype, ast::Type::TypeString { .. }))
                    && rettypes.len() == 1
                    && matches!(rettypes[0], ast::Type::TypeInteger { .. })
            }
            _ => false,
        });

        if !has_valid_main {
            let loc = program
                .iter()
                .find_map(|node| match node {
                    TopLevel::TopLevelFunc { name, loc, .. } if name == "main" => Some(*loc),
                    _ => None,
                })
                .unwrap_or(Loc { line: 1, col: 1 });
            self.error(
                loc,
                "função 'main' precisa ter a assinatura main(args: {string}): integer.",
            );
        }
    }

    // ---- Passada 2: corpos ----------------------------------------------

    fn check_toplevel(&mut self, node: &TopLevel) -> Option<TypedTopLevel> {
        match node {
            TopLevel::TopLevelFunc {
                loc,
                islocal,
                name,
                params,
                block,
                ..
            } => {
                let Some(Symbol {
                    ty:
                        Type::Function {
                            params: param_types,
                            rettypes: ret_types,
                        },
                    ..
                }) = self.st.find_symbol(name).cloned()
                else {
                    // Assinatura já rejeitada na passada 1.
                    return None;
                };

                self.st.open_block();
                for (param, ty) in params.iter().zip(param_types.iter()) {
                    self.st
                        .add_symbol(&param.name, ty.clone(), SymbolKind::Param);
                }

                let body = self.check_stat(block, &ret_types);

                self.st.close_block();

                let mut body = body?;
                // Fix-up de mutabilidade (decisão 6): agora que todas as
                // atribuições do corpo foram vistas, marca as declarações
                // reatribuídas.
                fixup_mutability(&mut body, &self.assigned);
                let named_params = params
                    .iter()
                    .zip(param_types)
                    .map(|(p, t)| (p.name.clone(), t))
                    .collect();

                Some(TypedTopLevel::Func {
                    loc: *loc,
                    islocal: *islocal,
                    name: name.clone(),
                    params: named_params,
                    rettypes: ret_types,
                    body,
                })
            }
            // Já reportado como erro na passada 1.
            _ => None,
        }
    }

    fn check_stat(&mut self, stat: &Stat, rettypes: &[Type]) -> Option<TypedStat> {
        match stat {
            Stat::StatBlock { loc, stats } => {
                self.st.open_block();
                let mut typed_stats = Vec::with_capacity(stats.len());
                let mut ok = true;
                for s in stats {
                    match self.check_stat(s, rettypes) {
                        Some(typed) => typed_stats.push(typed),
                        None => ok = false,
                    }
                }
                self.st.close_block();
                if ok {
                    Some(TypedStat::Block {
                        loc: *loc,
                        stats: typed_stats,
                    })
                } else {
                    None
                }
            }
            Stat::StatDecl { loc, decls, exps } => {
                if decls.len() != 1 || exps.len() != 1 {
                    self.error(
                        *loc,
                        "declaração múltipla (`local a, b = ...`) não é suportada nesta fase.",
                    );
                    return None;
                }
                let decl = &decls[0];
                let value = self.check_exp(&exps[0])?;

                let ty = match &decl.r#type {
                    Some(annotated) => {
                        let declared = self.resolve_type(annotated)?;
                        if !declared.compatible(&value.ty) {
                            self.error(
                                decl.loc,
                                format!(
                                    "tipos incompatíveis na declaração de '{}': esperado {}, encontrado {}.",
                                    decl.name,
                                    type_name(&declared),
                                    type_name(&value.ty)
                                ),
                            );
                            return None;
                        }
                        declared
                    }
                    None => value.ty.clone(),
                };

                let decl_id = self.next_decl_id;
                self.next_decl_id += 1;
                self.st
                    .add_symbol(&decl.name, ty.clone(), SymbolKind::Local { decl_id });

                Some(TypedStat::Decl {
                    loc: *loc,
                    name: decl.name.clone(),
                    ty,
                    value,
                    decl_id,
                    mutable: false,
                })
            }
            Stat::StatCall { loc, callexp } => {
                let call = self.check_exp(callexp)?;
                Some(TypedStat::Call { loc: *loc, call })
            }
            Stat::StatReturn { loc, exps } => {
                let mut typed_exps = Vec::with_capacity(exps.len());
                let mut ok = true;
                for e in exps {
                    match self.check_exp(e) {
                        Some(typed) => typed_exps.push(typed),
                        None => ok = false,
                    }
                }
                if !ok {
                    return None;
                }

                if typed_exps.len() != rettypes.len() {
                    self.error(
                        *loc,
                        format!(
                            "retornou {} valor(es), mas a função espera {}.",
                            typed_exps.len(),
                            rettypes.len()
                        ),
                    );
                    return None;
                }

                for (found, expected) in typed_exps.iter().zip(rettypes) {
                    if !expected.compatible(&found.ty) {
                        self.error(
                            found.loc,
                            format!(
                                "retorno incompatível: esperado {}, encontrado {}.",
                                type_name(expected),
                                type_name(&found.ty)
                            ),
                        );
                        return None;
                    }
                }

                Some(TypedStat::Return {
                    loc: *loc,
                    exps: typed_exps,
                })
            }
            Stat::StatIf {
                loc,
                thens,
                elsestat,
            } => {
                // Defensivo: o parser (T11) sempre produz ao menos um ramo.
                if thens.is_empty() {
                    self.error(*loc, "um `if` precisa de ao menos uma condição.");
                    return None;
                }
                let mut typed_thens = Vec::with_capacity(thens.len());
                let mut ok = true;
                for then in thens {
                    let condition = self.check_condition(&then.condition, "if");
                    let block = self.check_stat(&then.block, rettypes);
                    match (condition, block) {
                        (Some(condition), Some(block)) => typed_thens.push(TypedThen {
                            loc: then.loc,
                            condition,
                            block,
                        }),
                        _ => ok = false,
                    }
                }
                let typed_else = match elsestat {
                    Some(stat) => match self.check_stat(stat, rettypes) {
                        Some(typed) => Some(Box::new(typed)),
                        None => {
                            ok = false;
                            None
                        }
                    },
                    None => None,
                };
                if !ok {
                    return None;
                }
                Some(TypedStat::If {
                    loc: *loc,
                    thens: typed_thens,
                    elsestat: typed_else,
                })
            }
            Stat::StatWhile {
                loc,
                condition,
                block,
            } => {
                let condition = self.check_condition(condition, "while");
                let block = self.check_stat(block, rettypes);
                Some(TypedStat::While {
                    loc: *loc,
                    condition: condition?,
                    block: Box::new(block?),
                })
            }
            Stat::StatRepeat { loc, .. } => {
                self.error(*loc, "`repeat` não é suportado nesta fase.");
                None
            }
            Stat::StatFor {
                loc,
                decl,
                start,
                finish,
                inc,
                block,
            } => self.check_for(*loc, decl, start, finish, inc.as_deref(), block, rettypes),
            Stat::StatAssign { loc, vars, exps } => {
                if vars.len() != 1 || exps.len() != 1 {
                    // Defensivo: o parser (T11) só produz single-target.
                    self.error(
                        *loc,
                        "atribuição múltipla (`a, b = ...`) não é suportada nesta fase.",
                    );
                    return None;
                }
                self.check_assign(*loc, &vars[0], &exps[0])
            }
        }
    }

    /// Condição de `if`/`elseif`/`while`: precisa ser `Boolean` (ou `Value`,
    /// via `compatible` — gradual typing), como o `checkexp(cond, ...,
    /// types.Boolean())` do original.
    fn check_condition(&mut self, exp: &Exp, contexto: &str) -> Option<TypedExp> {
        let typed = self.check_exp(exp)?;
        if !Type::Boolean.compatible(&typed.ty) {
            self.error(
                typed.loc,
                format!(
                    "a condição do `{contexto}` precisa ser boolean, encontrado {}.",
                    type_name(&typed.ty)
                ),
            );
            return None;
        }
        Some(typed)
    }

    /// `for` numérico, espelhando `checkfor` (`checker.lua:239-288`):
    /// expressões verificadas **antes** de declarar a variável (elas não
    /// podem referenciá-la), tipo da variável vindo da anotação ou inferido
    /// de `start`, e — decisão 5 da Fase 1 — `start`/`finish`/`inc` com tipo
    /// **idêntico** ao da variável (sem coerção int→float).
    #[allow(clippy::too_many_arguments)]
    fn check_for(
        &mut self,
        loc: Loc,
        decl: &ast::Decl,
        start: &Exp,
        finish: &Exp,
        inc: Option<&Exp>,
        block: &Stat,
        rettypes: &[Type],
    ) -> Option<TypedStat> {
        let typed_start = self.check_exp(start)?;
        let typed_finish = self.check_exp(finish)?;
        let typed_inc = match inc {
            Some(exp) => Some(self.check_exp(exp)?),
            None => None,
        };

        let var_ty = match &decl.r#type {
            Some(annotated) => self.resolve_type(annotated)?,
            None => typed_start.ty.clone(),
        };

        if !matches!(var_ty, Type::Integer | Type::Float) {
            self.error(
                decl.loc,
                format!(
                    "a variável de controle do `for` precisa ser integer ou float, encontrado {}.",
                    type_name(&var_ty)
                ),
            );
            return None;
        }

        let mut ok = true;
        for (typed, papel) in [
            (&typed_start, "valor inicial"),
            (&typed_finish, "limite"),
        ]
        .into_iter()
        .chain(typed_inc.iter().map(|t| (t, "passo")))
        {
            if !typed.ty.equals(&var_ty) {
                self.error(
                    typed.loc,
                    format!(
                        "o {papel} do `for` precisa ter o mesmo tipo da variável de controle ({}), encontrado {}.",
                        type_name(&var_ty),
                        type_name(&typed.ty)
                    ),
                );
                ok = false;
            }
        }
        if !ok {
            return None;
        }

        // `inc` omitido vira `1`/`1.0` conforme o tipo, com o `loc` do
        // limite (como `ast.ExpInteger(node.finish.loc, 1)` no original).
        let typed_inc = typed_inc.unwrap_or_else(|| TypedExp {
            loc: typed_finish.loc,
            ty: var_ty.clone(),
            kind: match var_ty {
                Type::Integer => TypedExpKind::Integer(1),
                _ => TypedExpKind::Float(1.0),
            },
        });

        // A variável de controle vive num bloco próprio que não vaza para
        // fora do laço (o corpo `StatBlock` abre o seu por cima).
        self.st.open_block();
        self.st
            .add_symbol(&decl.name, var_ty.clone(), SymbolKind::ForVar);
        let typed_block = self.check_stat(block, rettypes);
        self.st.close_block();

        Some(TypedStat::For {
            loc,
            name: decl.name.clone(),
            ty: var_ty,
            start: typed_start,
            finish: typed_finish,
            inc: typed_inc,
            block: Box::new(typed_block?),
        })
    }

    /// Atribuição single-target `nome = exp` (`checker.lua:378-410`).
    fn check_assign(&mut self, loc: Loc, var: &Var, exp: &Exp) -> Option<TypedStat> {
        let Var::VarName {
            loc: var_loc, name, ..
        } = var
        else {
            // Defensivo: o parser (T11) só produz `VarName` como alvo.
            self.error(
                loc,
                "atribuição a índice ou campo não é suportada nesta fase.",
            );
            return None;
        };

        let Some(symbol) = self.st.find_symbol(name).cloned() else {
            self.error(*var_loc, format!("'{name}' não foi declarado."));
            return None;
        };

        match symbol.kind {
            // Globais nesta fase são sempre funções (`print` e as top-level)
            // — "trying to assign to a function", `checker.lua:401`.
            SymbolKind::Global => {
                self.error(*var_loc, "não é possível atribuir a uma função.");
                return None;
            }
            SymbolKind::Param => {
                self.error(
                    *var_loc,
                    format!("não é possível atribuir ao parâmetro '{name}' nesta fase."),
                );
                return None;
            }
            // `ForVar` é sempre `mut` no template do T15 (nada a rastrear);
            // `Local` é registrada mais abaixo, após a atribuição validar.
            SymbolKind::ForVar | SymbolKind::Local { .. } => {}
        }

        let value = self.check_exp(exp)?;
        if !symbol.ty.compatible(&value.ty) {
            self.error(
                value.loc,
                format!(
                    "atribuição incompatível para '{name}': esperado {}, encontrado {}.",
                    type_name(&symbol.ty),
                    type_name(&value.ty)
                ),
            );
            return None;
        }

        if let SymbolKind::Local { decl_id } = symbol.kind {
            self.assigned.insert(decl_id);
        }

        Some(TypedStat::Assign {
            loc,
            name: name.clone(),
            value,
        })
    }

    fn check_exp(&mut self, exp: &Exp) -> Option<TypedExp> {
        match exp {
            Exp::ExpNil { loc } => Some(TypedExp {
                loc: *loc,
                ty: Type::Nil,
                kind: TypedExpKind::Nil,
            }),
            Exp::ExpBool { loc, value } => Some(TypedExp {
                loc: *loc,
                ty: Type::Boolean,
                kind: TypedExpKind::Bool(*value),
            }),
            Exp::ExpInteger { loc, value } => Some(TypedExp {
                loc: *loc,
                ty: Type::Integer,
                kind: TypedExpKind::Integer(*value),
            }),
            Exp::ExpFloat { loc, value } => Some(TypedExp {
                loc: *loc,
                ty: Type::Float,
                kind: TypedExpKind::Float(*value),
            }),
            Exp::ExpString { loc, value } => Some(TypedExp {
                loc: *loc,
                ty: Type::String,
                kind: TypedExpKind::String(value.clone()),
            }),
            Exp::ExpVar { loc, var } => self.check_var(loc, var),
            Exp::ExpConcat { loc, exps } => {
                let mut typed_exps = Vec::with_capacity(exps.len());
                let mut ok = true;
                for e in exps {
                    match self.check_exp(e) {
                        Some(typed) => {
                            if !matches!(typed.ty, Type::String | Type::Value) {
                                self.error(
                                    typed.loc,
                                    format!(
                                        "operando de `..` precisa ser string, encontrado {}.",
                                        type_name(&typed.ty)
                                    ),
                                );
                                ok = false;
                            }
                            typed_exps.push(typed);
                        }
                        None => ok = false,
                    }
                }
                if !ok {
                    return None;
                }
                Some(TypedExp {
                    loc: *loc,
                    ty: Type::String,
                    kind: TypedExpKind::Concat(typed_exps),
                })
            }
            Exp::ExpCall { loc, exp, args } => self.check_call(loc, exp, args),
            Exp::ExpInitList { loc, .. } => {
                self.error(
                    *loc,
                    "inicializador de array/record (`{...}`) não é suportado nesta fase.",
                );
                None
            }
            Exp::ExpUnop { loc, .. } => {
                self.error(*loc, "operador unário não é suportado nesta fase.");
                None
            }
            Exp::ExpBinop { loc, .. } => {
                self.error(
                    *loc,
                    "operador binário aritmético/lógico não é suportado nesta fase.",
                );
                None
            }
            Exp::ExpCast { loc, .. } => {
                self.error(*loc, "cast de tipo (`as`) não é suportado nesta fase.");
                None
            }
            Exp::ExpAdjust { loc, .. } | Exp::ExpExtra { loc, .. } => {
                self.error(
                    *loc,
                    "múltiplos valores de retorno não são suportados nesta fase.",
                );
                None
            }
        }
    }

    fn check_var(&mut self, _loc: &Loc, var: &Var) -> Option<TypedExp> {
        match var {
            Var::VarName { loc, name } => match self.st.find_symbol(name).cloned() {
                Some(symbol) => Some(TypedExp {
                    loc: *loc,
                    ty: symbol.ty,
                    kind: TypedExpKind::Var(name.clone()),
                }),
                None => {
                    self.error(*loc, format!("'{name}' não foi declarado."));
                    None
                }
            },
            Var::VarBracket { loc, .. } => {
                self.error(*loc, "indexação (`v[i]`) não é suportada nesta fase.");
                None
            }
            Var::VarDot { loc, .. } => {
                self.error(
                    *loc,
                    "acesso a campo (`v.campo`) não é suportado nesta fase.",
                );
                None
            }
        }
    }

    fn check_call(&mut self, loc: &Loc, callee: &Exp, args: &Args) -> Option<TypedExp> {
        let Exp::ExpVar { var, .. } = callee else {
            self.error(
                *loc,
                "só é possível chamar um nome de função diretamente nesta fase.",
            );
            return None;
        };
        let Var::VarName { name, .. } = var.as_ref() else {
            self.error(
                *loc,
                "só é possível chamar um nome de função diretamente nesta fase.",
            );
            return None;
        };

        let Args::ArgsFunc { args: arg_exps, .. } = args else {
            self.error(*loc, "chamada de método não é suportada nesta fase.");
            return None;
        };

        let Some(symbol) = self.st.find_symbol(name).cloned() else {
            self.error(*loc, format!("função '{name}' não foi declarada."));
            return None;
        };

        let Type::Function { params, rettypes } = symbol.ty else {
            self.error(*loc, format!("'{name}' não é uma função."));
            return None;
        };

        let mut typed_args = Vec::with_capacity(arg_exps.len());
        let mut ok = true;
        for arg in arg_exps {
            match self.check_exp(arg) {
                Some(typed) => typed_args.push(typed),
                None => ok = false,
            }
        }
        if !ok {
            return None;
        }

        if typed_args.len() != params.len() {
            self.error(
                *loc,
                format!(
                    "'{name}' espera {} argumento(s), mas recebeu {}.",
                    params.len(),
                    typed_args.len()
                ),
            );
            return None;
        }

        for (arg, expected) in typed_args.iter().zip(&params) {
            if !expected.compatible(&arg.ty) {
                self.error(
                    arg.loc,
                    format!(
                        "argumento incompatível em chamada de '{name}': esperado {}, encontrado {}.",
                        type_name(expected),
                        type_name(&arg.ty)
                    ),
                );
                return None;
            }
        }

        // A Fase 0 só produz uma função de retorno único; `rettypes[0]`
        // sempre existe porque toda assinatura coletada tem ao menos um tipo
        // de retorno (`TypeNil` quando omitido).
        let ty = rettypes.first().cloned().unwrap_or(Type::Nil);

        Some(TypedExp {
            loc: *loc,
            ty,
            kind: TypedExpKind::Call {
                callee: name.clone(),
                args: typed_args,
            },
        })
    }
}

/// Fix-up de mutabilidade (decisão 6 da Fase 1): percorre o corpo tipado da
/// função marcando `mutable = true` nas declarações cujo `decl_id` recebeu
/// alguma atribuição. Shadowing é respeitado naturalmente — o id registrado
/// veio do símbolo resolvido na pilha de escopos.
fn fixup_mutability(stat: &mut TypedStat, assigned: &HashSet<DeclId>) {
    match stat {
        TypedStat::Block { stats, .. } => {
            for s in stats {
                fixup_mutability(s, assigned);
            }
        }
        TypedStat::Decl {
            decl_id, mutable, ..
        } => {
            *mutable = assigned.contains(decl_id);
        }
        TypedStat::If {
            thens, elsestat, ..
        } => {
            for then in thens {
                fixup_mutability(&mut then.block, assigned);
            }
            if let Some(stat) = elsestat {
                fixup_mutability(stat, assigned);
            }
        }
        TypedStat::While { block, .. } | TypedStat::For { block, .. } => {
            fixup_mutability(block, assigned);
        }
        TypedStat::Call { .. } | TypedStat::Return { .. } | TypedStat::Assign { .. } => {}
    }
}

fn type_name(ty: &Type) -> String {
    match ty {
        Type::Invalid => "<inválido>".to_string(),
        Type::Nil => "nil".to_string(),
        Type::Boolean => "boolean".to_string(),
        Type::Integer => "integer".to_string(),
        Type::Float => "float".to_string(),
        Type::String => "string".to_string(),
        Type::Value => "value".to_string(),
        Type::Function { .. } => "function".to_string(),
        Type::Array { elem } => format!("{{{}}}", type_name(elem)),
        Type::Map { keys, values } => format!("map {{{}: {}}}", type_name(keys), type_name(values)),
        Type::Record { name, .. } => name.clone(),
        Type::Option { base } => format!("{}?", type_name(base)),
    }
}

/// Verifica o programa por completo, produzindo a AST tipada em caso de
/// sucesso.
///
/// Nunca panic: qualquer construção fora do subconjunto suportado, ou erro de
/// tipo, vira uma entrada em `Err`.
pub fn check(program: &Program) -> Result<TypedProgram, Vec<CheckError>> {
    let mut checker = Checker::new();

    for node in program {
        checker.collect_signature(node);
    }

    checker.check_has_main(program);

    let mut typed_program = Vec::with_capacity(program.len());
    for node in program {
        if let Some(typed) = checker.check_toplevel(node) {
            typed_program.push(typed);
        }
    }

    if checker.errors.is_empty() {
        Ok(typed_program)
    } else {
        Err(checker.errors)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::Decl;
    use crate::lexer::lex;
    use crate::parser::parse;

    fn check_source(source: &str) -> Result<TypedProgram, Vec<CheckError>> {
        let tokens =
            lex(source).unwrap_or_else(|e| panic!("fonte não deveria ter erro léxico: {e}"));
        let program =
            parse(&tokens).unwrap_or_else(|e| panic!("fonte não deveria ter erro sintático: {e}"));
        check(&program)
    }

    #[test]
    fn aceita_hello_titan() {
        let source = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../examples/hello.titan"
        ))
        .expect("examples/hello.titan deve existir");

        let typed = check_source(&source).unwrap_or_else(|errs| {
            panic!(
                "esperava sucesso, obteve erros: {}",
                errs.iter()
                    .map(|e| e.to_string())
                    .collect::<Vec<_>>()
                    .join("; ")
            )
        });

        assert_eq!(typed.len(), 1);
        let TypedTopLevel::Func {
            name,
            params,
            rettypes,
            ..
        } = &typed[0];
        assert_eq!(name, "main");
        assert_eq!(params.len(), 1);
        assert_eq!(
            params[0].1,
            Type::Array {
                elem: Box::new(Type::String)
            }
        );
        assert_eq!(rettypes, &vec![Type::Integer]);
    }

    #[test]
    fn print_com_argumento_incompativel_produz_erro() {
        let source = "function main(args: {string}): integer\n    print(42)\n    return 0\nend";
        let errs = check_source(source).unwrap_err();
        assert!(errs.iter().any(|e| e.message.contains("incompatível")));
    }

    #[test]
    fn chamada_a_funcao_nao_declarada_produz_erro() {
        let source =
            "function main(args: {string}): integer\n    funcao_inexistente()\n    return 0\nend";
        let errs = check_source(source).unwrap_err();
        assert!(errs.iter().any(|e| e.message.contains("não foi declarada")));
    }

    #[test]
    fn main_retornando_string_produz_erro_de_retorno_incompativel() {
        let source = "function main(args: {string}): integer\n    return \"oi\"\nend";
        let errs = check_source(source).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| e.message.contains("retorno incompatível"))
        );
    }

    #[test]
    fn assinatura_de_main_invalida_produz_erro() {
        let source = "function main(): integer\n    return 0\nend";
        let errs = check_source(source).unwrap_err();
        assert!(errs.iter().any(|e| e.message.contains("main")));
    }

    #[test]
    fn atribuicao_multipla_montada_a_mao_produz_erro_defensivo() {
        // O parser (T11) nunca produz multi-assign — AST montada à mão para
        // exercitar a rejeição defensiva do checker (PRD.md, T12).
        let loc = Loc { line: 1, col: 1 };
        let program: Program = vec![TopLevel::TopLevelFunc {
            loc,
            islocal: false,
            name: "main".to_string(),
            params: vec![Decl {
                loc,
                name: "args".to_string(),
                r#type: Some(ast::Type::TypeArray {
                    loc,
                    subtype: Box::new(ast::Type::TypeString { loc }),
                }),
                option: false,
            }],
            rettypes: vec![ast::Type::TypeInteger { loc }],
            block: Stat::StatBlock {
                loc,
                stats: vec![Stat::StatAssign {
                    loc,
                    vars: vec![
                        Var::VarName {
                            loc,
                            name: "a".to_string(),
                        },
                        Var::VarName {
                            loc,
                            name: "b".to_string(),
                        },
                    ],
                    exps: vec![
                        Exp::ExpInteger { loc, value: 1 },
                        Exp::ExpInteger { loc, value: 2 },
                    ],
                }],
            },
        }];

        let errs = check(&program).unwrap_err();
        assert!(errs.iter().any(|e| e.message.contains("atribuição múltipla")));
    }

    #[test]
    fn foreign_import_e_record_produzem_erro_de_construcao_nao_suportada() {
        // Mesmo espírito de um arquivo `.titan` do Titan original: `record`
        // e `foreign import` não fazem parte do subconjunto da Fase 0.
        let loc = Loc { line: 1, col: 1 };
        let program: Program = vec![
            TopLevel::TopLevelForeignImport {
                loc,
                localname: "stdio".to_string(),
                headername: "stdio.h".to_string(),
            },
            TopLevel::TopLevelRecord {
                loc,
                name: "Ponto".to_string(),
                fields: vec![],
            },
        ];

        let errs = check(&program).unwrap_err();
        assert!(errs.iter().any(|e| e.message.contains("foreign import")));
        assert!(errs.iter().any(|e| e.message.contains("record")));
    }

    #[test]
    fn chamada_antes_da_declaracao_e_permitida() {
        let source = r#"function chamador(): integer
    return chamado()
end

function chamado(): integer
    return 1
end"#;
        // `chamador` está definido antes de `chamado`, mas a passada 1 já
        // coletou todas as assinaturas — não deve haver erro de "main"
        // fora daqui, então filtramos essa mensagem específica.
        let result = check_source(source);
        match result {
            Ok(_) => panic!("esperava erro só por falta de 'main' válida"),
            Err(errs) => {
                assert!(errs.iter().all(|e| e.message.contains("main")));
            }
        }
    }

    // ---- Fase 1 (T12): if / while / for / atribuição --------------------

    /// Verifica `source` e devolve os statements tipados do corpo da
    /// primeira função.
    fn typed_body_stats(source: &str) -> Vec<TypedStat> {
        let typed = check_source(source).unwrap_or_else(|errs| {
            panic!(
                "esperava sucesso, obteve erros: {}",
                errs.iter()
                    .map(|e| e.to_string())
                    .collect::<Vec<_>>()
                    .join("; ")
            )
        });
        let TypedTopLevel::Func { body, .. } = &typed[0];
        let TypedStat::Block { stats, .. } = body else {
            panic!("esperava TypedStat::Block como corpo");
        };
        stats.clone()
    }

    #[test]
    fn aceita_if_while_for_e_atribuicao() {
        let stats = typed_body_stats(
            "function main(args: {string}): integer\n\
             \x20   local x: integer = 0\n\
             \x20   if true then\n\
             \x20       x = 1\n\
             \x20   else\n\
             \x20       x = 2\n\
             \x20   end\n\
             \x20   while false do\n\
             \x20       x = 3\n\
             \x20   end\n\
             \x20   for i = 1, 10 do\n\
             \x20       x = 4\n\
             \x20   end\n\
             \x20   return x\n\
             end",
        );

        let TypedStat::If {
            thens, elsestat, ..
        } = &stats[1]
        else {
            panic!("esperava TypedStat::If, obteve {:?}", stats[1]);
        };
        assert_eq!(thens.len(), 1);
        assert_eq!(thens[0].condition.ty, Type::Boolean);
        assert!(elsestat.is_some());

        let TypedStat::While { condition, .. } = &stats[2] else {
            panic!("esperava TypedStat::While, obteve {:?}", stats[2]);
        };
        assert_eq!(condition.ty, Type::Boolean);

        let TypedStat::For { ty, inc, .. } = &stats[3] else {
            panic!("esperava TypedStat::For, obteve {:?}", stats[3]);
        };
        assert_eq!(*ty, Type::Integer);
        assert!(matches!(inc.kind, TypedExpKind::Integer(1)));
    }

    #[test]
    fn condicao_de_if_nao_boolean_produz_erro() {
        let source =
            "function main(args: {string}): integer\n    if 42 then\n    end\n    return 0\nend";
        let errs = check_source(source).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| e.message.contains("condição") && e.message.contains("boolean"))
        );
    }

    #[test]
    fn condicao_de_while_nao_boolean_produz_erro() {
        let source = "function main(args: {string}): integer\n    while \"oi\" do\n    end\n    return 0\nend";
        let errs = check_source(source).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| e.message.contains("condição") && e.message.contains("boolean"))
        );
    }

    #[test]
    fn for_com_variavel_nao_numerica_produz_erro() {
        let source = "function main(args: {string}): integer\n    for x = \"a\", 10 do\n    end\n    return 0\nend";
        let errs = check_source(source).unwrap_err();
        assert!(errs.iter().any(|e| e.message.contains("integer ou float")));
    }

    #[test]
    fn for_com_tipos_nao_identicos_produz_erro() {
        // Decisão 5 da Fase 1: sem coerção int→float no `for`.
        let source = "function main(args: {string}): integer\n    for x = 1, 10.0 do\n    end\n    return 0\nend";
        let errs = check_source(source).unwrap_err();
        assert!(errs.iter().any(|e| e.message.contains("mesmo tipo")));
    }

    #[test]
    fn for_float_ganha_inc_default_float() {
        let stats = typed_body_stats(
            "function main(args: {string}): integer\n\
             \x20   for f = 1.5, 2.5 do\n\
             \x20   end\n\
             \x20   return 0\n\
             end",
        );
        let TypedStat::For { ty, inc, .. } = &stats[0] else {
            panic!("esperava TypedStat::For");
        };
        assert_eq!(*ty, Type::Float);
        assert_eq!(inc.ty, Type::Float);
        assert!(matches!(inc.kind, TypedExpKind::Float(v) if v == 1.0));
    }

    #[test]
    fn variavel_do_for_nao_vaza_do_laco() {
        let source = "function main(args: {string}): integer\n\
             \x20   for i = 1, 10 do\n\
             \x20   end\n\
             \x20   i = 5\n\
             \x20   return 0\n\
             end";
        let errs = check_source(source).unwrap_err();
        assert!(errs.iter().any(|e| e.message.contains("'i' não foi declarado")));
    }

    #[test]
    fn atribuir_a_variavel_de_controle_do_for_e_permitido() {
        // O original também permite (a variável é uma declaração comum);
        // no template do T15 ela é sempre `mut`, sem rastreio.
        let source = "function main(args: {string}): integer\n\
             \x20   for i = 1, 10 do\n\
             \x20       i = 5\n\
             \x20   end\n\
             \x20   return 0\n\
             end";
        check_source(source).unwrap_or_else(|errs| {
            panic!(
                "esperava sucesso, obteve erros: {}",
                errs.iter()
                    .map(|e| e.to_string())
                    .collect::<Vec<_>>()
                    .join("; ")
            )
        });
    }

    #[test]
    fn atribuicao_sem_declaracao_produz_erro() {
        let source = "function main(args: {string}): integer\n    x = 10\n    return 0\nend";
        let errs = check_source(source).unwrap_err();
        assert!(errs.iter().any(|e| e.message.contains("'x' não foi declarado")));
    }

    #[test]
    fn atribuir_a_funcao_produz_erro() {
        let source = "function main(args: {string}): integer\n    print = 1\n    return 0\nend";
        let errs = check_source(source).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| e.message.contains("não é possível atribuir a uma função"))
        );
    }

    #[test]
    fn atribuir_a_parametro_produz_erro_nesta_fase() {
        // Divergência documentada do original (que permite): parâmetros não
        // têm rastreio de `mut`, e o Rust gerado não compilaria.
        let source = "function f(x: integer): integer\n\
             \x20   x = 1\n\
             \x20   return x\n\
             end\n\
             function main(args: {string}): integer\n\
             \x20   return 0\n\
             end";
        let errs = check_source(source).unwrap_err();
        assert!(errs.iter().any(|e| e.message.contains("parâmetro")));
    }

    #[test]
    fn atribuicao_com_tipo_incompativel_produz_erro() {
        let source = "function main(args: {string}): integer\n\
             \x20   local x: integer = 0\n\
             \x20   x = \"oi\"\n\
             \x20   return x\n\
             end";
        let errs = check_source(source).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| e.message.contains("atribuição incompatível"))
        );
    }

    #[test]
    fn mutabilidade_marca_somente_locais_reatribuidos() {
        let stats = typed_body_stats(
            "function main(args: {string}): integer\n\
             \x20   local x: integer = 0\n\
             \x20   local y: integer = 1\n\
             \x20   while true do\n\
             \x20       x = 2\n\
             \x20   end\n\
             \x20   return y\n\
             end",
        );
        let TypedStat::Decl { name, mutable, .. } = &stats[0] else {
            panic!("esperava TypedStat::Decl");
        };
        assert_eq!(name, "x");
        assert!(*mutable, "x é reatribuída dentro do while → mutable");

        let TypedStat::Decl { name, mutable, .. } = &stats[1] else {
            panic!("esperava TypedStat::Decl");
        };
        assert_eq!(name, "y");
        assert!(!*mutable, "y nunca é reatribuída → imutável");
    }

    #[test]
    fn shadowing_marca_somente_a_declaracao_interna() {
        let stats = typed_body_stats(
            "function main(args: {string}): integer\n\
             \x20   local x: integer = 1\n\
             \x20   if true then\n\
             \x20       local x: integer = 2\n\
             \x20       x = 3\n\
             \x20   end\n\
             \x20   return x\n\
             end",
        );
        let TypedStat::Decl { mutable, .. } = &stats[0] else {
            panic!("esperava TypedStat::Decl externa");
        };
        assert!(!*mutable, "a externa nunca é atingida — o `x = 3` resolve para a interna");

        let TypedStat::If { thens, .. } = &stats[1] else {
            panic!("esperava TypedStat::If");
        };
        let TypedStat::Block { stats: inner, .. } = &thens[0].block else {
            panic!("esperava TypedStat::Block no ramo then");
        };
        let TypedStat::Decl { mutable, .. } = &inner[0] else {
            panic!("esperava TypedStat::Decl interna");
        };
        assert!(*mutable, "a interna é a atingida pelo `x = 3`");
    }
}
