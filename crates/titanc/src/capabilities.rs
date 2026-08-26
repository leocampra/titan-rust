//! Tabela de capabilities (PRD.md, T37).
//!
//! `builtins.rs` é a fonte única de verdade para `print`, mas tem dois
//! limites que a Fase 3 encosta: `titan_name` é plano (sem namespace) e
//! `rettype` é escalar (um retorno só) — o bastante para uma função solta,
//! não para um módulo com tipos opacos e métodos. Este arquivo é a fonte
//! única de verdade equivalente para *módulos* (`import data`): checker
//! (popular o símbolo do módulo e resolver `data.DataFrame`/membros),
//! codegen (caminho Rust da função/método) e driver (dependência no
//! `Cargo.toml` gerado) consultam [`CAPABILITIES`]/[`lookup_module`] em vez
//! de conhecer módulos de forma independente.
//!
//! `BUILTINS` (`builtins.rs`) continua existindo sem mudança de
//! comportamento — `print` não é membro de nenhum módulo.

use crate::types::Type;

/// Um tipo opaco exportado por um módulo (`data.DataFrame`): o programa
/// carrega e passa adiante, mas não inspeciona os campos.
pub struct OpaqueType {
    /// Nome do tipo como o programa Titan escreve (`DataFrame` em
    /// `data.DataFrame`).
    pub titan_name: &'static str,
    /// Caminho Rust totalmente qualificado do tipo (`titan_data::DataFrame`).
    pub rust_path: &'static str,
}

/// Uma função de módulo (`data.read_csv(...)`) ou método sobre um tipo
/// opaco do módulo (`df.soma(...)`) — mesmo par `params`/`rettype`/
/// `rust_path` de [`crate::builtins::Builtin`], com o nome do receptor a
/// mais quando é método.
pub struct CapabilityFn {
    /// Nome pelo qual o programa Titan chama a função/método (`read_csv`,
    /// `soma`).
    pub titan_name: &'static str,
    /// Para um método, o nome do tipo opaco (`OpaqueType::titan_name`) sobre
    /// o qual ele é chamado. `None` para função de módulo.
    pub receiver: Option<&'static str>,
    /// Caminho Rust totalmente qualificado que o codegen emite
    /// (`titan_data::read_csv`).
    pub rust_path: &'static str,
    pub params: &'static [Type],
    pub rettype: Type,
}

/// Um módulo de capability (`import data`): nome Titan, nome/caminho do
/// crate (para o `Cargo.toml` gerado pelo driver), tipos opacos exportados
/// e as funções/métodos do módulo.
pub struct Capability {
    /// Nome pelo qual o programa Titan importa o módulo (`data` em
    /// `import data`).
    pub titan_name: &'static str,
    /// Nome do crate Rust que implementa o módulo (`titan-data`).
    pub crate_name: &'static str,
    /// Caminho do crate relativo à raiz do workspace, para o driver montar
    /// a dependência no `Cargo.toml` gerado (`crates/titan-data`).
    pub crate_path: &'static str,
    pub opaque_types: &'static [OpaqueType],
    pub functions: &'static [CapabilityFn],
}

impl Capability {
    /// Busca um tipo opaco do módulo pelo nome Titan (`DataFrame` em
    /// `data.DataFrame`).
    pub fn find_opaque(&self, name: &str) -> Option<&'static OpaqueType> {
        self.opaque_types.iter().find(|t| t.titan_name == name)
    }

    /// Busca uma função **de módulo** (sem receptor) pelo nome
    /// (`data.read_csv`).
    pub fn find_function(&self, name: &str) -> Option<&'static CapabilityFn> {
        self.functions
            .iter()
            .find(|f| f.receiver.is_none() && f.titan_name == name)
    }

    /// Busca um **método** (com receptor do tipo opaco dado) pelo nome
    /// (`df.soma`, onde `df: data.DataFrame`).
    pub fn find_method(&self, receiver_type: &str, name: &str) -> Option<&'static CapabilityFn> {
        self.functions
            .iter()
            .find(|f| f.receiver == Some(receiver_type) && f.titan_name == name)
    }
}

/// Módulo `data`, com um único tipo opaco `DataFrame` — o suficiente para o
/// checker (T38) resolver `import data` e `data.DataFrame`. T41 é quem
/// acrescenta o crate real `titan-data` por trás deste stub.
const DATA_OPAQUE_TYPES: &[OpaqueType] = &[OpaqueType {
    titan_name: "DataFrame",
    rust_path: "titan_data::DataFrame",
}];

