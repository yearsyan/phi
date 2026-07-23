import { stat } from "node:fs/promises";
import { mkdtemp, mkdir, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { afterEach, describe, expect, it } from "vitest";

import {
  DEFAULT_BIND_ADDRESS,
  loadDaemonConfig,
  parseBindAddress,
} from "../src/config.js";

const temporaryDirectories: string[] = [];

afterEach(async () => {
  await Promise.all(
    temporaryDirectories.splice(0).map((path) => rm(path, { recursive: true, force: true })),
  );
});

describe("daemon config", () => {
  it("uses the Pi agent directory and the Pi extension port", async () => {
    const root = await temporaryDirectory();
    const agentDir = join(root, "pi-agent");
    const workspace = join(root, "workspace");
    await mkdir(workspace);

    const config = await loadDaemonConfig({
      agentDir,
      cwd: workspace,
      env: {},
    });

    expect(config.bindAddress).toBe(DEFAULT_BIND_ADDRESS);
    expect(config.port).toBe(8788);
    expect(config.dataDir).toBe(join(agentDir, "daemon"));
    expect(config.authKeyFile).toBe(join(agentDir, "daemon", "auth.key"));
    expect(Buffer.byteLength(config.authKey)).toBeGreaterThanOrEqual(32);
    if (process.platform !== "win32") {
      expect((await stat(config.authKeyFile)).mode & 0o777).toBe(0o600);
    }
  });

  it("parses IPv4, hostnames, IPv6, and ephemeral ports", () => {
    expect(parseBindAddress("127.0.0.1:0")).toEqual({ host: "127.0.0.1", port: 0 });
    expect(parseBindAddress("localhost:8788")).toEqual({ host: "localhost", port: 8788 });
    expect(parseBindAddress("[::1]:8788")).toEqual({ host: "::1", port: 8788 });
    expect(() => parseBindAddress("127.0.0.1")).toThrow("invalid bind address");
    expect(() => parseBindAddress("127.0.0.1:70000")).toThrow("invalid bind address");
  });
});

async function temporaryDirectory(): Promise<string> {
  const path = await mkdtemp(join(tmpdir(), "pi-ext-config-"));
  temporaryDirectories.push(path);
  return path;
}
