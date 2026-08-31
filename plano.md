Projeto Titan esta na pasta /home/leonardo/titan-rust/titan
Projeto Lua esta na pasta /home/leonardo/titan-rust/lua

# Uma nova plataforma de desenvolvimento para o Redox OS

Estou pensando em um projeto que começou com uma ideia relativamente simples: **recriar/evoluir o compilador da linguagem Lua para que, em vez de gerar código C ou depender de um runtime tradicional, ele gere código Rust**.

A ideia é usar como ponto de partida o **Titan**, uma linguagem derivada do Lua que já possui lexer, parser, AST, análise de tipos, symbol table e compilador. Em vez de construir tudo do zero, poderíamos aproveitar essa base e desenvolver um novo backend capaz de transformar a linguagem em Rust.

A arquitetura inicial seria:

```text
Linguagem baseada em Lua
        ↓
Lexer
        ↓
Parser
        ↓
AST
        ↓
Análise semântica / tipos
        ↓
IR
        ↓
Backend Rust
        ↓
Cargo / rustc
        ↓
Executável nativo
```

O objetivo não seria simplesmente criar um "Lua que compila para Rust". A ideia é criar uma **linguagem de alto nível para desenvolvimento de sistemas**, mantendo a simplicidade e produtividade do Lua, mas aproveitando a segurança e o ecossistema do Rust.

---

# Por que o Redox OS?

O Redox seria a primeira plataforma-alvo porque ele já é construído em Rust e possui uma arquitetura muito interessante baseada em microkernel e serviços.

A princípio, **não precisaríamos modificar o kernel do Redox**.

A linguagem, o compilador e os runtimes seriam programas instalados no espaço de usuário.

Seria possível instalar a plataforma no Redox e começar a desenvolver imediatamente:

```text
Redox OS
   │
   ├── Compiler
   ├── Language Runtime
   ├── Standard Library
   └── Capability Runtimes
```

O kernel continuaria responsável apenas pelo que deve ser responsabilidade do kernel: processos, memória, IPC, drivers básicos, segurança etc.

---

# A linguagem poderia desenvolver muito mais do que aplicações

Um dos objetivos seria permitir que o desenvolvedor use a mesma linguagem para criar:

* aplicações;
* bibliotecas;
* serviços;
* sistemas de arquivos;
* componentes de rede;
* ferramentas;
* futuramente drivers;
* aplicações de IA;
* sistemas de automação.

Por exemplo, conceitualmente:

```lua
driver "wifi" {

    fn probe(device)

    fn read(buffer)

    fn write(buffer)

}
```

O compilador poderia gerar a implementação Rust necessária para integrar esse driver ao Redox.

Da mesma forma:

```lua
library "finance"
```

poderia gerar uma biblioteca Rust reutilizável por outros softwares.

A ideia é que o desenvolvedor não precise escrever Rust diretamente para grande parte dessas tarefas, mas o resultado final continue sendo código nativo e seguro baseado no ecossistema Rust.

---

# O segundo grande componente: Capability Runtimes

Depois surgiu uma ideia ainda maior.

Em vez de a linguagem ter dezenas de bibliotecas independentes, o sistema teria **Runtimes especializados por capacidade**.

Por exemplo:

```text
AI Runtime
Crypto Runtime
Security Runtime
Data Runtime
Database Runtime
Network Runtime
Graphics Runtime
Audio Runtime
Video Runtime
IoT Runtime
Robotics Runtime
Cloud Runtime
```

A linguagem teria APIs simples para acessar essas capacidades.

Por exemplo:

```lua
import ai

local model = Model("Llama")

local resposta =
    model.chat("Analise este documento.")
```

O programa não precisaria saber como o modelo está sendo executado.

O AI Runtime cuidaria de:

* carregamento do modelo;
* gerenciamento de memória;
* CPU/GPU/NPU;
* contexto;
* embeddings;
* RAG;
* memória;
* tool calling;
* agentes;
* inferência.

O backend poderia mudar sem modificar o programa.

---

# IA como capacidade nativa do sistema

A ideia é que a IA deixe de ser apenas uma API externa.

Hoje normalmente temos:

```text
Aplicação
    ↓
API OpenAI / outro provedor
    ↓
Internet
    ↓
LLM
```

A proposta seria:

```text
Aplicação
    ↓
AI Runtime
    ↓
LLM local
    ↓
GPU / CPU / NPU
```

