# Registro de decisões de arquitetura (ADRs)

| ADR | Decisão |
|---|---|
| [0001](0001-compilador-novo-em-rust.md) | Compilador novo, escrito em Rust, do zero — Titan original só como referência |
| [0002](0002-print-via-runtime.md) | `print` vem do runtime, não é palavra-chave |
| [0003](0003-extensao-titan-e-nomes-fixados.md) | Extensão `.titan`, binário `titanc`, runtime `titan-runtime` |
| [0004](0004-for-desacucarado-para-while.md) | `for` numérico desaçucarado para `while`, nunca `Range` do Rust |
| [0005](0005-and-or-boolean-estrito.md) | `and`/`or` boolean estrito, divergindo do truthy/falsy do original |
| [0006](0006-semantica-de-valor-clone-na-atribuicao.md) | Semântica de valor com `clone()` na atribuição de compostos (diverge do aliasing do original) |
| [0007](0007-parametros-compostos-por-mut.md) | Parâmetros compostos passados por `&mut`, preservando o idioma in-place |
| [0008](0008-indexacao-checada-e-variancia-invariante.md) | Indexação checada no runtime (`T`, não `T?`); `Array`/`Map` invariantes em `compatible` |
| [0009](0009-records-como-struct-rust-nominal.md) | Records como `struct` Rust nominal |
| [0010](0010-string-sempre-string.md) | `string` é sempre `String` — fim da dualidade `&str`/`String` |
| [0011](0011-import-como-acucar-sintatico.md) | `import data` como declaração de topo, `data` como nome fixo (diverge do original) |
| [0012](0012-modulo-como-symbolkind-nao-tipo.md) | Módulo é `SymbolKind`, não `Type` (diverge do `Type.Module` do original) |
| [0013](0013-tipo-opaco-composto-por-heranca.md) | Tipo opaco `Type::Opaque`, composto por herança (exige `Clone`) |
| [0014](0014-metodo-com-ponto-nao-dois-pontos.md) | Método chamado com `.`, não `:` (diverge do original) |
| [0015](0015-api-data-como-contrato-backend-trocavel.md) | API `data.*` é o contrato; o backend (Polars) é detalhe interno trocável |
| [0016](0016-acesso-a-texto-por-capability.md) | Acesso a texto por capability `texto`, não builtins globais nem `s[i]` |
| [0017](0017-break-sim-continue-nao.md) | `break` sim, `continue` não (o `for` desaçucarado perderia o incremento) |
| [0018](0018-titanc-lib-lsp-reusa-pipeline.md) | `titanc` exposto como lib: o LSP reusa o pipeline sem invocar o `cargo` |
| [0019](0019-lsp-tower-lsp-deps-isoladas.md) | LSP sobre `tower-lsp`; deps do servidor nunca entram no `Cargo.toml` gerado |
| [0020](0020-self-hosting-por-etapas.md) | Self-hosting por etapas: lexer na Fase 4, parser/checker na Fase 5 |
