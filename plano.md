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



