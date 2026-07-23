import { constants as fsConstants, type Dirent } from "node:fs";
import { access, readdir, realpath, stat } from "node:fs/promises";
import { isAbsolute, dirname, join } from "node:path";

import { ApiError } from "./errors.js";
import type { WorkspaceBrowseResponse, WorkspaceDirectory } from "./protocol.js";

const MAX_SCANNED_ENTRIES = 10_000;
const MAX_DIRECTORY_RESULTS = 2_000;

/** Resolve a client-provided workspace using the same absolute/canonical boundary as phi-daemon. */
export async function resolveWorkspacePath(path: string): Promise<string> {
  if (!path || !isAbsolute(path)) {
    throw new ApiError(
      400,
      "invalid_workspace",
      "workspace path must be a non-empty absolute path",
    );
  }

  let canonical: string;
  try {
    canonical = await realpath(path);
    const metadata = await stat(canonical);
    if (!metadata.isDirectory()) {
      throw new ApiError(
        400,
        "invalid_workspace",
        `workspace is not a directory: ${canonical}`,
      );
    }
    await access(canonical, fsConstants.R_OK);
    await readdir(canonical, { withFileTypes: true });
  } catch (error) {
    if (error instanceof ApiError) throw error;
    throw workspaceIoError(path, error);
  }
  return canonical;
}

export async function browseWorkspace(path: string): Promise<WorkspaceBrowseResponse> {
  const canonical = await resolveWorkspacePath(path);
  let entries: Dirent<string>[];
  try {
    entries = await readdir(canonical, { withFileTypes: true });
  } catch (error) {
    throw workspaceIoError(canonical, error);
  }

  const directories: WorkspaceDirectory[] = [];
  let scanned = 0;
  let truncated = false;
  for (const entry of entries) {
    scanned += 1;
    if (scanned > MAX_SCANNED_ENTRIES) {
      truncated = true;
      break;
    }
    const entryPath = join(canonical, entry.name);
    let isDirectory = entry.isDirectory();
    if (!isDirectory && entry.isSymbolicLink()) {
      try {
        isDirectory = (await stat(entryPath)).isDirectory();
      } catch {
        isDirectory = false;
      }
    }
    if (!isDirectory) continue;
    if (directories.length === MAX_DIRECTORY_RESULTS) {
      truncated = true;
      break;
    }
    directories.push({ name: entry.name, path: entryPath });
  }

  directories.sort(
    (left, right) =>
      left.name.toLocaleLowerCase("en-US").localeCompare(right.name.toLocaleLowerCase("en-US")) ||
      left.name.localeCompare(right.name),
  );
  const parentPath = dirname(canonical);
  return {
    path: canonical,
    parent: parentPath === canonical ? null : parentPath,
    directories,
    truncated,
  };
}

function workspaceIoError(path: string, error: unknown): ApiError {
  const code = isNodeError(error) ? error.code : undefined;
  const [status, stableCode] =
    code === "ENOENT"
      ? [404, "workspace_not_found"]
      : code === "EACCES" || code === "EPERM"
        ? [403, "workspace_unreadable"]
        : [500, "workspace_io_error"];
  const message = error instanceof Error ? error.message : String(error);
  return new ApiError(status, stableCode, `could not access workspace ${path}: ${message}`);
}

function isNodeError(error: unknown): error is NodeJS.ErrnoException {
  return error instanceof Error && "code" in error;
}