Assim, o usuário poderia executar LLMs privadas diretamente no computador.

Por exemplo:

```lua
local agente = Agent {

    model = "Llama",

    knowledge = "./empresa",

    memory = PersistentMemory(),

    tools = {
        Filesystem,
        Database
    }
}

local resposta =
    agente.ask(
        "Analise o fluxo de caixa da empresa."
    )
```

A linguagem poderia ter conceitos nativos como:

```text
Model
Agent
Memory
Knowledge
Tool
Workflow
VectorStore
Embedding
Event
```

Não necessariamente implementados dentro do compilador. O compilador apenas geraria chamadas para o AI Runtime.

---

# A instalação seria simples

A ideia é que, ao instalar a linguagem no Redox:

```bash
pkg install aether
```

o sistema já instale:

```text
Compiler
Runtime
Standard Library
Capability Manager
AI Runtime
LLM backend
Modelo padrão
```

Ou seja, o usuário poderia instalar a linguagem e imediatamente executar:

```lua
local model = Model()

print(model.chat("Olá"))
```

Depois poderia adicionar outras capacidades:

```bash
aether add crypto
aether add data
aether add security
aether add robotics
```

Cada Capability instalaria:

```text
Biblioteca da linguagem
        +
Runtime
        +
Backend necessário
```

A ideia é que o desenvolvedor pense em **capacidades**, e não em dezenas de bibliotecas e SDKs individuais.

---

# Crypto Runtime

O mesmo modelo poderia ser usado para criptografia.

```lua
import crypto

local key = crypto.generate_key()

local signature =
    crypto.sign(key, document)
```

O Crypto Runtime poderia utilizar diferentes implementações por baixo:

```text
RustCrypto
libsodium
OpenSSL
hardware security
```

A aplicação não precisaria ficar acoplada a uma implementação específica.

---

# Security Runtime

Também poderia existir um Security Runtime para:

* criptografia;
* auditoria;
* monitoramento;
* análise de vulnerabilidades;
* segurança de aplicações;
* IDS/IPS;
* compliance;
* gerenciamento de certificados.

Por exemplo:

```lua
import security

local report =
    security.audit("./system")
```

Funcionalidades de pentest poderiam existir como módulos opcionais e ser utilizadas apenas em ambientes autorizados.

---

# Data Science Runtime

A mesma arquitetura poderia transformar a maneira como softwares de ciência de dados são construídos.

Hoje é comum montar ambientes com:

```text
Python
Pandas
NumPy
PyTorch
Scikit-Learn
Jupyter
CUDA
DuckDB
Spark
Parquet
etc.
```

Na proposta, poderiam existir APIs de alto nível:

```lua
import data
import ai

local df =
    DataFrame.read("vendas.parquet")

local resultado =
    df.group_by("cidade")
```

O Data Runtime escolheria as implementações adequadas, podendo utilizar tecnologias como Arrow, Parquet, Polars, DuckDB e aceleradores de hardware.

A ideia não é necessariamente substituir essas tecnologias, mas **transformá-las em componentes internos dos runtimes**, escondendo a complexidade da infraestrutura da aplicação.

---

# O conceito central

A ideia acabou evoluindo de:

> "Criar um compilador Lua → Rust"

para:

> **Criar uma plataforma de desenvolvimento orientada a capacidades, onde uma linguagem simples de alto nível permite acessar serviços do sistema operacional e runtimes especializados.**

O modelo seria:

```text
                    Linguagem
                       │
                       ▼
                   Compilador
                       │
                       ▼
                     Rust
                       │
                       ▼
                     Redox
                       │
          ┌────────────┼────────────┐
          ▼            ▼            ▼
         AI         Crypto       Security
       Runtime      Runtime       Runtime
          │            │            │
          └────────────┼────────────┘
                       ▼
                Capability Manager
```

---

# O papel do Redox

O Redox não precisaria ser modificado inicialmente.

A primeira versão poderia ser completamente instalada como software.

Depois, conforme o projeto amadurecesse, poderíamos integrar mais profundamente com conceitos próprios do Redox, como seus serviços e Schemes.

Por exemplo, futuramente poderíamos ter recursos como:

```text
model://
agent://
memory://
vector://
workflow://
```

Isso permitiria tratar modelos, agentes e outros recursos de IA de maneira semelhante à forma como o Redox trata outros recursos do sistema.

---

# O compilador também poderia evoluir

Inicialmente ele poderia funcionar como:

