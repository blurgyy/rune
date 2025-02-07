import * as lc from "vscode-languageclient/node";
import { Env, substituteVariablesInEnv } from "./config";
import { outputChannel, traceOutputChannel } from "./extension";
import { log } from "./util";

export function createClient(
    serverPath: string,
    extraEnv: Env
): lc.LanguageClient {
    const newEnv = substituteVariablesInEnv(Object.assign({}, process.env, extraEnv));
    log.debug('newEnv', newEnv);

    const run: lc.Executable = {
        command: serverPath,
        options: { env: newEnv }
    };
    const serverOptions: lc.ServerOptions = {
        run,
        debug: run,
    };

    const clientOptions: lc.LanguageClientOptions = {
        documentSelector: [{ scheme: 'file', language: 'rune' }],
        traceOutputChannel: traceOutputChannel(),
        outputChannel: outputChannel(),
    };

    const client = new lc.LanguageClient(
        "rune-languageserver",
        "Rune Language Server",
        serverOptions,
        clientOptions
    );

    return client;
}
