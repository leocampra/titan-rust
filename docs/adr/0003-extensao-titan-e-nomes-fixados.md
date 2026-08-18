# ADR 0003 — Extensão `.titan`, binário `titanc`, runtime `titan-runtime`

## Status

Aceito.

## Contexto

Precisava-se fixar, logo no início do projeto (T0), um conjunto de nomes
externos e observáveis: a extensão de arquivo-fonte, o nome do binário
compilador, e o nome do crate de runtime. Esses nomes aparecem em toda
interação com o projeto — CLI, mensagens de erro, `Cargo.toml` gerado — e
trocá-los depois exigiria revisar exemplos, testes de integração e
documentação já publicados.

## Decisão

- Extensão de arquivo-fonte: **`.titan`** (mesma do projeto original — o
  `titan-rust` não é uma linguagem nova do zero, é uma nova implementação da
  mesma linguagem-conceito).
- Binário do compilador: **`titanc`** (mesmo nome do `titanc` original em
  `titan/titanc`, que é um script Lua — o nosso é um binário Rust
  independente; nunca são instalados no mesmo PATH).
- Crate de runtime: **`titan-runtime`**, referenciado por caminho absoluto no
  `Cargo.toml` de cada projeto gerado (ver
  [`docs/arquitetura.md`](../arquitetura.md), seção "armadilhas do Cargo").
- Redox OS, cotado no planejamento inicial (`plano.md`) como plataforma-alvo,
  fica **fora de escopo** nesta fase — o alvo é Linux nativo via `cargo`
  comum. Nada na arquitetura impede um `--target` Redox depois, já que quem
  compila de fato é sempre o `cargo`.

## Consequências

- Um usuário familiarizado com o Titan original reconhece a extensão e o nome
  do binário; a familiaridade não se estende ao comportamento do compilador
  em si (que é uma implementação nova — ver
  [ADR 0001](0001-compilador-novo-em-rust.md)).
- `titanc` (Rust) e `titan/titanc` (Lua) coexistem no mesmo repositório sem
  colidir, porque nenhum dos dois é instalado globalmente — ambos são
  invocados por caminho explícito.
- Adiar a decisão de plataforma-alvo (Redox vs. Linux) para depois da Fase 0
  evita acoplar o desenho do pipeline a detalhes de um SO ainda não
  necessário; o ponto de extensão natural é a invocação do `cargo` em
  `driver.rs`.
