# ADR 0021 — Bitwise exige `integer` estrito, sem coagir `float`

## Status

Aceito.

## Contexto

O Titan original coage silenciosamente `float` para `integer` nos operadores
bitwise. Em `checker.lua:1097-1109` (`|`, `&`, `<<`, `>>`) e em
`checker.lua:870-881` (`~` unário), o lado que chega como `Type.Float`
passa por `trycoerce(..., types.Integer())` antes da checagem — só depois é
que um operando ainda não-inteiro vira erro. Na prática, `1.5 & 2` é aceito
e o `1.5` vira `1`.

Esse é o comportamento herdado do Lua, onde a conversão float→integer só
falha quando o valor tem parte fracionária **em tempo de execução**. Num
compilador estaticamente tipado, reproduzi-lo significaria ou truncar em
silêncio na emissão, ou gerar uma checagem de runtime para cada operando
bitwise que nasce `float`.

Há ainda uma terceira saída, pior: delegar ao rustc. O `&` do Rust não existe
para `f64`, então o Rust gerado simplesmente não compilaria — e o erro
chegaria ao usuário **em inglês, sobre código que ele não escreveu**,
violando a convenção mais antiga do projeto (toda mensagem do compilador em
português, apontando o fonte Titan).

## Decisão

Os operadores bitwise — `&`, `|`, `~` binário (XOR), `<<`, `>>` e `~` unário
(NOT) — exigem **`integer` dos dois lados**, sem nenhuma coerção, e resultam
sempre `integer`. `1.5 & 2` é erro de tipo do checker, em português,
apontando o operando culpado:

```
operando de `&` precisa ser integer, encontrado float.
```

Isso é uma **divergência deliberada** do original, na mesma família do ADR
0005 (`and`/`or` boolean estrito): onde o Titan original aceita por coerção,
o `titan-rust` prefere o erro explícito.

A divergência **não** se estende a `//`, que segue a regra aritmética comum
de `+ - * %` (`checker.lua:988`): `integer // integer` dá `integer`, e
qualquer `float` promove os dois lados e resulta `float`.

A decisão é sobre **tipos**, e não afrouxa a semântica de execução: onde o
operador do Rust diverge do Titan, a conta vai para o runtime em vez de sair
crua. É o caso de `//` (`titan_runtime::idiv` — o `/` do Rust trunca, o `//`
do Titan arredonda para baixo) e dos deslocamentos (`titan_runtime::shl`/
`shr` — o `<<` do Rust exige deslocamento em `0..64`, o do Titan aceita
qualquer inteiro: negativo inverte a direção, 64 ou mais zera). No caso dos
deslocamentos isso não é só semântica: `1 << 64` faz o **rustc recusar a
compilação**, e o erro chegaria em inglês sobre código gerado.

## Consequências

- Quem realmente quer truncar escreve o truncamento — o cast `as` (T71) torna
  a intenção visível no fonte, em vez de escondê-la numa regra de coerção.
- A emissão fica trivial e auditável para `&`, `|`, `~` e `~` unário: o
  checker garantiu `i64` dos dois lados, então `&`, `|`, `^` e `!` do Rust
  saem sem um único cast intermediário. Os deslocamentos são a exceção, e
  pela semântica, não pelo tipo: saem por chamada ao runtime.
- Programas Titan originais que dependam da coerção deixam de compilar aqui,
  com erro claro apontando a linha — não silenciosamente com outro resultado.
- Se um dia `titan-rust` precisar aceitar o idioma original sem alterações,
  esta decisão é o ponto de revisão: basta inserir a coerção no mesmo lugar
  onde hoje o erro é emitido (`check_integer_operands`, em `checker.rs`).
