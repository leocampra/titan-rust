# ADR 0023 — `continue` entra na linguagem

## Status

Aceito. **Supera o [ADR 0017](0017-break-sim-continue-nao.md)** ("`break` sim,
`continue` não"), cuja razão de ser desapareceu quando o
[ADR 0022](0022-for-como-loop-com-incremento-no-topo.md) moveu o incremento do
`for` para o topo do laço.

## Contexto

O ADR 0017 recusou `continue` por um motivo técnico preciso, não por gosto: o
`for` numérico era desaçucarado para um `while` com o incremento como **última
instrução do corpo** (ADR 0004), então um `continue` do usuário saltaria por
cima dele e voltaria ao teste de parada com a variável de controle intacta —
laço infinito silencioso, em tempo de execução, exatamente no laço mais comum
de um lexer. A rejeição ficava no parser, com uma mensagem que explicava esse
motivo, e o próprio ADR 0017 registrou a condição para ser revisto: *"se o
`for` deixar de ser desaçucarado para `while` num futuro redesenho, a razão de
ser desta decisão desaparece e `continue` pode ser reconsiderado"*.

Foi o que a T62 fez. O ADR 0022 emite o `for` como um `loop` do Rust com o
incremento no **topo**, guardado por um flag de primeira iteração:

```rust
loop {
    if titan_for_primeira { titan_for_primeira = false; } else { i += titan_for_inc; }
    if !(/* teste de parada */) { break; }
    // corpo — um `continue` aqui volta ao topo, e o incremento acontece
}
```

Todo caminho que volta ao topo do `loop` — a queda natural do fim do corpo ou
um `continue` explícito — passa pelo incremento. A premissa do ADR 0017 deixou
de ser verdade, e com ela a mensagem de erro que a citava.

## Decisão

`continue` entra na linguagem, com **exatamente** o mesmo desenho de `break`
(T55), em cada camada:

- `ast.rs`: nó `StatContinue { loc }` — o segundo nó de AST realmente novo do
  projeto, depois de `StatBreak`. O Titan original não tem nenhum dos dois.
- `parser.rs`: um braço em `parse_stat`, no lugar onde morava a rejeição. A
  mensagem do ADR 0017 sai do código: ela explicava um comportamento do `for`
  que não existe mais, e mantê-la seria documentar uma inverdade.
- `checker.rs`: variante `TypedStat::Continue` e a **mesma** checagem de
  `loop_depth` que `break` usa — `continue` fora de laço é erro claro em
  português (`` `continue` fora de um laço (`while`/`for`). ``), não panic.
- `codegen.rs`: emite literalmente `continue;`, **sem label**.

Não há laço auxiliar: a alternativa considerada na época do ADR 0017 — envolver
o corpo num `loop` interno do Rust e emitir `break` desse laço para o
`continue` do Titan — foi descartada pelo ADR 0022 junto com o template antigo.

No `while`, nada precisou ser feito: o `while` do Rust reavalia a condição ao
voltar ao topo, que é a semântica do `while` do Titan e do Lua. O incremento
ali é escrito pelo usuário, então um `continue` antes dele é um laço infinito
**do programa do usuário**, não do código gerado — a mesma armadilha que o Lua,
o C e o Rust têm, e que o compilador não tenta adivinhar.

## Consequências

- O idioma "ignora este item e siga para o próximo" passa a ser escrito
  diretamente. `examples/lexer.titan` (T56) o contornava com `if`/`else`
  aninhado por causa do ADR 0017; o arquivo continua correto como está, e
  reescrevê-lo não é parte desta decisão.
- `continue` já era palavra-chave desde a T59 (junto com as outras seis da
  fase), então **não há quebra de compatibilidade nova** aqui — ela foi paga
  lá, com os mesmos termos de `as` (T20), `import` (T34) e `break` (T55).
- A rejeição de `continue` deixa de ser caso de teste permanente e vira caso
  positivo com execução real: `continue` em `for` crescente, em `for`
  decrescente com passo negativo, em `while`, em laço aninhado (afeta só o
  laço mais interno) e convivendo com `break` no mesmo laço. O que resta de
  negativo é `continue` fora de laço, que mudou de camada — era erro de
  sintaxe, agora é erro de tipos, igual a `break`.
- `break` e `continue` funcionarão dentro de `repeat`/`until` (T64) sem caso
  especial: ambos emitem a instrução do Rust sem label, e `repeat` também é
  emitido como um `loop`.
