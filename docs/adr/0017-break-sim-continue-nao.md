# ADR 0017 — `break` sim, `continue` não

## Status

Aceito.

## Contexto

Um lexer (T56) precisa pular iterações — o idioma mais comum é `continue`
para "ignora este caractere, siga para o próximo". Mas o `for` numérico desta
linguagem é desaçucarado para `while` em tempo de codegen, com o incremento
**no fim do corpo** ([ADR 0004](0004-for-desacucarado-para-while.md)):

```rust
// for i = 1, n do <corpo> end  vira:
let mut i = 1;
while i <= n {
    <corpo>
    i += 1; // incremento no fim
}
```

Um `continue` dentro de `<corpo>` mapeado para o `continue` do Rust pularia
esse incremento — o laço nunca avança e trava. Isso não é um erro de
compilação: é um bug silencioso, em tempo de execução, precisamente no laço
mais comum de um lexer (`while pos <= tamanho do ... end`).

`break`, ao contrário, não tem esse problema: mapeia direto para o `break` do
Rust em qualquer um dos dois laços, sem depender de onde o incremento mora no
corpo desaçucarado.

## Decisão

`break` entra na linguagem (T55): nova keyword (`lexer.rs`), novo nó de AST
`StatBreak` (`ast.rs`; o primeiro nó realmente novo do projeto — nas fases
anteriores `ast.rs` já vinha completa e a tarefa era ensinar parser/checker/
codegen a usá-la), braço em `parse_stat` (`parser.rs`), variante
`TypedStat::Break` no checker com rejeição de `break` fora de laço (rastreando
profundidade de `while`/`for` aninhados no `Checker`), e emissão de `break;`
no codegen.

`continue` **não** entra. É rejeitado explicitamente pelo parser
(`parser.rs:427`) com uma mensagem que explica o motivo — pularia o
incremento do `for` desaçucarado — em vez de cair no erro genérico de
"declaração inesperada".

## Consequências

- `examples/lexer.titan` (T56) usa `break` para sair do laço de varredura
  quando o caractere atual não casa com nenhum ramo; o padrão "ignora e siga"
  que normalmente pediria `continue` é reescrito como `if`/`else` aninhado.
- Quebra de compatibilidade: `break` deixa de poder ser usado como
  identificador (nome de variável, função, campo). Mesmo tipo de mudança que
  `as` (T20) e `import` (T34) — registrada, não é a primeira vez.
- A rejeição de `continue` é um caso de teste permanente em
  `integration.rs` (T57): garante que a ausência é uma decisão de design
  testada, não um esquecimento que algum dia "some" quando alguém adicionar
  `continue` sem revisitar o desaçucaramento do `for`.
- Se o `for` deixar de ser desaçucarado para `while` num futuro redesenho
  (por exemplo, gerando um `for` nativo do Rust com `Range`), a razão de ser
  desta decisão desaparece e `continue` pode ser reconsiderado — mas isso
  reabriria [ADR 0004](0004-for-desacucarado-para-while.md), fora do escopo
  desta fase.
