import { mkdtemp, mkdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { afterEach, describe, expect, it } from "vitest";

import { browseWorkspace, resolveWorkspacePath } from "../src/workspace.js";

const temporaryDirectories: string[] = [];

afterEach(async () => {
  await Promise.all(
    temporaryDirectories.splice(0).map((path) => rm(path, { recursive: true, force: true })),
  );
});

describe("workspace browser", () => {
  it("returns only directories in stable case-insensitive order", async () => {
    const root = await temporaryDirectory();
    await Promise.all([
      mkdir(join(root, "Zulu")),
      mkdir(join(root, "alpha")),
      writeFile(join(root, "file.txt"), "not a directory"),
    ]);

    const result = await browseWorkspace(root);

    expect(result.path).toBe(await resolveWorkspacePath(root));
    expect(result.directories.map(({ name }) => name)).toEqual(["alpha", "Zulu"]);
    expect(result.truncated).toBe(false);
  });

  it("rejects relative paths with a stable API error", async () => {
    await expect(resolveWorkspacePath("relative")).rejects.toMatchObject({
      status: 400,
      code: "invalid_workspace",
    });
  });
});

async function temporaryDirectory(): Promise<string> {
  const path = await mkdtemp(join(tmpdir(), "pi-ext-workspace-"));
  temporaryDirectories.push(path);
  return path;
}
