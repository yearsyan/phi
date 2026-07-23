import { randomBytes } from "node:crypto";
import { constants as fsConstants } from "node:fs";
import { access, chmod, mkdir, open, readFile, realpath } from "node:fs/promises";
import { isAbsolute, join, resolve } from "node:path";

import { getAgentDir } from "@earendil-works/pi-coding-agent";

export const DEFAULT_BIND_ADDRESS = "127.0.0.1:8788";
export const MIN_AUTH_KEY_BYTES = 32;
export const MAX_AUTH_KEY_BYTES = 4096;

export interface TlsConfig {
  certificateFile: string;
  privateKeyFile: string;
}

export interface DaemonConfig {
  host: string;
  port: number;
  bindAddress: string;
  agentDir: string;
  dataDir: string;
  authKeyFile: string;
  authKey: string;
  workspaceDir: string;
  tls?: TlsConfig;
  wsTokenTtlMs: number;
}

export interface LoadConfigOptions {
  env?: NodeJS.ProcessEnv;
  cwd?: string;
  agentDir?: string;
}

export async function loadDaemonConfig(options: LoadConfigOptions = {}): Promise<DaemonConfig> {
  const env = options.env ?? process.env;
  const cwd = resolve(options.cwd ?? process.cwd());
  const agentDir = resolve(options.agentDir ?? getAgentDir());
  const bindAddress = normalizedEnv(env.PI_EXT_BIND) ?? DEFAULT_BIND_ADDRESS;
  const { host, port } = parseBindAddress(bindAddress);
  const dataDir = resolve(normalizedEnv(env.PI_EXT_DATA_DIR) ?? join(agentDir, "daemon"));
  const configuredAuthPath = normalizedEnv(env.PI_EXT_AUTH_KEY_FILE);
  const authKeyFile = resolve(configuredAuthPath ?? join(dataDir, "auth.key"));
  const workspaceDir = await canonicalWorkspace(
    normalizedEnv(env.PI_EXT_WORKSPACE_DIR) ?? cwd,
    cwd,
  );
  const tlsCert = normalizedEnv(env.PI_EXT_TLS_CERT_FILE);
  const tlsKey = normalizedEnv(env.PI_EXT_TLS_KEY_FILE);
  if ((tlsCert === undefined) !== (tlsKey === undefined)) {
    throw new Error("PI_EXT_TLS_CERT_FILE and PI_EXT_TLS_KEY_FILE must be configured together");
  }
  let tls: TlsConfig | undefined;
  if (tlsCert !== undefined && tlsKey !== undefined) {
    await access(tlsCert, fsConstants.R_OK);
    await access(tlsKey, fsConstants.R_OK);
    tls = { certificateFile: resolve(tlsCert), privateKeyFile: resolve(tlsKey) };
  }
  await mkdir(dataDir, { recursive: true, mode: 0o700 });
  await bestEffortChmod(dataDir, 0o700);
  const authKey = await loadOrCreateAuthKey(authKeyFile, configuredAuthPath === undefined);
  return {
    host,
    port,
    bindAddress,
    agentDir,
    dataDir,
    authKeyFile,
    authKey,
    workspaceDir,
    ...(tls === undefined ? {} : { tls }),
    wsTokenTtlMs: 60_000,
  };
}

export function parseBindAddress(value: string): { host: string; port: number } {
  const input = value.trim();
  let host: string;
  let portText: string;
  if (input.startsWith("[")) {
    const close = input.indexOf("]");
    if (close < 0 || input[close + 1] !== ":") throw new Error(`invalid bind address: ${value}`);
    host = input.slice(1, close);
    portText = input.slice(close + 2);
  } else {
    const separator = input.lastIndexOf(":");
    if (separator <= 0) throw new Error(`invalid bind address: ${value}`);
    host = input.slice(0, separator);
    portText = input.slice(separator + 1);
  }
  const port = Number(portText);
  if (!host || !Number.isInteger(port) || port < 0 || port > 65_535) {
    throw new Error(`invalid bind address: ${value}`);
  }
  return { host, port };
}

export async function canonicalWorkspace(path: string, cwd = process.cwd()): Promise<string> {
  const absolute = isAbsolute(path) ? path : resolve(cwd, path);
  const canonical = await realpath(absolute);
  const stat = await import("node:fs/promises").then(({ stat }) => stat(canonical));
  if (!stat.isDirectory()) throw new Error(`workspace is not a directory: ${canonical}`);
  await access(canonical, fsConstants.R_OK);
  return canonical;
}

async function loadOrCreateAuthKey(path: string, mayCreate: boolean): Promise<string> {
  let text: string;
  try {
    text = await readFile(path, "utf8");
  } catch (error) {
    if (!isNodeError(error, "ENOENT") || !mayCreate) throw error;
    await mkdir(resolve(path, ".."), { recursive: true, mode: 0o700 });
    const generated = randomBytes(32).toString("base64url");
    const handle = await open(path, "wx", 0o600).catch(async (openError: unknown) => {
      if (!isNodeError(openError, "EEXIST")) throw openError;
      return undefined;
    });
    if (handle !== undefined) {
      try {
        await handle.writeFile(`${generated}\n`, "utf8");
      } finally {
        await handle.close();
      }
      text = generated;
    } else {
      text = await readFile(path, "utf8");
    }
  }
  const key = text.trim();
  const bytes = Buffer.byteLength(key);
  if (bytes < MIN_AUTH_KEY_BYTES || bytes > MAX_AUTH_KEY_BYTES || /\s/.test(key)) {
    throw new Error(
      `daemon auth key must contain ${MIN_AUTH_KEY_BYTES}-${MAX_AUTH_KEY_BYTES} non-whitespace bytes`,
    );
  }
  await bestEffortChmod(path, 0o600);
  return key;
}

function normalizedEnv(value: string | undefined): string | undefined {
  const normalized = value?.trim();
  return normalized ? normalized : undefined;
}

function isNodeError(error: unknown, code: string): error is NodeJS.ErrnoException {
  return error instanceof Error && "code" in error && error.code === code;
}

async function bestEffortChmod(path: string, mode: number): Promise<void> {
  if (process.platform === "win32") return;
  await chmod(path, mode);
}
