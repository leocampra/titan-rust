# ADR 0008 — Indexação checada no runtime (`T`, não `T?`) e variância invariante

## Status

Aceito.

## Contexto

Duas decisões de tipos, relacionadas por tocarem a mesma superfície
(indexação de `array`/`map`), foram fixadas juntas:

**1. Tipo de `v[i]`.** O Titan original tipa a indexação de array como `T?`
(`Option<T>`) — uma leitura fora da faixa retorna um valor ausente que o
programa precisa desembrulhar. Replicar isso exigiria `Option`/`?` no sistema
de tipos, que está fora de escopo até uma fase futura (decisão 2 da Fase 2).
A alternativa de tipar `v[i]` como `T` puro só é sound se toda leitura/escrita
fora da faixa for impedida antes de produzir um `T` — ou seja, checagem em
runtime, com abort claro em vez de devolver um valor inventado.

**2. Variância de `compatible` em `Array`/`Map`.** Antes da Fase 2,
`compatible` (`types.rs`) era **covariante** em `Array`: `{value}` aceitava
`{integer}`. Isso era sound enquanto arrays eram imutáveis por fora do
checker. Com parâmetros compostos passados por `&mut`
([ADR 0007](0007-parametros-compostos-por-mut.md)), covariância deixa de ser
sound da forma clássica: se uma função recebe `xs: {value}` e por trás a
referência aponta para um `{integer}` do chamador, escrever uma `string`
através de `xs[i] = "oi"` corromperia um array que o chamador ainda enxerga
como `{integer}` — exatamente o buraco de tipos que arrays mutáveis
covariantes sempre abrem (o mesmo problema que `Object[]`/`Array` covariante
tem em Java e TypeScript).

## Decisão

- **`v[i]` tem tipo `T`**, não `T?`. Toda leitura e escrita passa pelas
  funções checadas do `titan-runtime` (`array_get`, `array_get_mut`,
  `array_set`, `map_get`), que convertem o índice 1-based do Titan para
  0-based internamente e abortam com `eprintln!` em português +
  `std::process::exit(1)` — nunca um `panic!` cru do Rust — quando o índice é
  inválido. Mensagens cobrem os casos: índice `0` (`"arrays em Titan começam
  em 1"`), índice negativo ou além do fim (`"índice N fora da faixa (array
  tem M elementos)"`), e o caso especial de escrita em `#v + 1`, que faz
  **append** em vez de abortar (decisão 5 da Fase 2).
- **`Array` e `Map` passam a ser invariantes** em `compatible`
  (`types.rs:80`): o braço que antes recursava em `compatible` passa a
  recursar em `equals`. `{value}` não aceita `{integer}` nem o inverso;
  `{integer}` só aceita `{integer}`. `Value` continua compatível com
  qualquer coisa **no topo** — `f(x: value)` aceita um `{integer}` inteiro
  como argumento —, mas a invariância vale **dentro** do composto.
  `Record` (nominal) e `Option` também ganham braço explícito e invariante
  em vez de caírem no `_ => false` implícito, deixando a intenção escrita.

## Consequências

- Nenhum valor "ausente" ou placeholder é inventado para índice inválido —
  o programa aborta com mensagem clara, nunca segue executando com um `T`
  que não existe de verdade. Trade-off aceito: um programa Titan que
  dependesse de capturar o `nil` de uma leitura fora da faixa (padrão comum
  com `T?` no original) precisa ser reescrito; esse padrão está fora de
  escopo até `Option`/`?` entrarem no sistema de tipos.
- A checagem de faixa é testável sem subprocesso: cada função abortante do
  runtime tem uma variante `*_checked -> Result<_, String>` por trás, e a
  versão que aborta é só um wrapper fino.
- A invariância fecha o buraco de soundness descrito acima, ao custo de
  recusar alguns programas que o Titan original aceitaria (qualquer um que
  dependesse de covariância de array/map). Nenhum caso de uso real da Fase 2
  foi encontrado que precisasse de covariância aqui.
- Revisitar quando `Option`/`?` ou genéricos/variância explícita entrarem no
  sistema de tipos — neste ponto pode fazer sentido reintroduzir
  covariância apenas para posições somente-leitura, algo que o sistema de
  tipos atual não distingue.
