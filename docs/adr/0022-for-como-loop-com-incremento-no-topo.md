# ADR 0022 — `for` numérico como `loop` com o incremento no topo

## Status

Aceito. **Supera o [ADR 0004](0004-for-desacucarado-para-while.md)**, que
mantinha o mesmo `for` desaçucarado para `while` com o incremento no fim do
corpo.

## Contexto

O ADR 0004 fixou o desaçucaramento do `for` numérico para um `while` com o
incremento como **última instrução do corpo**. A forma cobria corretamente
todos os casos de `start`/`finish`/`inc` (integer/float, passo positivo,
negativo ou só conhecido em runtime), e continua valendo tudo o que aquele ADR
disse sobre **não** usar `Range`/`step_by` do Rust — as razões não mudaram.

O que mudou é o escopo da linguagem. O ADR 0017 ("`break` sim, `continue` não")
recusou o `continue` justamente por causa desse template: um `continue` do
usuário dentro de um `for` saltaria por cima do incremento e voltaria ao teste
de parada com a variável de controle intacta — laço infinito. A alternativa
considerada na época era envolver o corpo num `loop` interno do Rust e emitir
`break` desse `loop` interno para o `continue` do Titan. Isso funciona, mas
adiciona um nível de laço que não existe no programa do usuário, complica a
interação com `break` (que precisaria de label) e deixa o Rust gerado ilegível.

A Fase 5 inclui `continue` no escopo da linguagem. A decisão foi atacar a
causa em vez de contorná-la: se a posição do incremento é o impeditivo, é a
posição do incremento que muda.

## Decisão

`StatFor` é emitido como um `loop` do Rust com o incremento no **topo**,
guardado por um flag de primeira iteração, e o teste de parada logo depois:

```rust
{
    let mut nome: T = start;
    let titan_for_finish: T = finish;
    let titan_for_inc: T = inc;
    let titan_for_asc: bool = titan_for_inc > 0 as T;
    let mut titan_for_primeira: bool = true;
    loop {
        if titan_for_primeira {
            titan_for_primeira = false;
        } else {
            nome += titan_for_inc;
        }
        if !((titan_for_asc && nome <= titan_for_finish)
            || (!titan_for_asc && nome >= titan_for_finish)) {
            break;
        }
        // corpo — um `continue` aqui volta ao topo, e o incremento acontece
    }
}
```

Tudo o que o ADR 0004 fixou sobre a forma **permanece**:

- `T` é `i64` ou `f64`, vindo de `TypedStat::For::ty` — template único, sem
  ramo por tipo.
- `titan_for_asc` é computado **uma vez**, antes de entrar no laço, cobrindo
  `inc` positivo, negativo ou dinâmico sem casos especiais.
- O bloco externo isola a variável de controle e as auxiliares do escopo ao
  redor; laços aninhados apenas sombreiam as auxiliares do laço externo.
- O prefixo `titan_` segue a convenção de `mangle_fn_name`.
- Sem caminho otimizado para `inc = 1` literal — segue registrado como
  otimização futura.

O único ponto novo é o flag `titan_for_primeira`: é ele que permite ter o
incremento textualmente antes do corpo sem alterar o valor visto na primeira
iteração.

## Consequências

- **`continue` passa a ser implementável sem laço auxiliar** (PRD T63): todo
  caminho que volta ao topo do `loop` — a queda natural do fim do corpo ou um
  `continue` explícito — passa pelo incremento. O `continue` do Titan emite
  literalmente `continue;` em Rust, e `break` segue emitindo `break;`, ambos
  sem label. O ADR 0017 perde seu fundamento técnico, e é revisto na T63 —
  junto com a implementação do `continue`, não aqui.
- O laço paga um teste de booleano por iteração a mais que a forma do ADR 0004.
  É o preço do `continue`; o predicador de ramificação da CPU acerta esse teste
  em todas as iterações menos a primeira, e o Rust gerado é artefato de
  depuração, não código de produção otimizado à mão.
- O teste de parada aparece negado (`if !(... ) { break; }`) em vez de como
  condição de continuação do `while`. É a mesma expressão do ADR 0004, e
  mantê-la assim — em vez de distribuir a negação — preserva a correspondência
  linha a linha com o template antigo.
- Risco assumido e coberto por teste: a mudança de template podia regredir
  silenciosamente qualquer um dos casos de borda do `for`. Os testes da T15
  (incluindo `for i = 1, 0` → zero iterações, que é o caso que o flag de
  primeira iteração poderia ter quebrado) foram rodados verdes **antes** da
  troca e passam sem alteração depois dela.
