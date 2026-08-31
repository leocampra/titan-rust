# ADR 0018 — `titanc` exposto como lib: o LSP reusa o pipeline sem invocar o `cargo`

## Status

Aceito.

## Contexto

Até o fim da Fase 3, `titanc` era só um binário: `main.rs` declarava
`lexer`, `parser`, `checker`, `codegen`, `ast`, `types`, `capabilities`,
`builtins` e `driver` como `mod` privados (`main.rs:11-23` antes da T47).
Nada fora desse binário podia chamar `lex`/`parse`/`check` diretamente.

Construir um LSP (T48) exige exatamente isso: rodar `lex → parse → check`
sobre o buffer do editor, em memória, a cada `didOpen`/`didChange`, sem gerar
projeto Cargo nem invocar `cargo build` — o que tornaria o diagnóstico lento
demais para ser útil interativamente, e é justamente o oposto do que um LSP
deve fazer. As quatro etapas já eram funções puras (`lex(&str) -> Vec<Token>`,
`parse(&[Token]) -> Program`, `check(&Program) -> TypedProgram`,
`generate(&TypedProgram) -> String`) — faltava só uma fronteira de biblioteca
para reusá-las.

## Decisão

`crates/titanc/src/lib.rs` (T47) move a declaração de módulos de `main.rs`
para lá, tornando-os públicos. `main.rs` vira um bin fino sobre a lib
(`use titanc::driver::{self, Options}`), sem mudança de comportamento — CLI e
chamada a `driver::compile` seguem idênticas. `crates/titan-lsp` (T48) depende
de `titanc` como biblioteca (`titanc = { path = "../titanc" }`,
`crates/titan-lsp/Cargo.toml`) e chama `titanc::lexer::lex`,
`titanc::parser::parse`, `titanc::checker::check` diretamente sobre o texto do
buffer, parando antes de `driver::compile` — nunca escreve `build/<nome>/`,
nunca invoca `cargo` (decisão 6 do PRD.md, Fase 4).

Os `#[allow(dead_code)]` que existiam em `main.rs:11-23` foram revisados na
T47: o que era código morto num binário passou a ser API pública de uma lib,
e a maioria saiu — o que sobrou tem justificativa própria.

## Consequências

- O LSP é o **segundo consumidor** do pipeline (`docs/arquitetura.md`), ao
  lado do binário `titanc`. Qualquer mudança futura em `lex`/`parse`/`check`
  precisa considerar os dois consumidores — em particular, o LSP depende de
  `checker::check` continuar devolvendo `Vec<CheckError>` (todos os erros de
  uma vez) para publicar diagnósticos completos numa passada só.
- `driver::compile` continua sendo o único ponto que toca disco (fora do
  arquivo de entrada) e invoca `cargo` — o LSP nunca herda esse custo, e o
  binário `titanc` nunca precisa saber que existe um LSP.
- A lib expõe mais do que o LSP usa hoje (`builtins`, `capabilities`,
  `types`) porque a fronteira é o pipeline inteiro, não uma API sob medida
  para T48 — T50 (autocomplete) já consome `capabilities::find_function`/
  `find_method` e `builtins::BUILTINS` diretamente, sem exigir mudança na
  lib.
- Regressão coberta por teste: T47 exige saída byte-a-byte idêntica de
  `titanc` em `hello`, `nucleo`, `compostos` e `dados` antes/depois da
  extração — a lib não pode mudar o comportamento do binário.
