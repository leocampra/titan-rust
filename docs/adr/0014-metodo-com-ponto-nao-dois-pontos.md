# ADR 0014 — Método com `.` e com `:`, sendo `.` a forma preferida

## Status

Aceito (revisado na T72 — ver "Revisão" abaixo).

## Contexto

O Titan original (como o Lua que o inspira) usa dois-pontos para chamada de
método (`obj:metodo(args)`), açúcar sintático para `obj.metodo(obj, args)` —
o dois-pontos insere o receptor como primeiro argumento implícito. Ao
desenhar como `df.soma(df, "valor")` e a forma abreviada equivalente
coexistiriam nesta fase (o `PRD.md` da T45 exige que ambas funcionem:
`data.soma(df, "valor")` **e** `df.soma("valor")`), a sintaxe do dois-pontos
era a escolha natural por precedente do original.

Mas o dois-pontos exige uma regra gramatical própria (`:` como token de
chamada, distinto de `.` como token de acesso a campo) só para essa
diferença semântica — inserir o receptor implicitamente. O parser desta fase
já resolve `data.soma(...)` e `df.soma(...)` pelo mesmo caminho sintático
(`VarDot` seguido de chamada); a única diferença entre os dois é
**semântica**, resolvida no checker (`SymbolKind::Module` vs. tipo `Opaque`
do receptor), não sintática.

## Decisão

Chamada de método aceita **as duas formas**, com `.` sendo a preferida:

- `df.soma("valor")` é parseado como qualquer acesso a campo seguido de
  chamada (`VarDot` + `ExpCall`);
- `df:soma("valor")` é parseado como `ExpCall` cujo `exp` é o receptor e
  cujos argumentos são `Args::ArgsMethod { method, args }`.

As duas formas **convergem no checker**, em `resolve_method_callee`: ao ver
que o receptor tem `Type::Opaque`, o método é resolvido contra
`capability.find_method` e as duas produzem o mesmo `Callee::Method`, com o
mesmo receptor e os mesmos argumentos. Do checker para baixo elas são
indistinguíveis — o codegen não sabe qual foi escrita, e o Rust gerado é
byte a byte o mesmo (provado por
`emit_rust_de_dois_pontos_e_identico_ao_de_ponto`, em
`crates/titanc/tests/integration.rs`).

`.` segue sendo a forma **preferida** na documentação e nos exemplos, por
ser a mesma sintaxe de acesso a campo e de função de módulo — uma regra a
menos para quem lê. `:` existe por compatibilidade com o idioma do Titan
original.

## Revisão (T72)

A decisão original desta ADR era mais forte: `:` **não existia** na
gramática, e `df:soma(...)` era rejeitado pelo parser com erro de sintaxe
("não há suporte a ambas as formas nem plano de adicionar dois-pontos como
alias de `.` em fase futura"). A T72 reverteu esse corte.

O que mudou não foi o julgamento sobre o valor de ter duas sintaxes — esse
segue sendo baixo —, mas o **custo** de oferecê-las. `Args::ArgsMethod` já
existia na AST (`ast.rs`) desde o começo, sem nenhuma referência fora dela; e
o ponto do checker onde `.` resolve o método já estava isolado. Construir a
segunda forma custou um braço a mais no laço de sufixos do parser e a
extração de `resolve_method_callee` do braço `VarDot` de `resolve_callee` —
nenhum mecanismo novo, nenhuma entrada nova no codegen. Com esse custo, o
argumento de "duas formas de sintaxe para a mesma coisa" deixa de pagar por
si a divergência do original.

## Consequências

- Uma única regra **semântica** (receptor com `Type::Opaque` → resolve
  contra `capability.find_method`) cobre as duas sintaxes; a distinção entre
  função de módulo (`data.read_csv(...)`), método sobre opaco (`df.soma(...)`
  ou `df:soma(...)`) e leitura de campo de record (`p.campo`) continua sendo
  inteiramente do checker, nunca do parser.
- Um programa `.titan` escrito no idioma do original (`obj:metodo()`)
  compila aqui sem reescrita — o contrário do que esta ADR dizia antes da
  T72.
- `data.soma(df, "valor")` (forma de função de módulo, receptor explícito),
  `df.soma("valor")` e `df:soma("valor")` chamam o mesmo `CapabilityFn`.
  `examples/dados.titan` (T45) exercita as duas primeiras sobre o mesmo `df`
  e confere que o relatório bate.
- O `:` entra no laço de sufixos de `parse_suffixed_exp`, então encadeia com
  `.` e `[` como qualquer outro sufixo (`a.b:c(1).d`). Não há ambiguidade com
  o `:` de anotação de tipo: aquele é consumido por `parse_decl` /
  `parse_decl_opt_type` / `parse_rettypes_opt` / `parse_type`, sempre depois
  de um **nome** em posição de declaração, nunca depois de uma expressão.
