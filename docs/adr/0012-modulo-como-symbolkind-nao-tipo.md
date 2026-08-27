# ADR 0012 — Módulo é `SymbolKind`, não `Type`

## Status

Aceito.

## Contexto

O Titan original modela módulo como um caso de `Type` (`Type.Module` em
`types.lua`) — um módulo é um valor com tipo próprio, como qualquer outro.
Ao desenhar `import data` para esta fase ([ADR 0011](0011-import-como-acucar-sintatico.md)),
a mesma escolha se repetia: dar a `data` um `Type::Module` faria o sistema de
tipos existente (`compatible`, `equals`, os `match` de codegen sobre `Type`)
aceitar `data` em qualquer lugar onde um tipo é esperado — anotação de
variável (`local x: data = ...`), campo de record, elemento de array,
parâmetro de função.

Nenhum desses lugares faz sentido para um módulo nesta fase: `data` não é um
valor que circula pelo programa, é um nome fixo que só existe para qualificar
acesso (`data.read_csv(...)`, `data.DataFrame`). Inflar `Type` com mais uma
variante que 90% do resto do checker/codegen precisaria saber ignorar era o
sinal de que a modelagem estava no lugar errado.

## Decisão

Módulo é uma variante de `SymbolKind` (`checker.rs:91`,
`SymbolKind::Module { name: String }`), não de `Type`. `import data` insere
`data` na tabela de símbolos com esse `kind` (`checker.rs:695`); o símbolo
não carrega nenhum `Type` significativo, porque nunca é usado como um.

Consequência direta: todo ponto do checker que itera sobre `SymbolKind` para
decidir o que uma atribuição ou uma anotação de tipo significa ganha um braço
explícito para `Module` que **sempre rejeita** — `checker.rs:1271`
(atribuição: "não é possível atribuir a um módulo") e `checker.rs:1348`
(mesma rejeição no caminho de leitura de variável raiz para
composição/mutação). `data = outracoisa`, `local x: data = ...` e qualquer
uso de `data` como valor são erros de checker, não de parser — a gramática
aceita a sintaxe (`data` é um `Name` válido em qualquer posição de
expressão), mas o checker nega semanticamente.

## Consequências

- `Type` não ganhou uma variante `Module` — o `match` exaustivo em
  `rust_type_name`/`rust_param_type_name` (`codegen.rs`) e em `compatible`/
  `equals` (`types.rs`) não precisa de um braço para um caso que nunca
  poderia ocorrer ali.
- Resolver `data.read_csv(...)` e `data.DataFrame` passa por
  `SymbolKind::Module { name }` → `capabilities::lookup_module(name)`
  (`checker.rs:2013`, `checker.rs:2249`), nunca pelo mecanismo de tipos —
  são dois sistemas de resolução paralelos e intencionalmente distintos.
- Diverge do original: lá, tratar módulo como tipo era possível porque não
  havia o rastreio de mutabilidade por `&mut` desta fase
  ([ADR 0007](0007-parametros-compostos-por-mut.md)) forçando cada `Type` a
  ter uma representação Rust coerente. Aqui, cada `Type` novo é um
  compromisso com `rust_type_name`; `SymbolKind::Module` evita esse
  compromisso para algo que nunca terá uma representação Rust própria —
  módulo não existe em tempo de execução, só em tempo de compilação.
