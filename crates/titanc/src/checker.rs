//! Análise semântica e verificação de tipos do Titan.
//!
//! Espelha a estratégia de `titan/titan-compiler/checker.lua` (1662 linhas),
//! reduzida ao subconjunto da Fase 0 (T5 do PRD.md):
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
//!
//! Como `ast::Exp` é um valor imutável (ao contrário do Lua, que anexa
//! `_type` dinamicamente ao nó), o checker produz uma **AST tipada paralela**
//! (`TypedProgram` e companhia) em vez de mutar a árvore original — é o que
//! `codegen.rs` (T6) vai consumir.
//!
//! Tudo fora do subconjunto da Fase 0 (`if`/`while`/`for`, records, maps,
//! arrays manipuláveis, `import`, `foreign import`, métodos, operadores
//! aritméticos, retornos múltiplos, `Option`/`?`) produz um erro semântico
//! claro — nunca panic.

use std::collections::HashMap;

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

/// Pilha de escopos léxicos. Cada bloco é um `HashMap` de nome → tipo.
struct SymTab {
    blocks: Vec<HashMap<String, Type>>,
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

    fn add_symbol(&mut self, name: &str, ty: Type) {
        self.blocks
            .last_mut()
            .expect("symtab sempre tem pelo menos um bloco")
            .insert(name.to_string(), ty);
    }

    fn find_symbol(&self, name: &str) -> Option<&Type> {
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
    },
    Call {
        loc: Loc,
        call: TypedExp,
    },
    Return {
        loc: Loc,
        exps: Vec<TypedExp>,
    },
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
        );
        Checker {
            st,
            errors: Vec::new(),
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
                let Some(Type::Function {
                    params: param_types,
                    rettypes: ret_types,
                }) = self.st.find_symbol(name).cloned()
                else {
                    // Assinatura já rejeitada na passada 1.
                    return None;
                };

                self.st.open_block();
                for (param, ty) in params.iter().zip(param_types.iter()) {
                    self.st.add_symbol(&param.name, ty.clone());
                }

                let body = self.check_stat(block, &ret_types);

                self.st.close_block();

                let body = body?;
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

                self.st.add_symbol(&decl.name, ty.clone());

                Some(TypedStat::Decl {
                    loc: *loc,
                    name: decl.name.clone(),
                    ty,
                    value,
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
            Stat::StatIf { loc, .. } => {
                self.error(*loc, "`if` não é suportado nesta fase.");
                None
            }
            Stat::StatWhile { loc, .. } => {
                self.error(*loc, "`while` não é suportado nesta fase.");
                None
            }
            Stat::StatRepeat { loc, .. } => {
                self.error(*loc, "`repeat` não é suportado nesta fase.");
                None
            }
            Stat::StatFor { loc, .. } => {
                self.error(*loc, "`for` não é suportado nesta fase.");
                None
            }
            Stat::StatAssign { loc, .. } => {
                self.error(*loc, "atribuição (`x = exp`) não é suportada nesta fase.");
                None
            }
        }
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
                Some(ty) => Some(TypedExp {
                    loc: *loc,
                    ty,
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

        let Some(callee_type) = self.st.find_symbol(name).cloned() else {
            self.error(*loc, format!("função '{name}' não foi declarada."));
            return None;
        };

        let Type::Function { params, rettypes } = callee_type else {
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
    fn if_fora_do_subconjunto_produz_erro_claro_sem_panic() {
        // O parser (T4) já não reconhece `if`, então a AST precisa ser
        // montada à mão para exercitar a rejeição do checker diretamente.
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
                stats: vec![Stat::StatIf {
                    loc,
                    thens: vec![],
                    elsestat: None,
                }],
            },
        }];

        let errs = check(&program).unwrap_err();
        assert!(errs.iter().any(|e| e.message.contains("`if`")));
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
}
