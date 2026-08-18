# ADR 0002 — `print` vem do runtime, não é palavra-chave

## Status

Aceito.

## Contexto

O Titan original **não tem `print`** — não existe no lexer, no parser nem no
checker de `titan/titan-compiler/`. Para compilar e rodar
`examples/hello.titan`, a Fase 0 precisa de alguma forma de escrever na saída
padrão.

Duas opções: (a) adicionar `print` como palavra-chave/forma especial da
linguagem, tratada em algum ponto do pipeline (lexer, parser ou checker); ou
(b) tratá-la como uma função comum, com assinatura conhecida, que vem de uma
biblioteca externa ao compilador.

## Decisão

`print` é uma função comum da stdlib, implementada em
`crates/titan-runtime/src/lib.rs` (`pub fn print(s: &str)`) e registrada pelo
**checker** no escopo global como
`Type::Function { params: [String], rettypes: [Nil] }`
(`crates/titanc/src/checker.rs`, `Checker::new`) — não é um token do lexer,
não é uma produção do parser. Do ponto de vista da gramática, `print("x")` é
uma `ExpCall` como qualquer outra chamada de função top-level.

O mesmo raciocínio vale para `concat`, que dá suporte ao operador `..`: é
runtime, não geração de código ad-hoc por operador.

## Consequências

- O parser não precisa de caso especial nenhum para `print` — o mesmo caminho
  de `ExpCall`/`StatCall` que trata qualquer chamada de usuário também trata
  `print`.
- `codegen.rs` mapeia o nome `print` para `titan_runtime::print(...)` só na
  hora de emitir a chamada (`emit_call`), análogo a como mapearia qualquer
  outra função de uma biblioteca externa — não há um nó de AST dedicado a
  `print`.
- Adicionar novas funções de stdlib na Fase 1+ segue o mesmo padrão: função
  Rust em `titan-runtime`, entrada no escopo global do checker, sem tocar
  lexer/parser.
- Diverge deliberadamente do Titan original, que não tem `print` nem esse
  conceito de runtime vinculado ao checker — é uma decisão nova desta
  implementação, não herdada.
