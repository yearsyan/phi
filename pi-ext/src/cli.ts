#!/usr/bin/env node

import { parseArgs } from "node:util";

import { createPiDaemon } from "./index.js";

const VERSION = "0.1.0";

async function main(): Promise<void> {
  const { values } = parseArgs({
    strict: true,
    allowPositionals: false,
    options: {
      bind: { type: "string" },
      "data-dir": { type: "string" },
      "auth-key-file": { type: "string" },
      workspace: { type: "string" },
      "tls-cert-file": { type: "string" },
      "tls-key-file": { type: "string" },
      help: { type: "boolean", short: "h" },
      version: { type: "boolean", short: "v" },
    },
  });
  if (values.help) {
    process.stdout.write(helpText());
    return;
  }
  if (values.version) {
    process.stdout.write(`${VERSION}\n`);
    return;
  }

  const env: NodeJS.ProcessEnv = {
    ...process.env,
    ...(values.bind === undefined ? {} : { PI_EXT_BIND: values.bind }),
    ...(values["data-dir"] === undefined ? {} : { PI_EXT_DATA_DIR: values["data-dir"] }),
    ...(values["auth-key-file"] === undefined
      ? {}
      : { PI_EXT_AUTH_KEY_FILE: values["auth-key-file"] }),
    ...(values.workspace === undefined ? {} : { PI_EXT_WORKSPACE_DIR: values.workspace }),
    ...(values["tls-cert-file"] === undefined
      ? {}
      : { PI_EXT_TLS_CERT_FILE: values["tls-cert-file"] }),
    ...(values["tls-key-file"] === undefined
      ? {}
      : { PI_EXT_TLS_KEY_FILE: values["tls-key-file"] }),
  };
  const daemon = await createPiDaemon({ env });
  const address = await daemon.start();
  const scheme = daemon.config.tls === undefined ? "http" : "https";
  const host = address.family === "IPv6" ? `[${address.address}]` : address.address;
  process.stdout.write(
    `pi-daemon listening on ${scheme}://${host}:${address.port}\n` +
      `Pi config: ${daemon.config.agentDir}\n` +
      `Daemon auth key file: ${daemon.config.authKeyFile}\n`,
  );

  let closing = false;
  const shutdown = async (): Promise<void> => {
    if (closing) return;
    closing = true;
    await daemon.close();
  };
  process.once("SIGINT", () => void shutdown());
  process.once("SIGTERM", () => void shutdown());
}

function helpText(): string {
  return `pi-daemon ${VERSION}

Phi daemon v1-compatible transport backed by @earendil-works/pi-coding-agent.

Usage: pi-daemon [options]

Options:
  --bind <host:port>       Listen address (default: 127.0.0.1:8788)
  --data-dir <path>        Control data (default: <Pi agent dir>/daemon)
  --auth-key-file <path>   Long-lived bearer key file
  --workspace <path>       Default workspace
  --tls-cert-file <path>   PEM certificate (requires --tls-key-file)
  --tls-key-file <path>    PEM private key (requires --tls-cert-file)
  -h, --help               Show this help
  -v, --version            Show the version
`;
}

void main().catch((error: unknown) => {
  const message = error instanceof Error ? error.message : String(error);
  process.stderr.write(`pi-daemon: ${message}\n`);
  process.exitCode = 1;
});