```text
Source
 ↓
AST
 ↓
Type Checker
 ↓
Rust Backend
 ↓
Cargo
```

Depois poderia ter um serviço residente semelhante a um language server:

```text
Editor
   ↓
Compiler Service
   ↓
AST / Types / IR
```

Isso permitiria:

* autocomplete;
* análise de erros;
* refatoração;
* compilação incremental;
* documentação automática;
* integração com IA.

No futuro, a própria linguagem poderia ser usada para desenvolver o próprio compilador, chegando a um estágio de **self-hosting**.

---

# O Titan como ponto de partida

Pesquisando projetos existentes, encontramos o **Titan**, uma linguagem inspirada em Lua que já possui vários componentes que seriam necessários:

```text
Lexer
Parser
AST
Type Checker
Symbol Table
Modules
Runtime
```

Por isso, em vez de recriar tudo do zero, poderíamos usar o Titan como base experimental.

A principal alteração seria substituir o backend atual por:

```text
Titan / nova linguagem
        ↓
Rust Backend
        ↓
Cargo
        ↓
Executável nativo
```

Depois evoluir a linguagem para incorporar:

```text
Ownership
Borrowing
Traits
Generics
FFI Rust
Redox SDK
Capabilities
```

e finalmente transformar o projeto em uma linguagem própria.

---

# O objetivo final

A visão de longo prazo seria ter um ambiente no qual um desenvolvedor possa instalar o sistema e começar a construir software sem precisar montar manualmente uma enorme cadeia de ferramentas.

Por exemplo:

```text
Instala Redox
     ↓
Instala Aether
     ↓
AI Runtime já disponível
     ↓
Llama local funcionando
     ↓
aether add crypto
     ↓
aether add data
     ↓
aether add security
```

E então escrever:

```lua
import ai
import crypto
import data

local dados =
    DataFrame.read("clientes.parquet")

local resumo =
    Model().chat(
        "Analise estes dados."
    )

local assinatura =
    crypto.sign(resumo)

print(assinatura)
```

Tudo compilado para Rust e executado nativamente.

---

# Em uma frase

**A proposta é criar uma linguagem simples, inspirada em Lua, compilada para Rust, capaz de construir aplicações, bibliotecas, serviços e componentes de sistemas, enquanto o sistema operacional fornece capacidades especializadas — como IA, criptografia, segurança e ciência de dados — por meio de runtimes modulares.**

O Redox seria a primeira plataforma de referência para essa visão.

Não seria apenas uma nova linguagem.

Seria uma tentativa de criar um **novo modelo de desenvolvimento de software em que capacidades como IA, segurança, dados e hardware são serviços nativos e padronizados da plataforma.**

---

# Fase 1 — Núcleo da linguagem

A Fase 0 está concluída: existe um compilador novo, escrito em Rust, que leva
`examples/hello.titan` até um executável nativo, com o pipeline completo
(`lexer → parser → checker → codegen → driver`) e testes verdes.

A Fase 1 transforma esse pipeline num **núcleo de linguagem de verdade**: aritmética,
comparação, lógica booleana, controle de fluxo (`if`/`while`/`for`) e atribuição — o
suficiente para escrever algoritmos reais (fatorial, fibonacci) em Titan e compilá-los
para Rust nativo.

Um ponto de partida favorável descoberto no fechamento da Fase 0: a AST em `ast.rs` já foi
definida **completa** (com `StatIf`, `StatWhile`, `StatFor`, `StatAssign`, `ExpBinop`,
`ExpUnop`). A Fase 1 não precisa criar nós novos — ela ensina o parser a produzi-los, o
checker a verificá-los e o codegen a traduzi-los.

## O que a fase cobre

```text
Operadores aritméticos    + - * / % ^
Operadores relacionais    == ~= < > <= >=
Operadores lógicos        and or not
Unários                   - (negação)  not
Controle de fluxo         if / elseif / else · while · for numérico
Atribuição                x = exp (variável local já declarada)
Concatenação ampliada     "texto" .. 42 (número vira string)
```

## Decisões de design fixadas

1. **`for` apenas numérico** — `for x = start, finish[, inc] do ... end` com integer ou
   float. `for-in` (iteradores) depende de construções da Fase 2+.
2. **Atribuição single-target** — `nome = exp` para local já declarada. Multi-assign
   (`a, b = 1, 2`), índice e campo ficam para fases futuras.
