// Cliente VS Code para o titan-lsp (PRD.md, T51).
//
// Sobe o binário `titan-lsp` (crates/titan-lsp, T48/T49/T50) via stdio e
// negocia `positionEncoding: "utf-8"` — o servidor já anuncia isso em
// `initialize` (main.rs), então basta o cliente aceitar; sem isso um
// `.titan` com acento exporia posições erradas (o projeto inteiro escreve
// em português).

import * as vscode from "vscode";
import {
  LanguageClient,
  LanguageClientOptions,
  ServerOptions,
} from "vscode-languageclient/node";

let client: LanguageClient | undefined;

export function activate(context: vscode.ExtensionContext): void {
  const config = vscode.workspace.getConfiguration("titan");
  const command = config.get<string>("serverPath", "titan-lsp");

  const serverOptions: ServerOptions = {
    command,
    args: [],
  };

  const clientOptions: LanguageClientOptions = {
    documentSelector: [{ scheme: "file", language: "titan" }],
  };

  client = new LanguageClient(
    "titanLanguageServer",
    "Titan Language Server",
    serverOptions,
    clientOptions,
  );

  context.subscriptions.push(client);
  client.start();
}

export function deactivate(): Thenable<void> | undefined {
  return client?.stop();
}
