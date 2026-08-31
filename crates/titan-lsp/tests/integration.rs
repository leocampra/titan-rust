//! Teste de integração ponta a ponta do `titan-lsp` (PRD.md, T48/T49): fala
//! JSON-RPC de verdade com o binário via stdin/stdout, no mesmo espírito do
//! teste de `titanc/tests/integration.rs` — aciona o processo real, não as
//! funções internas.
//!
//! Cobre a armadilha central da tarefa: `Loc.col` é 1-indexado e conta
//! **bytes**, enquanto o LSP por padrão usa posição 0-indexada em UTF-16. O
//! servidor evita a conversão anunciando `positionEncoding: "utf-8"` em
//! `initialize` — este teste manda um `.titan` com acento (o projeto inteiro
//! escreve em português) para expor um eventual erro de posicionamento que
//! só apareceria em fonte não-ASCII.
//!
//! `hover_sobre_local_array_mostra_o_tipo` e
//! `goto_definition_sobre_chamada_salta_para_a_funcao` cobrem os dois
//! critérios de aceite da T49.

use std::io::{BufReader, Read, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use serde_json::{Value, json};

struct LspClient {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    next_id: i64,
}

impl LspClient {
    fn start() -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_titan-lsp"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("falha ao iniciar o processo titan-lsp");

        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());

        LspClient {
            child,
            stdin: Some(stdin),
            stdout,
            next_id: 1,
        }
    }

    fn write_message(&mut self, body: &Value) {
        let payload = serde_json::to_string(body).unwrap();
        let stdin = self.stdin.as_mut().expect("stdin já foi fechado");
        write!(
            stdin,
            "Content-Length: {}\r\n\r\n{}",
            payload.len(),
            payload
        )
        .unwrap();
        stdin.flush().unwrap();
    }

    /// Lê uma mensagem `Content-Length`-delimitada de stdout — request ou
    /// notification, o framing é o mesmo.
    fn read_message(&mut self) -> Value {
        let mut content_length: Option<usize> = None;
        loop {
            let mut line = String::new();
            let mut byte = [0u8; 1];
            loop {
                self.stdout.read_exact(&mut byte).unwrap();
                line.push(byte[0] as char);
                if line.ends_with("\r\n") {
                    break;
                }
            }
            let trimmed = line.trim_end();
            if trimmed.is_empty() {
                break;
            }
            if let Some(value) = trimmed.strip_prefix("Content-Length: ") {
                content_length = Some(value.parse().unwrap());
            }
        }

        let len = content_length.expect("cabeçalho Content-Length ausente");
        let mut buf = vec![0u8; len];
        self.stdout.read_exact(&mut buf).unwrap();
        serde_json::from_slice(&buf).unwrap()
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.write_message(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        }));
        loop {
            let msg = self.read_message();
            if msg.get("id") == Some(&json!(id)) {
                return msg;
            }
            // notification (ex.: window/logMessage) recebida antes da
            // resposta — ignora e continua esperando.
        }
    }

    fn notify(&mut self, method: &str, params: Value) {
        self.write_message(&json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        }));
    }

    /// Espera especificamente por uma notificação `textDocument/publishDiagnostics`,
    /// ignorando outras notificações (ex.: `window/logMessage`) no meio do caminho.
    fn wait_for_publish_diagnostics(&mut self) -> Value {
        loop {
            let msg = self.read_message();
            if msg.get("method") == Some(&json!("textDocument/publishDiagnostics")) {
                return msg;
            }
        }
    }

    fn shutdown_and_exit(&mut self) {
        self.request("shutdown", Value::Null);
        self.notify("exit", Value::Null);
        // `tower-lsp` só encerra o loop de leitura ao ver EOF em stdin —
        // sem fechar aqui, `child.wait()` trava esperando o processo sair.
        self.stdin.take();
        self.child.wait().unwrap();
    }
}

#[test]
fn did_open_com_erro_de_tipo_publica_diagnostico_com_posicao_correta() {
    let mut client = LspClient::start();

    let init = client.request(
        "initialize",
        json!({
            "processId": null,
            "rootUri": null,
            "capabilities": {},
        }),
    );
    assert_eq!(
        init["result"]["capabilities"]["positionEncoding"],
        json!("utf-8"),
        "servidor precisa anunciar positionEncoding utf-8 para Loc (bytes) valer como offset direto"
    );
    client.notify("initialized", json!({}));

    // Identificador acentuado ("preço") antes do erro de tipo: em UTF-16 a
    // coluna do erro seria diferente da contagem em bytes que `Loc` usa —
    // é exatamente a armadilha que este teste precisa expor.
    let source = "function main(args: {string}): integer\n    local preço: integer = \"oi\"\n    return 0\nend";
    let uri = "file:///teste_acento.titan";

    client.notify(
        "textDocument/didOpen",
        json!({
            "textDocument": {
                "uri": uri,
                "languageId": "titan",
                "version": 1,
                "text": source,
            }
        }),
    );

    let publish = client.wait_for_publish_diagnostics();
    let params = &publish["params"];
    assert_eq!(params["uri"], json!(uri));

    let diagnostics = params["diagnostics"].as_array().unwrap();
    assert!(
        !diagnostics.is_empty(),
        "esperava ao menos um diagnóstico de erro de tipo, recebeu: {diagnostics:?}"
    );

    let diag = &diagnostics[0];
    let message = diag["message"].as_str().unwrap();
    assert!(
        message.to_lowercase().contains("incompat")
            || message.to_lowercase().contains("tipo")
            || message.to_lowercase().contains("string"),
        "mensagem em português do erro de tipo não bateu o esperado: {message}"
    );

    // Linha 1 (0-indexada) é `    local preço: integer = "oi"`, e a `Loc`
    // do checker é 1-indexada — o diagnóstico do LSP deve ser 0-indexado.
    let start_line = diag["range"]["start"]["line"].as_u64().unwrap();
    assert_eq!(start_line, 1, "linha do diagnóstico não convertida para 0-indexado");

    client.shutdown_and_exit();
}

