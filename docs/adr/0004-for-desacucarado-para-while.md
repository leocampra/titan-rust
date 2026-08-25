# ADR 0004 — `for` numérico desaçucarado para `while`, nunca `Range` do Rust

## Status

Aceito.

## Contexto

A Fase 1 precisa gerar Rust para o `for` numérico do Titan
(`for x = start, finish[, inc] do ... end`, `x` `integer` ou `float`). O
candidato óbvio seria mapear direto para um `for` do Rust sobre um `Range` ou
`(start..finish).step_by(inc)`, mas essa forma não cobre os casos que o Titan
original aceita:

- `Range<f64>` **não implementa** `Iterator` na stdlib do Rust — não existe
  `for x in 0.0..1.0` para `f64`.
- `.step_by` exige `usize` **positivo** — não aceita passo negativo
  (`for i = 5, 1, -1`) nem passo fracionário (`for x = 0.0, 1.0, 0.25`).
- `inc` pode ser uma expressão só conhecida em tempo de execução (não um
  literal), então nenhuma especialização baseada em "o passo é `1`?" pode ser
  decidida em tempo de compilação em geral.

Cobrir os quatro casos (integer/float × passo conhecido/desconhecido × ± ×
omitido) com formas diferentes de `Range`/`step_by` exigiria uma família de
templates de codegen, cada um com sua própria lógica de borda — exatamente o
tipo de ramificação que a Fase 0 tentou evitar isolando o mapeamento de tipos
numa função só (ver tabela em `docs/arquitetura.md`).

## Decisão

`StatFor` é sempre desaçucarado para um `while`, com um único template para
`integer` e `float`:

```rust
{
    let mut nome: T = start;
    let titan_for_finish: T = finish;
    let titan_for_inc: T = inc;
    let titan_for_asc: bool = titan_for_inc > 0 as T;
    while (titan_for_asc && nome <= titan_for_finish)
        || (!titan_for_asc && nome >= titan_for_finish) {
        // corpo
        nome += titan_for_inc;
    }
}
```

- `T` é `i64` ou `f64`, vindo de `TypedStat::For::ty` — o template é
  idêntico para os dois, não há ramo separado por tipo.
- A direção do laço (`titan_for_asc`) é computada **uma vez**, antes de
  entrar no `while`, comparando o sinal de `inc` — cobre `inc` positivo,
  negativo, ou só conhecido em runtime, sem casos especiais.
- O bloco externo (`{ ... }`) isola a variável de controle e as auxiliares
  (`titan_for_finish`, `titan_for_inc`, `titan_for_asc`) do escopo ao redor —
  nenhuma delas vaza para fora do laço, fiel à semântica do Titan original.
- O prefixo `titan_` nas auxiliares segue a convenção de mangling já usada em
  `mangle_fn_name`, evitando colisão com variáveis do programa do usuário.
- Sem caminho otimizado para o caso comum `inc = 1` literal — deliberado
  nesta fase (ver Consequências).

## Consequências

- Um único template no codegen cobre integer e float, `inc` omitido
  (default `1`/`1.0`), `inc` negativo e `inc` só conhecido em runtime — sem
  matriz de casos especiais para manter em sincronia.
- O Rust gerado para um `for` simples (`for i = 1, 5 do ... end`) é mais
  verboso do que a forma idiomática (`for i in 1..=5 { ... }`) — o laço
  desaçucarado sempre paga o custo de duas comparações e uma variável de
  direção por iteração, mesmo quando o compilador poderia provar em tempo de
  compilação que `inc = 1` e a direção é sempre ascendente.
- Fica registrado como **otimização futura**: detectar `inc` literal
  positivo/negativo (ou omitido) e emitir a forma idiomática com `Range`
  nesse subconjunto de casos, mantendo o template geral como fallback para
  `inc` dinâmico. Não implementado na Fase 1 — o ganho é só de legibilidade
  do Rust gerado (que já é artefato de depuração, não código de produção),
  não de corretude.
