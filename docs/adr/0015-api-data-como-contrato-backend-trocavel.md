# ADR 0015 — API `data.*` é o contrato; o backend é detalhe interno

## Status

Aceito.

## Contexto

`titan-data` precisa de uma biblioteca de dados por trás de `import data`.
Escrever um motor de DataFrame do zero (mesmo que mínimo) era possível, mas
reimplementaria leitura de CSV, agregações e tipagem de coluna — trabalho já
resolvido e testado no ecossistema Rust. Polars foi escolhido como a
implementação (crate `polars`, `crates/titan-data/Cargo.toml`, features
`lazy` + `csv`).

Escolher Polars carrega um risco concreto, medido antes da fase começar
(risco 1 do `PRD.md`): **~2min de build e ~3GB de `target/` por programa**
que usa `import data` — porque o `titanc` gera um projeto Cargo novo por
programa compilado (`build/<nome>/`), o custo de disco se paga a cada
programa, não uma vez só. Um programa Titan que nunca importa `data` não
pode ser forçado a pagar esse custo.

## Decisão

O programa Titan só enxerga a API `data.*` (`read_csv`, `linhas`, `colunas`,
`coluna`, `soma`/`media`/`minimo`/`maximo`, o tipo opaco `DataFrame`) —
nunca o Polars diretamente. Todo tipo Polars fica encapsulado dentro de
`titan_data::DataFrame` (`crates/titan-data/src/lib.rs:18`,
`pub struct DataFrame(polars::frame::DataFrame)`, campo privado); nenhuma
função pública de `titan-data` aceita ou devolve um tipo do crate `polars`.
Trocar Polars por outro motor no futuro é, em princípio, uma mudança
inteiramente dentro de `titan-data` — a assinatura de `data.*` (nomes,
tipos, comportamento observável) é o contrato que não muda.

O corolário prático de "backend é detalhe interno": o custo de build/disco
do Polars só é pago por quem importa `data`. `collect_deps`
(`driver.rs:154`) monta a lista de dependências do `Cargo.toml` gerado a
partir dos módulos realmente importados pelo programa — `titan-runtime`
sempre, `titan-data` só quando o programa tem `import data`. Um programa sem
`import` (`hello.titan`, `nucleo.titan`, `compostos.titan`) nunca inclui
Polars nas suas dependências geradas, e portanto nunca paga o custo medido.

## Consequências

- Mitigação de teste (risco 1): **um único** caso de integração ponta a
  ponta invoca o `cargo build` de um programa com `import data`
  (`examples/dados.titan`, T45) — todos os demais casos que exercitam
  `data.*` usam `--emit-rust`, que gera o Rust e para sem invocar o `cargo`
  (T44).
- `titan-data` é livre para trocar Polars por outra biblioteca (ou por uma
  implementação própria) sem exigir mudança em nenhum programa `.titan`
  existente, desde que a assinatura de `data.*` e o comportamento observado
  (mensagens de erro, valores de agregação) se mantenham. O contrato mora em
  `capabilities.rs` (nomes, tipos, `rust_path`) mais o comportamento
  documentado neste ADR — não no código do Polars.
- `DataFrame` precisa de `Clone` barato para não violar a suposição do
  mecanismo de compostos ([ADR 0013](0013-tipo-opaco-composto-por-heranca.md));
  qualquer backend candidato a substituir o Polars herda essa mesma
  exigência.
- Erros do Polars (que chegam em inglês, de dentro do crate) são sempre
  traduzidos para uma mensagem em português antes de cruzar a fronteira de
  `titan-data` (`read_csv_checked`, `crates/titan-data/src/lib.rs:29-35`) —
  o usuário do `.titan` nunca vê uma mensagem de erro do backend interno.