#[test]
fn did_open_sem_erro_publica_lista_vazia_de_diagnosticos() {
    let mut client = LspClient::start();

    client.request(
        "initialize",
        json!({
            "processId": null,
            "rootUri": null,
            "capabilities": {},
        }),
    );
    client.notify("initialized", json!({}));

    let source = "function main(args: {string}): integer\n    return 0\nend";
    let uri = "file:///teste_ok.titan";

    client.notify(
        "textDocument/didOpen",
        json!({
            "textDocument": {
                "uri": uri,
                "languageId": "titan",
                "version": 1,
                "text": source,
            }
        }),
    );

    let publish = client.wait_for_publish_diagnostics();
    let diagnostics = publish["params"]["diagnostics"].as_array().unwrap();
    assert!(
        diagnostics.is_empty(),
        "programa válido não deveria gerar diagnósticos: {diagnostics:?}"
    );

    client.shutdown_and_exit();
}

/// Critério de aceite da T49: hover sobre uma variável local `{integer}`
/// mostra o tipo formatado por `checker::type_name`.
#[test]
fn hover_sobre_local_array_mostra_o_tipo() {
    let mut client = LspClient::start();

    client.request(
        "initialize",
        json!({"processId": null, "rootUri": null, "capabilities": {}}),
    );
    client.notify("initialized", json!({}));

    // Linha 1: `    local qs: {integer} = {5, 3, 1, 4, 2}` — `qs` começa na
    // coluna 11 (1-indexada, contagem em bytes: 4 espaços + "local " = 10).
    let source = "function main(args: {string}): integer\n    local qs: {integer} = {5, 3, 1, 4, 2}\n    return 0\nend";
    let uri = "file:///teste_hover.titan";

    client.notify(
        "textDocument/didOpen",
        json!({
            "textDocument": {
                "uri": uri,
                "languageId": "titan",
                "version": 1,
                "text": source,
            }
        }),
    );
    client.wait_for_publish_diagnostics();

    let response = client.request(
        "textDocument/hover",
        json!({
            "textDocument": {"uri": uri},
            // 0-indexado: linha 1 é `    local qs: ...`; coluna 11 (0-idx)
            // cai em cima do `q` de `qs`.
            "position": {"line": 1, "character": 11},
        }),
    );

    let contents = &response["result"]["contents"];
    let text = contents["value"]
        .as_str()
        .or_else(|| contents.as_str())
        .expect("hover deveria ter contents de texto");
    assert!(
        text.contains("{integer}"),
        "hover não mostrou o tipo esperado: {text}"
    );

    client.shutdown_and_exit();
}

/// Critério de aceite da T49: go-to-definition sobre a chamada de uma
/// função salta para a declaração `function`.
#[test]
fn goto_definition_sobre_chamada_salta_para_a_funcao() {
    let mut client = LspClient::start();

    client.request(
        "initialize",
        json!({"processId": null, "rootUri": null, "capabilities": {}}),
    );
    client.notify("initialized", json!({}));

    // Linha 0: `function ajuda(): nil` — `ajuda` começa na coluna 9 (0-idx).
    // Linha 4: `    ajuda()` — chamada a `ajuda` na coluna 4 (0-idx).
    let source = "function ajuda(): nil\n\
                  end\n\
                  \n\
                  function main(args: {string}): integer\n\
                  \x20   ajuda()\n\
                  \x20   return 0\n\
                  end";
    let uri = "file:///teste_definicao.titan";

    client.notify(
        "textDocument/didOpen",
        json!({
            "textDocument": {
                "uri": uri,
                "languageId": "titan",
                "version": 1,
                "text": source,
            }
        }),
    );
    client.wait_for_publish_diagnostics();

    let response = client.request(
        "textDocument/definition",
        json!({
            "textDocument": {"uri": uri},
            "position": {"line": 4, "character": 4},
        }),
    );

    let result = &response["result"];
    let range = if result.is_array() {
        &result[0]["range"]
    } else {
        &result["range"]
    };
    assert_eq!(
        range["start"]["line"].as_u64(),
        Some(0),
        "go-to-definition deveria saltar para a linha da `function ajuda`, resultado: {response:?}"
    );

    client.shutdown_and_exit();
}
