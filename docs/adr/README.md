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
