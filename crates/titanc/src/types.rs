//! Sistema de tipos do Titan.
//!
//! Espelha `titan/titan-compiler/types.lua`: mesmas variantes, para que o
//! Titan original continue servindo de referência viva. Nesta fase (T2) só
//! primitivas, `Function` e `Array` são exercitadas pelo checker; as demais
//! variantes (`Map`, `Record`, `Option`) já entram completas para não exigir
//! refatoração nas fases seguintes.

/// Um tipo Titan (`types.lua`: `Type`).
#[derive(Debug, Clone, PartialEq)]
pub enum Type {
    Invalid,
    Nil,
    Boolean,
    Integer,
    Float,
    String,
    /// Tipo dinâmico do gradual typing: compatível com qualquer outro tipo.
    Value,
    Function {
        params: Vec<Type>,
        rettypes: Vec<Type>,
    },
    Array {
        elem: Box<Type>,
    },
    Map {
        keys: Box<Type>,
        values: Box<Type>,
    },
    Record {
        name: String,
        fields: Vec<(String, Type)>,
    },
    Option {
        base: Box<Type>,
    },
    /// Tipo que vem do runtime de uma capability: o programa não pode
    /// inspecionar sua representação, só chamar métodos sobre ele.
    /// `rust_path` é detalhe de emissão (não entra na identidade do tipo).
    Opaque {
        module: String,
        name: String,
        rust_path: String,
    },
}

impl Type {
    /// Igualdade estrutural de tipos (`types.lua:199`, `types.equals`).
    pub fn equals(&self, other: &Type) -> bool {
        match (self, other) {
            (Type::Array { elem: e1 }, Type::Array { elem: e2 }) => e1.equals(e2),
            (
                Type::Map {
                    keys: k1,
                    values: v1,
                },
                Type::Map {
                    keys: k2,
                    values: v2,
                },
            ) => k1.equals(k2) && v1.equals(v2),
            (
                Type::Function {
                    params: p1,
                    rettypes: r1,
                },
                Type::Function {
                    params: p2,
                    rettypes: r2,
                },
            ) => types_equal(p1, p2) && types_equal(r1, r2),
            (Type::Record { name: n1, .. }, Type::Record { name: n2, .. }) => n1 == n2,
            (Type::Option { base: b1 }, Type::Option { base: b2 }) => b1.equals(b2),
            (
                Type::Opaque {
                    module: m1,
                    name: n1,
                    ..
                },
                Type::Opaque {
                    module: m2,
                    name: n2,
                    ..
                },
            ) => m1 == m2 && n1 == n2,
            _ => std::mem::discriminant(self) == std::mem::discriminant(other),
        }
    }

    /// Relação de consistência de tipos, à la gradual typing (`types.lua:150`,
    /// `types.compatible`): `Value` é compatível com qualquer tipo.
    ///
    /// `Array`/`Map` são **invariantes** (via `equals`, não recursão em
    /// `compatible`): com arrays mutáveis passados por `&mut` (Fase 2, decisão
    /// 4), covariância seria *unsound* — permitiria escrever uma `string`
    /// através de uma referência `{value}` apontando para um `{integer}`. Ver
    /// ADR 0008. `Record` é nominal (via `equals`) e `Option` é invariante;
    /// ambos ganham braço explícito para deixar a intenção escrita, em vez de
    /// cair no `_ => false`.
    pub fn compatible(&self, other: &Type) -> bool {
        if self.equals(other) {
            return true;
        }

        match (self, other) {
            (Type::Value, _) | (_, Type::Value) => true,
            (Type::Array { elem: e1 }, Type::Array { elem: e2 }) => e1.equals(e2),
            (
                Type::Map {
                    keys: k1,
                    values: v1,
                },
                Type::Map {
                    keys: k2,
                    values: v2,
                },
            ) => k1.equals(k2) && v1.equals(v2),
            (
                Type::Function {
                    params: p1,
                    rettypes: r1,
                },
                Type::Function {
                    params: p2,
                    rettypes: r2,
                },
            ) => {
                p1.len() == p2.len()
                    && p1.iter().zip(p2).all(|(a, b)| a.compatible(b))
                    && r1.len() == r2.len()
                    && r1.iter().zip(r2).all(|(a, b)| a.compatible(b))
            }
            (Type::Record { .. }, Type::Record { .. }) => false,
            (Type::Option { .. }, Type::Option { .. }) => false,
            (Type::Opaque { .. }, Type::Opaque { .. }) => false,
            _ => false,
        }
    }
}