3. **Sem bitwise, sem `//`, sem `#`** — o conjunto de operadores é o essencial do núcleo;
   bitwise e divisão inteira podem voltar numa fase posterior.
4. **`..` coage número para string** — como o Titan original (`trytostr`), `"x: " .. 42`
   funciona; a conversão vira `to_string()` no Rust gerado.
5. **`for` exige tipos idênticos** — `start`, `finish` e `inc` devem bater exatamente com
   o tipo da variável de controle, fiel ao checker original (sem coerção implícita no laço).
6. **Mutabilidade rastreada no checker** — o Rust gerado usa `let mut` apenas nas variáveis
   de fato reatribuídas, mantendo o código gerado limpo (sem warnings do rustc).
7. **`and`/`or` boolean estrito** — os dois lados devem ser boolean e o resultado é boolean
   (mapeiam para `&&`/`||`). Divergência deliberada do truthy/falsy do Lua/Titan original,
   que só faz sentido quando `Value`/`Option` entrarem em uso (fases futuras).

## O que continua fora

Records, maps, arrays manipuláveis, `import`/`foreign import`, métodos, retornos múltiplos,
`Option`/`?`, `repeat`/`until`, `break`/`continue` — tudo continua rejeitado pelo compilador
com mensagem clara em português, nunca com panic. **Redox segue fora de escopo** — o alvo é
Linux nativo via cargo, e nada na arquitetura fecha a porta para um `--target` futuro.

A decisão mais delicada do projeto (modelo de memória em Rust para tipos compostos) continua
adiada para a Fase 2, e a Fase 1 é desenhada para não antecipá-la: nenhuma construção nova
assume que valores são `Copy` além dos primitivos.

A lista de tarefas executável da Fase 1 (T10–T18) está no `PRD.md`.

---

# Fase 2 — Tipos compostos

As Fases 0 e 1 entregaram o pipeline completo e um núcleo de linguagem capaz de
escrever algoritmos reais — mas apenas sobre valores escalares. A Fase 2 é a que
o roadmap sempre marcou como **alta complexidade**: *"arrays, maps, records,
strings dinâmicas — ownership/borrow mordem aqui"*.

É aqui que se paga o custo real do projeto. Gerar Rust para `integer` e `boolean`
é mecânico: eles são `Copy`, não têm dono, não têm tempo de vida. Um array não é
nada disso. A pergunta que a Fase 0 deliberadamente adiou — **qual modelo de
memória em Rust representa um valor composto do Titan?** — não tem mais como ser
adiada, e a resposta condiciona todo o resto da linguagem.

A Fase 0 foi desenhada para não fechar essa porta: o mapeamento de tipos ficou
isolado em duas funções (`rust_type_name` e `rust_param_type_name`, em
`codegen.rs`), justamente para que essa escolha fosse uma mudança localizada.

## O problema central

No Titan original, arrays, maps e records são **ponteiros para objetos
gerenciados pelo GC do Lua** (`Table*`, `CClosure*`). Isso tem uma consequência
semântica direta:

```lua
local a: {integer} = {1, 2, 3}
local b: {integer} = a   -- 'b' e 'a' são o MESMO array
b[1] = 99                -- 'a' também muda
```

O código gerado pelo backend original é literalmente `$CVAR = $CEXP;` — cópia de
ponteiro, nunca de conteúdo (`coder.lua:1321`). O manual do Titan prova a
identidade atravessando a fronteira com Lua: `assert(t == t2)` depois de a tabela
entrar e sair de uma função Titan (`doc/manual.md:191-203`).

Rust não dá isso de graça. Reproduzir aliasing exigiria `Rc<RefCell<...>>` — que
traz `.borrow_mut()` em cada indexação, risco de panic em runtime por duplo
empréstimo (violando a regra "nunca panic") e vazamento em records recursivos.

## As decisões desta fase

**1. Semântica de valor: `Vec`/`struct` donos, com clone na atribuição.**
`{integer}` vira `Vec<i64>`, um record vira uma `struct` Rust própria, e
`local b = a` emite `let b = a.clone();`. Isso **diverge do original** — não há
aliasing — e também do Rust idiomático, onde a atribuição moveria. O custo O(n)
é aceito conscientemente em troca de código gerado simples, sem `Rc`, sem
`RefCell`, sem risco de panic.

