# ADR 0009 — Records como `struct` Rust nominal

## Status

Aceito.

## Contexto

O Titan original representa um `record` como uma `Table*` do Lua com campos
nomeados — a mesma representação de memória usada para arrays e maps,
diferenciada apenas em tempo de checagem de tipo (`coder.lua:474-488`).
Reproduzir isso em Rust exigiria uma representação de "tabela genérica" (um
`HashMap<String, Value>` ou enum de valor dinâmico), abrindo mão da
verificação de campos em tempo de compilação que `struct`s nativas do Rust
dão de graça.

Duas outras questões precisavam de decisão junto:

- **Construtor estático.** O Titan original desaçucara `Ponto.new(...)` num
  `TopLevelStatic` sintético (`parser.lua:215-229`). Métodos estáticos estão
  fora de escopo da Fase 2 (só chegam com métodos em geral, fase futura), e
  implementar esse desaçúcar só para o construtor criaria um caso especial
  isolado que nada mais no compilador usa.
- **Namespace de nomes.** Funções top-level já passam por mangling
  (`mangle_fn_name`, prefixo `titan_`) para não colidir com o `fn main` do
  shim de entrada nem com keywords do Rust. Records precisavam de uma decisão
  própria sobre se o mesmo mangling se aplica a nomes de tipo.

## Decisão

Cada `record` do Titan vira uma `struct` Rust própria, emitida com
`#[derive(Clone, Debug, PartialEq)]` e campos `pub`. `Clone` é obrigatório —
é o que sustenta a semântica de valor do [ADR 0006](0006-semantica-de-valor-clone-na-atribuicao.md).
`Copy` **nunca** é derivado: um record pode conter `String` ou `Vec`, que não
são `Copy`, então derivar `Copy` incondicionalmente quebraria para qualquer
record com campo composto ou string.

**Sem mangling no nome do tipo** — `record Ponto` vira `struct Ponto`, sem
prefixo. O namespace de tipos do Rust é separado do namespace de valores
(onde `fn main` mora), então não há colisão possível com o shim de entrada; e
nomes reservados do Rust (`String`, `Vec`, `Option`, `Box`, `Result`,
`HashMap`) são rejeitados pelo checker antes de chegar ao codegen, então não
há colisão com tipos da std.

**Sem construtor sintético.** Um record é construído só por literal
(`Ponto { x = 1.0, y = 2.0 }`, checado exaustivamente pelo checker — todo
campo presente, nenhum extra, nenhum posicional). Não existe `Ponto.new(...)`
nesta fase.

**Record recursivo é rejeitado pelo checker.** `record No prox: No end`
seria um tipo de tamanho infinito em Rust sem indireção (`Box`), que o
`rustc` recusaria com um erro em inglês (`recursive type has infinite size`).
O checker verifica isso antes e rejeita com mensagem em português, mantendo a
convenção de nunca deixar o usuário ver um erro do compilador Rust
subjacente.

## Consequências

- Campos de record são verificados em tempo de compilação do Rust gerado
  (`struct` real, não mapa dinâmico) — um erro de nome de campo já teria sido
  pego pelo checker do Titan-Rust antes, então essa checagem dupla é
  redundância barata, não uma dependência nova.
- `#[derive(PartialEq)]` dá `==`/`~=` estrutural entre records de graça, sem
  código de comparação escrito à mão no codegen.
- Sem construtor estático, todo record precisa ser construído por literal
  completo em todo call site — mais verboso que `Ponto.new(1, 2)` do
  original, mas sem o caso especial de desaçúcar que só esse construtor
  precisaria.
- Record recursivo, mesmo que semanticamente razoável com `Box` (`prox:
  Option<Box<No>>`), é rejeitado nesta fase — falta a `Option` para expressar
  a base do recursivo (`Nil`). Revisitar quando `Option` entrar no sistema de
  tipos.
