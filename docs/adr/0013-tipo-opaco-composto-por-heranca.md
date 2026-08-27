# ADR 0013 — Tipo opaco (`Type::Opaque`) composto por herança

## Status

Aceito.

## Contexto

`titan-data` precisa expor um tipo (`data.DataFrame`) cujo programa Titan
carrega, passa entre funções e chama métodos sobre ele — mas nunca inspeciona
por dentro (nenhum campo, nenhuma construção por literal). É estruturalmente
diferente de `record` ([ADR 0009](0009-records-como-struct-rust-nominal.md)):
um `record` é definido *pelo* programa Titan (`record Nome ... end`,
campos conhecidos do checker); um tipo de capability é definido *pelo runtime
Rust* (`titan_data::DataFrame`, uma struct comum do crate `titan-data`) e o
checker só conhece seu nome e módulo de origem, nunca seus campos.

A pergunta central: um `DataFrame` passado como parâmetro de função deveria
ser copiado (semântica de valor, [ADR 0006](0006-semantica-de-valor-clone-na-atribuicao.md))
ou referenciado (`&mut`, como os demais compostos,
[ADR 0007](0007-parametros-compostos-por-mut.md))? A segunda opção exige que
o checker **já trate** `Opaque` como composto — não há uma terceira via
"opaco mas não composto" sem duplicar toda a lógica de `is_composite` para
mais um caso.

## Decisão

`Type::Opaque { module, name, rust_path }` (`types.rs:41`) é uma variante
própria de `Type` — não um `Record` disfarçado, não um alias de `String`.
Carrega o nome do módulo de origem (`data`), o nome Titan do tipo
(`DataFrame`) e o caminho Rust totalmente qualificado (`titan_data::DataFrame`,
usado só na emissão de código, fora da identidade do tipo — `equals` ignora
`rust_path`, `types.rs:76-86`).

A decisão central: `is_composite` (`checker.rs:2399`) inclui `Opaque` no
mesmo grupo de `Array`/`Map`/`Record`. `Opaque` **herda** o tratamento de
composto em vez de ganhar um caminho próprio — todo lugar que já sabe passar
um composto por `&mut` (parâmetros de função, receptor de método) passa a
tratar `Opaque` sem código adicional. É essa herança que barateia a fase:
zero lógica nova de "como passar um `DataFrame` para uma função", porque a
máquina de `is_composite` já resolve.

Consequência que a herança impõe de volta: todo tipo de runtime usado como
`Opaque` **precisa** implementar `Clone`, porque a máquina de compostos
assume que clonar é sempre uma operação válida e barata (`local b = a`
clona, [ADR 0006](0006-semantica-de-valor-clone-na-atribuicao.md)). Isso não
é opcional para adotar o mecanismo — é o preço de entrada. Para `DataFrame`
sobre Polars, `Clone` é barato (o `DataFrame` do Polars é
copy-on-write/`Arc`-backed internamente); **isso não está garantido para todo
tipo de runtime futuro**, e um `Clone` caro seria um custo silencioso: o
programa Titan continuaria compilando e rodando, só ficaria lento, sem
nenhum erro sinalizando o motivo (risco 2 do `PRD.md`, seção Fase 3).

## Consequências

- Novo tipo opaco em uma fase futura (`titan-crypto`, `titan-ai`) herda
  automaticamente `&mut` em parâmetro/receptor por já cair em `is_composite`
  — nenhuma mudança no checker além de registrar o tipo em
  `capabilities.rs`.
- Cada tipo de runtime usado como `Opaque` deve implementar `Clone`,
  documentado aqui como pré-requisito explícito: antes de adicionar uma
  capability nova com um tipo opaco, confirmar que `Clone` é barato para
  esse tipo — do contrário, medir o custo e decidir explicitamente (this ADR
  é o lugar para registrar a exceção, não um silêncio).
- `Type::Opaque` não é construído por literal do programa Titan — só surge
  como retorno de uma `CapabilityFn` (`data.read_csv(...)`, preenchido por
  `requalify_rettype`, `checker.rs:2411`) ou como parâmetro anotado com o
  tipo qualificado (`data.DataFrame`, resolvido em `checker.rs:814`).
- `equals` compara `Opaque` por `(module, name)` (`types.rs:76-86`), nunca
  por `rust_path` — dois `Opaque` do mesmo módulo/nome são o mesmo tipo
  mesmo que o caminho Rust mude entre versões do crate de runtime.