**2. Parâmetros por `&mut`, preservando a mutação in-place.**
Esta é a contrapartida da decisão anterior, e é independente dela: clonar **na
atribuição** não obriga a clonar **na passagem de parâmetro**. A diferença
decide se o idioma mais comum da linguagem de referência funciona:

```lua
function selection_sort(xs: {integer}): nil   -- ordena in-place, devolve nil
    ...
    xs[i] = xs[min_i]
end
```

Com passagem por valor, essa função compilaria, rodaria, ordenaria uma cópia — e
o chamador não veria nada. Um bug silencioso, sem erro de compilação, no idioma
central da linguagem. Com `&mut Vec<i64>`, o chamador vê a ordenação, ao custo
O(1), e **neste ponto convergimos** com o original.

O preço: `f(xs, xs)` é legal em Titan e o borrow checker do Rust o recusaria —
em inglês. O checker passa a rejeitá-lo antes, em português.

**3. `v[i]` tem tipo `T`, com checagem de faixa em português.**
No original, indexar produz `T?` (nunca `T`) — é o mecanismo de proteção contra
buracos. Mas `Option`/`?` está fora do escopo desta fase, e trazê-lo junto
exigiria narrowing de fluxo no checker. Em vez disso, `v[i]` tem tipo `T` e a
checagem vai para o runtime: índice inválido aborta com
`"índice 99 fora da faixa (array tem 3 elementos)"` — nunca o panic cru do Rust
em inglês.

**4. Escrever em `#v + 1` faz append.**
O original cresce o array silenciosamente ao escrever além do fim, porque uma
tabela Lua é um mapa esparso. Replicar isso sobre `Vec` exigiria inventar valores
default — e para `{Ponto}` não existe default sensato. A solução preserva
exatamente o idioma que importa (`res[#res+1] = x`, o padrão de append do
original) e recusa o resto com mensagem clara.

## O que a fase cobre

```text
Arrays            {T}  ·  {1,2,3}  ·  v[i]  ·  v[i] = x  ·  #v  ·  {{T}}
Records           record Nome campo: T end  ·  p.campo  ·  {campo = valor}
Maps              {K: V}  ·  m[k]  ·  m[k] = v
Strings           string passa a ser sempre String (fim da dualidade &str/String)
```

## O que continua fora

`Option`/`?`, cast `as`, métodos (`:`), `import`, retornos múltiplos,
multi-assign, `repeat`/`until`, `break`/`continue`, bitwise, `//`. Tudo segue
rejeitado com mensagem clara em português, nunca com panic.

Uma consequência nova: construções que o **rustc** recusaria em inglês passam a
ser rejeitadas antes, pelo checker, em português — `f(xs, xs)`, record recursivo
(`record No prox: No end`, que seria infinito em Rust sem `Box`), nome de record
colidindo com tipo do Rust (`record String`), e chave de map `float` (o `HashMap`
exige `Eq + Hash`, que `f64` não tem).

## Divergências deliberadas, registradas em ADR

| ADR | Decisão |
|---|---|
| 0006 | Semântica de valor com clone na atribuição (diverge do aliasing do original) |
| 0007 | Parâmetros compostos por `&mut`, preservando o idioma in-place |
| 0008 | Indexação checada no runtime, `T` em vez de `T?`; variância invariante |
| 0009 | Records como `struct` Rust nominal |
| 0010 | `string` é sempre `String` — fim da dualidade `&str`/`String` |

**Redox segue fora de escopo** — o alvo é Linux nativo via cargo.

A lista de tarefas executável da Fase 2 (T19–T33) está no `PRD.md`.



---

# Fase 3 — Capability Runtimes

As Fases 0–2 entregaram um compilador completo para uma linguagem **fechada em
si mesma**: tudo que um programa `.titan` pode fazer está no próprio arquivo,
mais `print`/`concat` do `titan-runtime`. Não há `import`, não há namespace,
não há como o programa alcançar uma biblioteca.

Isso é exatamente o que separa o projeto da sua tese central. A ideia deste
plano não é "um Lua que compila para Rust" — é que o desenvolvedor **adicione
capacidades** em vez de instalar dezenas de bibliotecas, e que cada capacidade
seja um runtime especializado escondendo a infraestrutura atrás de uma API
simples. Enquanto o compilador não souber o que é um módulo, essa tese não
existe em código.

A Fase 3 entrega **o mecanismo de capabilities** e o prova com **uma
capability real**: `titan-data`, um Data Runtime sobre o Polars. Ao fim,
`import data` mais um CSV de verdade produzem um relatório impresso por um
executável nativo.