fn types_equal(a: &[Type], b: &[Type]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.equals(y))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primitivas_iguais_a_si_mesmas() {
        assert!(Type::Integer.equals(&Type::Integer));
        assert!(Type::String.equals(&Type::String));
        assert!(!Type::Integer.equals(&Type::Float));
    }

    #[test]
    fn arrays_iguais_por_elemento() {
        let a1 = Type::Array {
            elem: Box::new(Type::Integer),
        };
        let a2 = Type::Array {
            elem: Box::new(Type::Integer),
        };
        let a3 = Type::Array {
            elem: Box::new(Type::String),
        };
        assert!(a1.equals(&a2));
        assert!(!a1.equals(&a3));
    }

    #[test]
    fn funcoes_iguais_por_assinatura() {
        let f1 = Type::Function {
            params: vec![Type::String],
            rettypes: vec![Type::Nil],
        };
        let f2 = Type::Function {
            params: vec![Type::String],
            rettypes: vec![Type::Nil],
        };
        let f3 = Type::Function {
            params: vec![Type::Integer],
            rettypes: vec![Type::Nil],
        };
        assert!(f1.equals(&f2));
        assert!(!f1.equals(&f3));
    }

    #[test]
    fn value_e_compativel_com_qualquer_tipo() {
        assert!(Type::Value.compatible(&Type::Integer));
        assert!(Type::String.compatible(&Type::Value));
        assert!(Type::Value.compatible(&Type::Array {
            elem: Box::new(Type::Boolean)
        }));
    }

    #[test]
    fn tipos_iguais_sao_compativeis() {
        assert!(Type::Integer.compatible(&Type::Integer));
        assert!(!Type::Integer.compatible(&Type::Boolean));
    }

    #[test]
    fn arrays_sao_invariantes_em_compatible() {
        // ADR 0008: `Array` deixou de ser covariante em `compatible` — com
        // arrays mutáveis por `&mut` (Fase 2), `{value}` aceitar `{integer}`
        // seria unsound (escrever `string` através de `{value}` que aponta
        // para um `{integer}`).
        let a_value = Type::Array {
            elem: Box::new(Type::Value),
        };
        let a_integer = Type::Array {
            elem: Box::new(Type::Integer),
        };
        assert!(!a_value.compatible(&a_integer));
        assert!(!a_integer.compatible(&a_value));

        let a_integer2 = Type::Array {
            elem: Box::new(Type::Integer),
        };
        assert!(a_integer.compatible(&a_integer2));

        // `value` (fora de um composto) segue compatível com qualquer coisa,
        // inclusive um array — a invariância vale só *dentro* do composto.
        assert!(Type::Value.compatible(&a_integer));
        assert!(a_integer.compatible(&Type::Value));
    }

    #[test]
    fn maps_sao_invariantes_em_compatible() {
        let m_value = Type::Map {
            keys: Box::new(Type::String),
            values: Box::new(Type::Value),
        };
        let m_integer = Type::Map {
            keys: Box::new(Type::String),
            values: Box::new(Type::Integer),
        };
        assert!(!m_value.compatible(&m_integer));
        assert!(!m_integer.compatible(&m_value));
    }

    #[test]
    fn records_sao_nominais_e_invariantes_em_compatible() {
        let p1 = Type::Record {
            name: "P".to_string(),
            fields: vec![("x".to_string(), Type::Integer)],
        };
        let p2 = Type::Record {
            name: "P".to_string(),
            fields: vec![("x".to_string(), Type::Integer)],
        };
        let q = Type::Record {
            name: "Q".to_string(),
            fields: vec![("x".to_string(), Type::Integer)],
        };
        assert!(p1.compatible(&p2));
        assert!(!p1.compatible(&q));
    }

    #[test]
    fn options_sao_invariantes_em_compatible() {
        let o_value = Type::Option {
            base: Box::new(Type::Value),
        };
        let o_integer = Type::Option {
            base: Box::new(Type::Integer),
        };
        assert!(!o_value.compatible(&o_integer));
        assert!(!o_integer.compatible(&o_value));

        let o_integer2 = Type::Option {
            base: Box::new(Type::Integer),
        };
        assert!(o_integer.compatible(&o_integer2));
    }

    #[test]
    fn opacos_sao_iguais_por_modulo_e_nome() {
        let df1 = Type::Opaque {
            module: "data".to_string(),
            name: "DataFrame".to_string(),
            rust_path: "titan_data::DataFrame".to_string(),
        };
        let df2 = Type::Opaque {
            module: "data".to_string(),
            name: "DataFrame".to_string(),
            // `rust_path` não entra na comparação: é detalhe de emissão, não
            // identidade do tipo.
            rust_path: "outro::caminho::DataFrame".to_string(),
        };
        let serie = Type::Opaque {
            module: "data".to_string(),
            name: "Series".to_string(),
            rust_path: "titan_data::Series".to_string(),
        };
        assert!(df1.equals(&df2));
        assert!(!df1.equals(&serie));
    }

    #[test]
    fn opacos_sao_nominais_e_invariantes_em_compatible() {
        let df1 = Type::Opaque {
            module: "data".to_string(),
            name: "DataFrame".to_string(),
            rust_path: "titan_data::DataFrame".to_string(),
        };
        let df2 = Type::Opaque {
            module: "data".to_string(),
            name: "DataFrame".to_string(),
            rust_path: "titan_data::DataFrame".to_string(),
        };
        let serie = Type::Opaque {
            module: "data".to_string(),
            name: "Series".to_string(),
            rust_path: "titan_data::Series".to_string(),
        };
        assert!(df1.compatible(&df2));
        assert!(!df1.compatible(&serie));
    }

    #[test]
    fn opacos_de_modulos_diferentes_com_mesmo_nome_nao_sao_compativeis() {
        let df_data = Type::Opaque {
            module: "data".to_string(),
            name: "DataFrame".to_string(),
            rust_path: "titan_data::DataFrame".to_string(),
        };
        let df_outro = Type::Opaque {
            module: "outro_modulo".to_string(),
            name: "DataFrame".to_string(),
            rust_path: "outro_modulo::DataFrame".to_string(),
        };
        assert!(!df_data.equals(&df_outro));
        assert!(!df_data.compatible(&df_outro));
    }

    #[test]
    fn value_e_compativel_com_opaco() {
        let df = Type::Opaque {
            module: "data".to_string(),
            name: "DataFrame".to_string(),
            rust_path: "titan_data::DataFrame".to_string(),
        };
        assert!(Type::Value.compatible(&df));
        assert!(df.compatible(&Type::Value));
    }

    #[test]
    fn funcoes_compativeis_por_assinatura_compativel() {
        let f1 = Type::Function {
            params: vec![Type::Value],
            rettypes: vec![Type::Nil],
        };
        let f2 = Type::Function {
            params: vec![Type::Integer],
            rettypes: vec![Type::Nil],
        };
        assert!(f1.compatible(&f2));

        let f3 = Type::Function {
            params: vec![Type::Integer, Type::Integer],
            rettypes: vec![Type::Nil],
        };
        assert!(!f1.compatible(&f3));
    }

    #[test]
    fn print_do_runtime_tem_assinatura_esperada() {
        let print_type = Type::Function {
            params: vec![Type::String],
            rettypes: vec![Type::Nil],
        };
        assert!(!print_type.compatible(&Type::Function {
            params: vec![Type::Integer],
            rettypes: vec![Type::Nil],
        }));
    }
}
