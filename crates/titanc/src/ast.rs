//! Estruturas da AST do Titan.
//!
//! Espelha `titan/titan-compiler/ast.lua`: mesmos nós, mesmos nomes de campo,
//! para que o Titan original continue servindo de referência viva. Todos os
//! enums são definidos por completo mesmo que parser/checker das fases
//! iniciais só produzam/aceitem um subconjunto.

/// Posição de um nó no código-fonte.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Loc {
    pub line: usize,
    pub col: usize,
}

/// Um programa Titan é uma lista de declarações de topo — o Titan original
/// também não tem um nó `Program`.
pub type Program = Vec<TopLevel>;

/// Anotações de tipo escritas no código-fonte (`ast.lua`: `Type`).
#[derive(Debug, Clone, PartialEq)]
pub enum Type {
    TypeNil {
        loc: Loc,
    },
    TypeBoolean {
        loc: Loc,
    },
    TypeInteger {
        loc: Loc,
    },
    TypeFloat {
        loc: Loc,
    },
    TypeString {
        loc: Loc,
    },
    TypeValue {
        loc: Loc,
    },
    TypeQualName {
        loc: Loc,
        module: String,
        name: String,
    },
    TypeName {
        loc: Loc,
        name: String,
    },
    TypeArray {
        loc: Loc,
        subtype: Box<Type>,
    },
    TypeMap {
        loc: Loc,
        keystype: Box<Type>,
        valuestype: Box<Type>,
    },
    TypeFunction {
        loc: Loc,
        argtypes: Vec<Type>,
        rettypes: Vec<Type>,
    },
    TypeOption {
        loc: Loc,
        basetype: Box<Type>,
    },
}

/// Declarações de topo de arquivo (`ast.lua`: `TopLevel`).
#[derive(Debug, Clone, PartialEq)]
pub enum TopLevel {
    TopLevelMethod {
        loc: Loc,
        class: String,
        name: String,
        params: Vec<Decl>,
        rettypes: Vec<Type>,
        block: Stat,
    },
    TopLevelStatic {
        loc: Loc,
        class: String,
        name: String,
        params: Vec<Decl>,
        rettypes: Vec<Type>,
        block: Stat,
    },
    TopLevelFunc {
        loc: Loc,
        islocal: bool,
        name: String,
        params: Vec<Decl>,
        rettypes: Vec<Type>,
        block: Stat,
    },
    TopLevelVar {
        loc: Loc,
        islocal: bool,
        decl: Decl,
        value: Exp,
    },
    TopLevelRecord {
        loc: Loc,
        name: String,
        fields: Vec<Decl>,
    },
    TopLevelImport {
        loc: Loc,
        localname: String,
        modname: String,
    },
    TopLevelForeignImport {
        loc: Loc,
        localname: String,
        headername: String,
    },
}

/// Declaração de nome tipado (`ast.lua`: `Decl.Decl`).
///
/// `type` é `None` quando o tipo não foi escrito explicitamente (inferência);
/// `option` marca uma declaração opcional sem tipo (`local x?`).
#[derive(Debug, Clone, PartialEq)]
pub struct Decl {
    pub loc: Loc,
    pub name: String,
    pub r#type: Option<Type>,
    pub option: bool,
}

/// Comandos (`ast.lua`: `Stat`).
#[derive(Debug, Clone, PartialEq)]
pub enum Stat {
    StatBlock {
        loc: Loc,
        stats: Vec<Stat>,
    },
    StatWhile {
        loc: Loc,
        condition: Exp,
        block: Box<Stat>,
    },
    StatRepeat {
        loc: Loc,
        block: Box<Stat>,
        condition: Exp,
    },
    StatIf {
        loc: Loc,
        thens: Vec<Then>,
        elsestat: Option<Box<Stat>>,
    },
    StatFor {
        loc: Loc,
        decl: Box<Decl>,
        start: Box<Exp>,
        finish: Box<Exp>,
        inc: Option<Box<Exp>>,
        block: Box<Stat>,
    },
    StatAssign {
        loc: Loc,
        vars: Vec<Var>,
        exps: Vec<Exp>,
    },
    StatDecl {
        loc: Loc,
        decls: Vec<Decl>,
        exps: Vec<Exp>,
    },
    StatCall {
        loc: Loc,
        callexp: Exp,
    },
    StatReturn {
        loc: Loc,
        exps: Vec<Exp>,
    },
}

/// Ramo `then` de um `if` (`ast.lua`: `Then.Then`).
#[derive(Debug, Clone, PartialEq)]
pub struct Then {
    pub loc: Loc,
    pub condition: Exp,
    pub block: Stat,
}

/// Lugares atribuíveis (`ast.lua`: `Var`).
#[derive(Debug, Clone, PartialEq)]
pub enum Var {
    VarName {
        loc: Loc,
        name: String,
    },
    VarBracket {
        loc: Loc,
        exp1: Box<Exp>,
        exp2: Box<Exp>,
    },
    VarDot {
        loc: Loc,
        exp: Box<Exp>,
        name: String,
    },
}

