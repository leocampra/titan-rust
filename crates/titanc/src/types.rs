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
            _ => std::mem::discriminant(self) == std::mem::discriminant(other),
        }
    }

    /// Relação de consistência de tipos, à la gradual typing (`types.lua:150`,
    /// `types.compatible`): `Value` é compatível com qualquer tipo.
    pub fn compatible(&self, other: &Type) -> bool {
        if self.equals(other) {
            return true;
        }

        match (self, other) {
            (Type::Value, _) | (_, Type::Value) => true,
            (Type::Array { elem: e1 }, Type::Array { elem: e2 }) => e1.compatible(e2),
            (
                Type::Map {
                    keys: k1,
                    values: v1,
                },
                Type::Map {
                    keys: k2,
                    values: v2,
                },
            ) => k1.compatible(k2) && v1.compatible(v2),
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
    fn arrays_compativeis_por_elemento_compativel() {
        let a1 = Type::Array {
            elem: Box::new(Type::Value),
        };
        let a2 = Type::Array {
            elem: Box::new(Type::Integer),
        };
        assert!(a1.compatible(&a2));

        let a3 = Type::Array {
            elem: Box::new(Type::String),
        };
        assert!(!a2.compatible(&a3));
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
