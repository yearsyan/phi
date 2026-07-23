import { randomBytes } from "node:crypto";
import { chmod, mkdir, open, readFile, rename, unlink } from "node:fs/promises";
import { dirname } from "node:path";

export class SecureJsonFile<T> {
  readonly #path: string;
  readonly #initial: () => T;
  #tail: Promise<void> = Promise.resolve();

  constructor(path: string, initial: () => T) {
    this.#path = path;
    this.#initial = initial;
  }

  get path(): string {
    return this.#path;
  }

  async read(): Promise<T> {
    await this.#tail;
    return this.#readNow();
  }

  async update<R>(mutator: (value: T) => R | Promise<R>): Promise<R> {
    let release: (() => void) | undefined;
    const previous = this.#tail;
    this.#tail = new Promise<void>((resolve) => {
      release = resolve;
    });
    await previous;
    try {
      const value = await this.#readNow();
      const result = await mutator(value);
      await writeSecureJson(this.#path, value);
      return result;
    } finally {
      release?.();
    }
  }

  async #readNow(): Promise<T> {
    try {
      const text = await readFile(this.#path, "utf8");
      return JSON.parse(text) as T;
    } catch (error) {
      if (isNodeError(error, "ENOENT")) return this.#initial();
      throw error;
    }
  }
}

export async function writeSecureJson(path: string, value: unknown): Promise<void> {
  const directory = dirname(path);
  await mkdir(directory, { recursive: true, mode: 0o700 });
  if (process.platform !== "win32") await chmod(directory, 0o700);
  const temporary = `${path}.${process.pid}.${randomBytes(6).toString("hex")}.tmp`;
  const handle = await open(temporary, "wx", 0o600);
  try {
    await handle.writeFile(`${JSON.stringify(value, null, 2)}\n`, "utf8");
    await handle.sync();
  } finally {
    await handle.close();
  }
  try {
    await rename(temporary, path);
    if (process.platform !== "win32") await chmod(path, 0o600);
  } catch (error) {
    await unlink(temporary).catch(() => undefined);
    throw error;
  }
}

function isNodeError(error: unknown, code: string): error is NodeJS.ErrnoException {
  return error instanceof Error && "code" in error && error.code === code;
}