/// `read_csv` (função de módulo) e `soma` (método sobre `DataFrame`, T40) —
/// o restante da superfície (`media`, `minimo`, `maximo`, etc.) fica para a
/// T41 trazer o crate `titan-data` de verdade por trás do stub. `rettype` de
/// um `Opaque` fica com `module`/`name`/`rust_path` vazios porque uma
/// declaração `const` não constrói `String` não-vazia; o checker
/// (`requalify_rettype`, T39) preenche o placeholder com o módulo real da
/// chamada antes de devolvê-lo — mesmo espírito do `FAKE_FUNCTIONS` de teste
/// abaixo. `soma` devolve sempre `float` (decisão 9 do PRD.md, T41).
const DATA_FUNCTIONS: &[CapabilityFn] = &[
    CapabilityFn {
        titan_name: "read_csv",
        receiver: None,
        rust_path: "titan_data::read_csv",
        params: &[Type::String],
        rettype: Type::Opaque {
            module: String::new(),
            name: String::new(),
            rust_path: String::new(),
        },
    },
    CapabilityFn {
        titan_name: "soma",
        receiver: Some("DataFrame"),
        rust_path: "titan_data::soma",
        params: &[Type::String],
        rettype: Type::Float,
    },
];

pub const CAPABILITIES: &[Capability] = &[Capability {
    titan_name: "data",
    crate_name: "titan-data",
    crate_path: "crates/titan-data",
    opaque_types: DATA_OPAQUE_TYPES,
    functions: DATA_FUNCTIONS,
}];

/// Busca um módulo de capability pelo nome usado em `import`.
pub fn lookup_module(titan_name: &str) -> Option<&'static Capability> {
    CAPABILITIES.iter().find(|c| c.titan_name == titan_name)
}

/// Lista os nomes de todos os módulos disponíveis, na ordem da tabela — usada
/// para compor a mensagem de erro de capability inexistente ("disponíveis:
/// ...").
pub fn available_module_names() -> Vec<&'static str> {
    CAPABILITIES.iter().map(|c| c.titan_name).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const FAKE_OPAQUE: OpaqueType = OpaqueType {
        titan_name: "DataFrame",
        rust_path: "titan_data::DataFrame",
    };

    const FAKE_FUNCTIONS: &[CapabilityFn] = &[
        CapabilityFn {
            titan_name: "read_csv",
            receiver: None,
            rust_path: "titan_data::read_csv",
            params: &[Type::String],
            rettype: Type::Opaque {
                module: String::new(),
                name: String::new(),
                rust_path: String::new(),
            },
        },
        CapabilityFn {
            titan_name: "soma",
            receiver: Some("DataFrame"),
            rust_path: "titan_data::soma",
            params: &[Type::String],
            rettype: Type::Float,
        },
    ];

    const FAKE_CAPABILITY: Capability = Capability {
        titan_name: "data",
        crate_name: "titan-data",
        crate_path: "crates/titan-data",
        opaque_types: std::slice::from_ref(&FAKE_OPAQUE),
        functions: FAKE_FUNCTIONS,
    };

    #[test]
    fn encontra_tipo_opaco_por_nome() {
        assert!(FAKE_CAPABILITY.find_opaque("DataFrame").is_some());
        assert!(FAKE_CAPABILITY.find_opaque("Series").is_none());
    }

    #[test]
    fn encontra_funcao_de_modulo_por_nome() {
        let f = FAKE_CAPABILITY
            .find_function("read_csv")
            .expect("read_csv deveria existir");
        assert_eq!(f.rust_path, "titan_data::read_csv");
        // `soma` é método, não função de módulo — não deve aparecer aqui.
        assert!(FAKE_CAPABILITY.find_function("soma").is_none());
    }

    #[test]
    fn encontra_metodo_por_tipo_receptor_e_nome() {
        let m = FAKE_CAPABILITY
            .find_method("DataFrame", "soma")
            .expect("soma deveria existir para DataFrame");
        assert_eq!(m.rust_path, "titan_data::soma");
        // Tipo receptor errado não encontra o método.
        assert!(FAKE_CAPABILITY.find_method("Series", "soma").is_none());
        // `read_csv` é função de módulo, não método.
        assert!(
            FAKE_CAPABILITY
                .find_method("DataFrame", "read_csv")
                .is_none()
        );
    }

    #[test]
    fn lookup_module_por_nome_titan() {
        // `data` é o stub real que o checker (T38) já enxerga; T41 é quem
        // acrescenta o crate de verdade por trás dele.
        assert!(lookup_module("data").is_some());
        assert!(lookup_module("inexistente").is_none());
    }

    #[test]
    fn capability_inexistente_reporta_lista_de_disponiveis() {
        // Mesma função de listagem que o checker (T38) usa para montar a
        // mensagem de erro "capability 'foo' não existe; disponíveis: ...".
        assert_eq!(available_module_names(), vec!["data"]);
    }

    #[test]
    fn membro_inexistente_no_modulo_fake_e_reportado() {
        assert!(FAKE_CAPABILITY.find_function("inexistente").is_none());
        assert!(
            FAKE_CAPABILITY
                .find_method("DataFrame", "inexistente")
                .is_none()
        );
    }
}