/// Expressões (`ast.lua`: `Exp`).
#[derive(Debug, Clone, PartialEq)]
pub enum Exp {
    ExpNil {
        loc: Loc,
    },
    ExpBool {
        loc: Loc,
        value: bool,
    },
    ExpInteger {
        loc: Loc,
        value: i64,
    },
    ExpFloat {
        loc: Loc,
        value: f64,
    },
    ExpString {
        loc: Loc,
        value: String,
    },
    ExpInitList {
        loc: Loc,
        fields: Vec<Field>,
    },
    ExpCall {
        loc: Loc,
        exp: Box<Exp>,
        args: Args,
    },
    ExpVar {
        loc: Loc,
        var: Box<Var>,
    },
    ExpUnop {
        loc: Loc,
        op: String,
        exp: Box<Exp>,
    },
    ExpConcat {
        loc: Loc,
        exps: Vec<Exp>,
    },
    ExpBinop {
        loc: Loc,
        lhs: Box<Exp>,
        op: String,
        rhs: Box<Exp>,
    },
    ExpCast {
        loc: Loc,
        exp: Box<Exp>,
        target: Type,
    },
    /// Ajusta uma chamada para produzir apenas o primeiro valor de retorno.
    ExpAdjust {
        loc: Loc,
        exp: Box<Exp>,
    },
    /// index-ésimo (>1) valor de retorno de uma chamada.
    ExpExtra {
        loc: Loc,
        exp: Box<Exp>,
        index: usize,
        r#type: Option<Type>,
    },
}

/// Argumentos de chamada (`ast.lua`: `Args`).
#[derive(Debug, Clone, PartialEq)]
pub enum Args {
    ArgsFunc {
        loc: Loc,
        args: Vec<Exp>,
    },
    ArgsMethod {
        loc: Loc,
        method: String,
        args: Vec<Exp>,
    },
}

/// Identidade de um campo de `ExpInitList` (`ast.lua`: `Field.name`, que é
/// polimórfico — `false | string | Exp`).
#[derive(Debug, Clone, PartialEq)]
pub enum FieldName {
    /// Sem chave: posicional, entra num array.
    None,
    /// Chave-nome: `x = 1`, campo de record.
    Name(String),
    /// Chave-expressão: `["a"] = 1`, entrada de map.
    Key(Box<Exp>),
}

/// Campo de record ou de `ExpInitList` (`ast.lua`: `Field.Field`).
#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    pub loc: Loc,
    pub name: FieldName,
    pub exp: Exp,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `{1,2}`, `{x=1}` e `{["a"]=1}` produzem `FieldName` distintos e
    /// representáveis (critério de aceite do T27).
    #[test]
    fn field_name_distingue_posicional_nome_e_chave() {
        let loc = Loc { line: 1, col: 1 };

        let posicional = Field {
            loc,
            name: FieldName::None,
            exp: Exp::ExpInteger { loc, value: 1 },
        };
        assert_eq!(posicional.name, FieldName::None);

        let nomeado = Field {
            loc,
            name: FieldName::Name("x".to_string()),
            exp: Exp::ExpInteger { loc, value: 1 },
        };
        assert_eq!(nomeado.name, FieldName::Name("x".to_string()));

        let chave = Field {
            loc,
            name: FieldName::Key(Box::new(Exp::ExpString {
                loc,
                value: "a".to_string(),
            })),
            exp: Exp::ExpInteger { loc, value: 1 },
        };
        assert_eq!(
            chave.name,
            FieldName::Key(Box::new(Exp::ExpString {
                loc,
                value: "a".to_string(),
            }))
        );

        assert_ne!(posicional.name, nomeado.name);
        assert_ne!(nomeado.name, chave.name);
    }

    /// Monta a `Program` equivalente a `examples/hello.titan`, para provar que
    /// os enums cobrem o subconjunto que a Fase 0 precisa produzir.
    #[test]
    fn monta_ast_do_hello_world() {
        let loc = Loc { line: 1, col: 1 };

        let corpo = Stat::StatBlock {
            loc,
            stats: vec![
                Stat::StatCall {
                    loc,
                    callexp: Exp::ExpCall {
                        loc,
                        exp: Box::new(Exp::ExpVar {
                            loc,
                            var: Box::new(Var::VarName {
                                loc,
                                name: "print".to_string(),
                            }),
                        }),
                        args: Args::ArgsFunc {
                            loc,
                            args: vec![Exp::ExpString {
                                loc,
                                value: "Olá, mundo!".to_string(),
                            }],
                        },
                    },
                },
                Stat::StatReturn {
                    loc,
                    exps: vec![Exp::ExpInteger { loc, value: 0 }],
                },
            ],
        };

        let programa: Program = vec![TopLevel::TopLevelFunc {
            loc,
            islocal: false,
            name: "main".to_string(),
            params: vec![Decl {
                loc,
                name: "args".to_string(),
                r#type: Some(Type::TypeArray {
                    loc,
                    subtype: Box::new(Type::TypeString { loc }),
                }),
                option: false,
            }],
            rettypes: vec![Type::TypeInteger { loc }],
            block: corpo,
        }];

        assert_eq!(programa.len(), 1);
        match &programa[0] {
            TopLevel::TopLevelFunc { name, params, .. } => {
                assert_eq!(name, "main");
                assert_eq!(params.len(), 1);
            }
            _ => panic!("esperava TopLevelFunc"),
        }
    }
}
