# ADR 0005 — `and`/`or` boolean estrito, divergindo do truthy/falsy do original

## Status

Aceito.

## Contexto

No Titan original (e em Lua), `and`/`or` não exigem operandos booleanos: são
operadores de curto-circuito sobre **qualquer** valor, seguindo a noção de
truthy/falsy da linguagem (`nil` e `false` são falsy, todo o resto —
incluindo `0` e `""` — é truthy). `a and b` retorna `a` se `a` for falsy,
senão `b`; `a or b` retorna `a` se `a` for truthy, senão `b`. O tipo de
resultado depende dos tipos de `a` e `b`, não é necessariamente `Boolean`.

Essa semântica depende da família de tipos `Value`/`Option` do Titan original
para representar "um valor que pode ser de tipos diferentes dependendo do
caminho" — e a Fase 1 não usa `Value`/`Option` (fora de escopo até a Fase 2).
Sem esses tipos, replicar o truthy/falsy exigiria ou introduzi-los
prematuramente, ou dar a `and`/`or` um tipo de retorno que já não existe no
sistema de tipos atual.

## Decisão

`and`/`or` na Fase 1 exigem **os dois lados `Boolean`**, resultado sempre
`Boolean`, mapeando diretamente para `&&`/`||` do Rust. Isso é uma
**divergência deliberada** do comportamento do Titan original, e não uma
omissão — está registrada aqui e como comentário no código do checker
(`checker.rs`, regras de `BinOp::And`/`BinOp::Or`).

`1 and 2` — válido em Lua/Titan (retorna `2`), erro claro no `titan-rust`
Fase 1 (operandos não são `Boolean`), coberto por teste em T16.

## Consequências

- O checker rejeita com erro claro (nunca panic) qualquer `and`/`or` cujos
  operandos não sejam ambos `Boolean` — inclusive os idiomas comuns em Lua
  como `x and x.campo` (guard) ou `a or default` (valor-padrão), que não têm
  equivalente direto nesta fase.
- O mapeamento para `&&`/`||` do Rust é direto e sem coerção — mais simples
  de auditar do que reproduzir curto-circuito com tipo de retorno variável.
- Quando `Value`/`Option` entrarem no sistema de tipos (fora de escopo até
  pelo menos a Fase 2), essa decisão deve ser revisitada: nesse ponto haverá
  um tipo capaz de representar "`a` ou `b`, o que for truthy", e a
  divergência registrada aqui deixa de ser uma limitação técnica para virar
  uma escolha de design a ser reconfirmada.