## O ponto de partida é melhor do que parecia

A investigação que abriu a fase encontrou o terreno muito mais preparado do
que o roadmap sugeria — em parte porque a `ast.rs` foi escrita completa desde
a Fase 0, em parte por acidente feliz:

- **`TopLevelImport`, `TypeQualName` e `ArgsMethod` já existem na AST**, e
  nenhum deles é construído hoje.
- **O parser já aceita `data.read_csv(x)` e `df.soma("valor")` sem nenhuma
  mudança.** O loop de sufixos é genérico: `.nome` vira `VarDot`, `(args)`
  vira `ExpCall`. As duas formas morrem no *checker*, não no parser.
- **O codegen não tem preâmbulo de `use`** — toda chamada de runtime é escrita
  qualificada inline (`titan_runtime::print(...)`). Acrescentar
  `titan_data::...` não exige tocar em nenhum cabeçalho gerado.

Sobra, portanto, o trabalho que de fato importa: ensinar o **checker** o que é
um módulo, o que é um tipo que ele não pode inspecionar, e como resolver o que
está à esquerda de um ponto.

## As decisões desta fase

**1. `import data`, e não `local data = import "data"`.**
O Titan original só tem a segunda forma, com acesso sempre pelo alias. Este
plano sempre escreveu a primeira. Adotamos a do plano, tratando-a como açúcar
da do original (`localname == modname`) — todo o resto do desenho de
referência continua valendo sem mudança.

**2. Módulo não é um tipo — é uma espécie de símbolo.**
O original admite em comentário que `Type.Module` é um hack, e paga o preço
com erros espalhados por todo uso indevido ("trying to access module as a
first-class value", "trying to assign to a module"). Aqui o módulo entra como
variante de `SymbolKind`, e módulo-como-valor fica **irrepresentável** em vez
de rejeitado caso a caso.

**3. Tipos opacos: `Type::Opaque`.**
`data.DataFrame` é um tipo nominal que o programa carrega e passa adiante, mas
cujos campos não pode inspecionar — ele não é declarado no `.titan`, vem do
runtime. Detalhe que barateia a fase inteira: fazendo o opaco entrar em
`is_composite`, ele herda de graça toda a máquina de lugares da Fase 2 —
parâmetro por `&mut` (ADR 0007), `clone()` na atribuição (ADR 0006),
`emit_place_mut`/`emit_place_expr`. O preço é uma exigência nova: **todo tipo
opaco de runtime precisa implementar `Clone`**.

**4. As duas formas de chamada, porque as duas resolvem no mesmo lugar.**
`data.read_csv("v.csv")` cria o DataFrame (função do módulo — não há receptor
ainda, então ela precisa existir de qualquer jeito) e `df.soma("valor")` opera
sobre ele (método). Parecia que métodos seriam o item mais caro do escopo; a
investigação mostrou que ambas passam pelo mesmo ponto do checker, que olha o
que está à esquerda do ponto — símbolo de módulo, expressão opaca ou record. É
um braço a mais, não um mecanismo novo.

**5. Método com ponto, não dois-pontos.**
O original usa `:` para método e reserva `.` para campo. Aqui o opaco não tem
campos acessíveis, então não há ambiguidade a desfazer — e é `.` que este
plano sempre escreveu (`model.chat(...)`).

## A escolha do backend, e a medição que a inverteu

A intenção inicial era um backend leve agora (Arrow/csv) e o Polars depois,
justamente para não impor uma árvore de dependências pesada a todo programa
que fizesse `import data`. A medição desfez a premissa:

| Dependência | Build limpo |
|---|---|
| crate `csv` | **6,7s** (release) |
| `arrow` (default-features off, csv) | **1m54s** (release) |
| `polars` (lazy, csv, parquet) | **1m43s** (debug) |
| rebuild incremental | **0,55s** |

Arrow custa o mesmo que o Polars. O meio-termo não existia: ou se paga ~2min
uma vez e se ganha o motor completo, ou se fica no `csv` cru reimplementando
agregação à mão. Decisão: **Polars**. O custo é amortizado porque o `titanc`
compila em `build/<nome>/`, que persiste entre execuções — só a primeira
compilação de cada programa paga.

Isso não fecha a porta que motivou a hesitação. A **API `data.*` é o
contrato; o backend é detalhe interno** (ADR 0015) — trocar o motor por baixo
não muda uma linha do programa Titan. Que é, afinal, a tese deste plano
aplicada a si mesma.

## O que a fase cobre

```text
Mecanismo    import data  ·  namespace  ·  data.f(...)  ·  tipo opaco  ·  df.m(...)
             dependências condicionais no Cargo.toml gerado
titan-data   read_csv  ·  linhas  ·  colunas  ·  coluna_integer/coluna_float
             soma  ·  media  ·  minimo  ·  maximo
```

Toda a superfície do `titan-data` foi validada contra o Polars 0.51 antes de a
fase começar — as assinaturas compilam e produzem os valores esperados sobre
um CSV real.

## O que continua fora

`foreign import`, `import` com alias (`import data as d`), módulos definidos
pelo usuário (um `.titan` importando outro `.titan` — esta fase só tem
capabilities internas), `titan-crypto`, `titan-ai`, `Option`/`?`, cast `as`,
retornos múltiplos, multi-assign, `repeat`/`until`, `break`/`continue`,
bitwise, `//`, e `df:metodo()` com dois-pontos. Tudo segue rejeitado com
mensagem clara em português, nunca com panic.

`titan-crypto` e `titan-ai` ficam para as fases 3b/3c: com o mecanismo pronto,
cada um vira essencialmente um crate novo e uma tabela de funções — engenharia
de biblioteca, não de compilador.

## Divergências deliberadas, registradas em ADR

| ADR | Decisão |
|---|---|
| 0011 | `import data` como açúcar de `local data = import "data"` (diverge do original) |
| 0012 | Módulo é `SymbolKind`, não tipo (diverge do `Type.Module` do original) |
| 0013 | Tipo opaco `Type::Opaque`, composto por herança (exige `Clone`) |
| 0014 | Método com ponto, não dois-pontos (diverge do original) |
| 0015 | API `data.*` é o contrato, backend é detalhe interno (Polars trocável) |

**Redox segue fora de escopo** — o alvo é Linux nativo via cargo.

A lista de tarefas executável da Fase 3 (T34–T46) está no `PRD.md`.

---

# Fase 4 — Self-hosting / LSP

As Fases 0–3 entregaram um compilador que leva um `.titan` até executável
nativo e um mecanismo de capabilities provado por um runtime real. O roadmap
sempre listou a Fase 4 como *"compilador escrito na própria linguagem —
complexidade muito alta"*, mas nunca a detalhou. A investigação que abre a
fase explica por quê: **self-hosting completo não cabe numa fase.**

## A medição que recortou a fase

Para escrever o `titanc` em Titan, faltam — em ordem de gravidade:

| Lacuna | Onde é rejeitada | Consequência |
|---|---|---|
| Tipos soma (`enum`/`match`) | não existem em `types.rs` | um `Exp` de 13 variantes não tem representação |
| Módulos de usuário | `driver.rs` lê **um** arquivo | o compilador teria de caber num único `.titan` |
| Ler arquivo | não há capability de I/O | o compilador não alcança o próprio fonte |
| Acesso a caractere | `checker.rs:2035` | um lexer não avança sobre o fonte |
| `string` ↔ número | sem `tonumber`/`tostring` | literal numérico não vira valor |
| `break`/`continue` | nem são keywords | laço de scan vira flag booleana |
| Retornos múltiplos | `checker.rs:1483` | `(token, pos)` vira record ou `&mut` |

O `checker.rs` sozinho tem 4144 linhas; sem módulos de usuário e sem tipos
soma, reescrevê-lo em Titan produziria um arquivo único, com uma AST de
records "gordos" e tags inteiras — um estilo que o próprio `titanc` não usa,
o que anularia boa parte do valor do exercício.

Do lado do LSP o quadro se inverte. Há um bloqueador estrutural, mas barato:
o `titanc` é **só um binário** — `main.rs` declara os módulos como `mod`
privados, sem `lib.rs`, então nada pode reusar o pipeline como biblioteca.
Fora isso o terreno é bom, e por acidente feliz: `lex`/`parse`/`check`/
`generate` são funções puras, todo erro já carrega `Loc { line, col }`, e
`checker::check` já devolve `Vec<CheckError>` — vários diagnósticos de uma
vez, que é exatamente o que um language server publica.

Daí o recorte: **a fase entrega duas provas — o LSP em Rust, completo e
demonstrável num editor, e a fundação de self-hosting, provada pelo lexer do
Titan escrito em Titan.** Tipos soma, módulos de usuário e o parser/checker
auto-hospedados ficam para a Fase 5.

## As decisões desta fase

**1. Acesso a texto por capability, não por builtin nem por `s[i]`.**
`import texto` traz `texto.byte`, `texto.sub`, `texto.para_inteiro` e
`texto.de_inteiro`. É a tese do plano aplicada a si mesma — capacidade, não
biblioteca — e sai quase de graça: a ABI de argumentos já emite corretamente
uma assinatura `(string, integer) -> integer`, então a capability é uma tabela
mais um crate, sem tocar em checker nem codegen. As alternativas custariam
mais: builtins globais exigiriam namespace numa tabela que hoje é plana, e
`s[i]` seria assimétrico (leitura sim, escrita não) e ainda deixaria
`sub`/`para_inteiro` sem casa.

**2. `break` sim, `continue` não.** `break` mapeia direto para o `break` do
Rust. `continue` não: o `for` numérico é desaçucarado para `while` com o
incremento **no fim do corpo** (ADR 0004), então um `continue` pularia o
incremento e produziria laço infinito — um bug silencioso, sem erro de
compilação, no idioma mais comum de um lexer. Fica rejeitado com mensagem
clara até a Fase 5 tratar o caso.

**3. O `break` é o primeiro nó de AST realmente novo do projeto.** Nas fases
anteriores a `ast.rs` já vinha completa e a tarefa era ensinar parser, checker
e codegen a usá-la. Aqui não: o Titan original também não tem `break`
(`ast.lua:33-42`), então o nó não existe em lugar nenhum. É trabalho pequeno,
mas em cinco arquivos.

**4. O LSP não invoca o `cargo`.** Ele roda `lex → parse → check` sobre o
buffer em memória e para aí — nunca gera projeto, nunca compila. É o que torna
o diagnóstico instantâneo, e é possível porque o pipeline já é feito de
funções puras.

**5. Protocolo por biblioteca (`tower-lsp`), semântica por conta própria.**
Reimplementar o framing `Content-Length` e ~100 structs do LSP não é o
trabalho interessante da fase. As dependências entram no workspace do
compilador e **nunca** no `Cargo.toml` gerado por programa, que continua
montado só a partir das capabilities de fato importadas.

## O que a fase cobre

```text
LSP          titanc como lib  ·  diagnósticos  ·  hover  ·  go-to-definition
             autocomplete  ·  extensão VS Code mínima
Linguagem    break  ·  capability `texto` (byte, sub, para_inteiro, de_inteiro)
             capability `io` (ler_arquivo)
Prova        examples/lexer.titan — o lexer do Titan escrito em Titan
```

## O que continua fora

Tipos soma e `match`, `continue`, módulos de usuário (um `.titan` importando
outro), parser e checker auto-hospedados, `Option`/`?`, cast `as`, retornos
múltiplos, multi-assign, `repeat`/`until`, `for`-in, bitwise, `//`,
`foreign import`, `import` com alias e `df:metodo()`. Tudo segue rejeitado
com mensagem clara em português, nunca com panic.

Uma consequência assumida: **o `examples/lexer.titan` vai ficar deselegante.**
Sem variáveis de topo, as constantes de `TokenKind` viram funções sem
argumento (`function TK_IF(): integer return 1 end`); sem retornos múltiplos,
a posição corrente anda num record de estado passado por `&mut`; sem tipos
soma, o token carrega uma tag inteira. Isso é dado, não defeito — é a
evidência empírica que justifica a Fase 5, e fica registrada no ADR 0020 em
vez de disfarçada.

## Divergências deliberadas, registradas em ADR

| ADR | Decisão |
|---|---|
| 0016 | Acesso a texto por capability `texto`, não builtins globais nem `s[i]` |
| 0017 | `break` sim, `continue` não (o `for` desaçucarado perderia o incremento) |
| 0018 | `titanc` exposto como lib: o LSP reusa o pipeline sem invocar o `cargo` |
| 0019 | LSP sobre `tower-lsp`; deps do servidor nunca entram no `Cargo.toml` gerado |
| 0020 | Self-hosting por etapas: lexer na Fase 4, parser/checker na Fase 5 |

**Redox segue fora de escopo** — o alvo é Linux nativo via cargo.

A lista de tarefas executável da Fase 4 (T47–T58) está no `PRD.md`.
