import { lstat, readlink, realpath } from "node:fs/promises";
import { homedir } from "node:os";
import { basename, dirname, isAbsolute, relative, resolve } from "node:path";

import type {
  BeforeToolCallContext,
  BeforeToolCallResult,
} from "@earendil-works/pi-agent-core";
import { uuidv7 } from "@earendil-works/pi-ai";

import { CommandError } from "./errors.js";
import type {
  CapabilityMode,
  ToolEffect,
  ToolPermissionDecision,
  ToolPermissionPrompt,
  ToolPermissionRule,
} from "./protocol.js";
import { toJsonValue } from "./protocol.js";

const MAX_PENDING_PERMISSIONS = 64;
const MAX_SESSION_RULES = 256;
export const TOOL_PERMISSION_RULES_ENTRY = "phi.tool_permission_rules";

export interface ToolPermissionBrokerOptions {
  rules?: readonly ToolPermissionRule[];
  persistRules?: (rules: readonly ToolPermissionRule[]) => void;
  workspace?: string;
}

interface PendingPermission {
  prompt: ToolPermissionPrompt;
  resolve: (result: BeforeToolCallResult | undefined) => void;
  signal?: AbortSignal;
  abortListener?: () => void;
}

export interface ToolPermissionEvents {
  requested(prompt: ToolPermissionPrompt): void;
  resolved(permissionId: string, allowed: boolean): void;
  cancelled(permissionId: string): void;
}

/** Bridges Pi's beforeToolCall hook to Phi's reconnect-safe permission protocol. */
export class ToolPermissionBroker {
  readonly #pending = new Map<string, PendingPermission>();
  readonly #rules: ToolPermissionRule[];
  readonly #persistRules: ((rules: readonly ToolPermissionRule[]) => void) | undefined;
  readonly #workspace: string | undefined;
  #mode: CapabilityMode;
  #events: ToolPermissionEvents | undefined;
  #promptingEnabled = true;

  constructor(mode: CapabilityMode, options: ToolPermissionBrokerOptions = {}) {
    this.#mode = mode;
    this.#rules = normalizePermissionRules(options.rules ?? []);
    this.#persistRules = options.persistRules;
    this.#workspace = options.workspace;
  }

  bind(events: ToolPermissionEvents): void {
    this.#events = events;
  }

  setCapabilityMode(mode: CapabilityMode): void {
    this.#mode = mode;
  }

  disablePrompts(): void {
    this.#promptingEnabled = false;
  }

  listPending(): ToolPermissionPrompt[] {
    return [...this.#pending.values()].map(({ prompt }) => structuredClone(prompt));
  }

  async authorize(
    context: BeforeToolCallContext,
    signal?: AbortSignal,
  ): Promise<BeforeToolCallResult | undefined> {
    const effect = effectForTool(context.toolCall.name);
    if (
      this.#mode !== "full_access" &&
      this.#workspace !== undefined &&
      ["read", "edit", "write", "grep", "find", "ls"].includes(context.toolCall.name)
    ) {
      const workspaceViolation = await this.#workspaceViolation(
        context.toolCall.name,
        context.args ?? context.toolCall.arguments ?? {},
      );
      if (workspaceViolation !== undefined) return { block: true, reason: workspaceViolation };
    }
    if (capabilityAllows(this.#mode, effect)) return undefined;
    const argumentsValue = toJsonValue(context.args ?? context.toolCall.arguments ?? {});
    if (
      this.#rules.some((rule) =>
        matchesRule(rule, context.toolCall.name, context.args ?? context.toolCall.arguments ?? {}),
      )
    ) {
      return undefined;
    }
    if (!this.#promptingEnabled) {
      return {
        block: true,
        reason: `tool \`${context.toolCall.name}\` exceeds the unattended session capability`,
      };
    }
    if (signal?.aborted) return { block: true, reason: "tool execution was cancelled" };
    if (this.#pending.size >= MAX_PENDING_PERMISSIONS) {
      return { block: true, reason: "too many tool permission requests are pending" };
    }

    const permissionId = uuidv7();
    const prompt: ToolPermissionPrompt = {
      permission_id: permissionId,
      call: {
        id: context.toolCall.id,
        name: context.toolCall.name,
        arguments: argumentsValue,
      },
      effect,
      capability_mode: this.#mode,
      suggestions: suggestionsFor(context.toolCall.name, context.args ?? context.toolCall.arguments ?? {}),
    };
    return new Promise<BeforeToolCallResult | undefined>((resolve) => {
      const pending: PendingPermission = {
        prompt,
        resolve,
        ...(signal === undefined ? {} : { signal }),
      };
      if (signal !== undefined) {
        pending.abortListener = () => this.#cancel(permissionId, "tool execution was cancelled");
        signal.addEventListener("abort", pending.abortListener, { once: true });
      }
      this.#pending.set(permissionId, pending);
      this.#events?.requested(structuredClone(prompt));
    });
  }

  async #workspaceViolation(toolName: string, args: unknown): Promise<string | undefined> {
    if (this.#workspace === undefined) return undefined;
    const rawPath = isObject(args) && typeof args.path === "string" ? args.path : ".";
    try {
      const requested = resolveToolPath(rawPath, this.#workspace);
      const canonical = await canonicalPotentialPath(requested);
      if (isWithin(this.#workspace, canonical)) return undefined;
      return `tool \`${toolName}\` path escapes the session workspace`;
    } catch {
      return `tool \`${toolName}\` path could not be validated inside the session workspace`;
    }
  }

  decide(permissionId: string, decision: ToolPermissionDecision): void {
    const pending = this.#pending.get(permissionId);
    if (pending === undefined) {
      throw new CommandError(
        "tool_permission_not_pending",
        `tool permission \`${permissionId}\` is not pending`,
      );
    }
    let allowed: boolean;
    let result: BeforeToolCallResult | undefined;
    switch (decision.type) {
      case "allow_once":
        allowed = true;
        result = undefined;
        break;
      case "allow_for_session": {
        const suggested = pending.prompt.suggestions.some((rule) => rulesEqual(rule, decision.rule));
        if (!suggested || !validRule(decision.rule)) {
          throw new CommandError(
            "invalid_tool_permission_decision",
            "allow_for_session rule must exactly match a server suggestion",
          );
        }
        if (this.#rules.length >= MAX_SESSION_RULES) {
          throw new CommandError(
            "invalid_tool_permission_decision",
            "the session permission rule limit has been reached",
          );
        }
        const nextRules = [...this.#rules, structuredClone(decision.rule)];
        this.#persistRules?.(nextRules);
        this.#rules.push(structuredClone(decision.rule));
        allowed = true;
        result = undefined;
        break;
      }
      case "deny":
        allowed = false;
        result = {
          block: true,
          reason: decision.message?.trim() || "tool execution was denied by the user",
        };
        break;
    }
    this.#settle(permissionId, pending, result);
    this.#events?.resolved(permissionId, allowed);
  }

  cancelAll(reason: string): void {
    for (const permissionId of [...this.#pending.keys()]) this.#cancel(permissionId, reason);
  }

  #cancel(permissionId: string, reason: string): void {
    const pending = this.#pending.get(permissionId);
    if (pending === undefined) return;
    this.#settle(permissionId, pending, { block: true, reason });
    this.#events?.cancelled(permissionId);
  }

  #settle(
    permissionId: string,
    pending: PendingPermission,
    result: BeforeToolCallResult | undefined,
  ): void {
    this.#pending.delete(permissionId);
    if (pending.signal !== undefined && pending.abortListener !== undefined) {
      pending.signal.removeEventListener("abort", pending.abortListener);
    }
    pending.resolve(result);
  }
}

export function effectForTool(toolName: string): ToolEffect {
  switch (toolName) {
    case "read":
    case "grep":
    case "find":
    case "ls":
      return "read_only";
    case "askuser":
      return "internal";
    case "edit":
    case "write":
      return "workspace_write";
    default:
      return "external_side_effect";
  }
}

export function capabilityAllows(mode: CapabilityMode, effect: ToolEffect): boolean {
  if (effect === "read_only" || effect === "internal") return true;
  if (effect === "workspace_write") return mode !== "read_only";
  return mode === "full_access";
}

export function matchesPermissionPattern(pattern: string, value: string): boolean {
  const expression: string[] = ["^"];
  for (let index = 0; index < pattern.length; index += 1) {
    const character = pattern[index];
    if (character === "\\" && index + 1 < pattern.length) {
      index += 1;
      expression.push(escapeRegExp(pattern[index] ?? ""));
    } else if (character === "*") {
      expression.push("[\\s\\S]*");
    } else {
      expression.push(escapeRegExp(character ?? ""));
    }
  }
  expression.push("$");
  const direct = new RegExp(expression.join(""), "u");
  if (direct.test(value)) return true;
  if (pattern.endsWith(" *") && pattern.indexOf("*") === pattern.length - 1) {
    return matchesPermissionPattern(pattern.slice(0, -2), value);
  }
  return false;
}

function matchesRule(rule: ToolPermissionRule, toolName: string, args: unknown): boolean {
  if (rule.tool_name !== toolName) return false;
  if (rule.pattern === undefined) return true;
  const target = permissionTarget(toolName, args);
  if (Buffer.byteLength(target) > 64 * 1024) return false;
  if (toolName === "bash") {
    if (complexShellCommand(target) && hasUnescapedWildcard(rule.pattern)) return false;
    if (rule.pattern.endsWith(":*")) {
      const prefix = rule.pattern.slice(0, -2);
      return !complexShellCommand(target) && (target === prefix || target.startsWith(`${prefix} `));
    }
  }
  return matchesPermissionPattern(rule.pattern, target);
}

function suggestionsFor(toolName: string, args: unknown): ToolPermissionRule[] {
  const target = permissionTarget(toolName, args);
  const exact: ToolPermissionRule = { tool_name: toolName, pattern: escapePattern(target) };
  if (toolName !== "bash" || complexShellCommand(target)) return [exact];
  const reusable = reusableBashPrefix(target);
  if (reusable === undefined) return [exact];
  const prefix: ToolPermissionRule = { tool_name: toolName, pattern: `${reusable} *` };
  return rulesEqual(prefix, exact) ? [exact] : [prefix, exact];
}

function reusableBashPrefix(command: string): string | undefined {
  const tokens = command.trim().split(/\s+/u);
  const executable = tokens[0];
  const subcommand = tokens[1];
  if (
    executable === undefined ||
    subcommand === undefined ||
    !/^[A-Za-z0-9_.-]+$/u.test(executable) ||
    shellWrapper(executable) ||
    !/^[a-z][a-z0-9-]*$/u.test(subcommand)
  ) {
    return undefined;
  }
  return `${executable} ${subcommand}`;
}

function permissionTarget(toolName: string, args: unknown): string {
  if (toolName === "bash" && isObject(args) && typeof args.command === "string") {
    return args.command;
  }
  return JSON.stringify(args ?? {});
}

function escapePattern(value: string): string {
  return value.replace(/\\/gu, "\\\\").replace(/\*/gu, "\\*");
}

function complexShellCommand(command: string): boolean {
  return /[;&|<>`\n\r]|\$\(/u.test(command);
}

function hasUnescapedWildcard(pattern: string): boolean {
  let escaped = false;
  for (const character of pattern) {
    if (escaped) {
      escaped = false;
    } else if (character === "\\") {
      escaped = true;
    } else if (character === "*") {
      return true;
    }
  }
  return false;
}

function shellWrapper(executable: string): boolean {
  return [
    "sh",
    "bash",
    "zsh",
    "fish",
    "csh",
    "tcsh",
    "ksh",
    "dash",
    "cmd",
    "powershell",
    "pwsh",
    "env",
    "xargs",
    "nice",
    "stdbuf",
    "nohup",
    "timeout",
    "time",
    "sudo",
    "doas",
    "pkexec",
    "command",
    "builtin",
    "eval",
    "exec",
    "source",
    ".",
    "python",
    "python3",
    "node",
    "ruby",
    "perl",
    "php",
    "lua",
    "deno",
    "osascript",
  ].includes(executable);
}

function rulesEqual(left: ToolPermissionRule, right: ToolPermissionRule): boolean {
  return left.tool_name === right.tool_name && left.pattern === right.pattern;
}

function validRule(rule: ToolPermissionRule): boolean {
  return (
    typeof rule.tool_name === "string" &&
    rule.tool_name.trim() === rule.tool_name &&
    rule.tool_name.length > 0 &&
    Buffer.byteLength(rule.tool_name) <= 256 &&
    (rule.pattern === undefined ||
      (typeof rule.pattern === "string" &&
        rule.pattern.trim().length > 0 &&
        Buffer.byteLength(rule.pattern) <= 4096))
  );
}

export function normalizePermissionRules(value: unknown): ToolPermissionRule[] {
  if (!Array.isArray(value)) return [];
  const rules: ToolPermissionRule[] = [];
  for (const candidate of value) {
    if (!isObject(candidate)) continue;
    if (!Object.keys(candidate).every((key) => key === "tool_name" || key === "pattern")) {
      continue;
    }
    const rule: ToolPermissionRule = {
      tool_name: typeof candidate.tool_name === "string" ? candidate.tool_name : "",
      ...(typeof candidate.pattern === "string" ? { pattern: candidate.pattern } : {}),
    };
    if (!validRule(rule) || rules.some((existing) => rulesEqual(existing, rule))) continue;
    rules.push(rule);
    if (rules.length === MAX_SESSION_RULES) break;
  }
  return rules;
}

function escapeRegExp(value: string): string {
  return value.replace(/[.*+?^${}()|[\]\\]/gu, "\\$&");
}

function isObject(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function resolveToolPath(path: string, workspace: string): string {
  const stripped = path.startsWith("@") ? path.slice(1) : path;
  if (stripped === "~") return homedir();
  if (stripped.startsWith("~/") || stripped.startsWith("~\\")) {
    return resolve(homedir(), stripped.slice(2));
  }
  return isAbsolute(stripped) ? resolve(stripped) : resolve(workspace, stripped);
}

async function canonicalPotentialPath(path: string, depth = 0): Promise<string> {
  if (depth > 64) throw new Error("too many symbolic links");
  try {
    return await realpath(path);
  } catch (error) {
    if (!isNodeError(error, "ENOENT")) throw error;
  }
  try {
    const metadata = await lstat(path);
    if (!metadata.isSymbolicLink()) throw new Error("path could not be canonicalized");
    const target = await readlink(path);
    const resolvedTarget = isAbsolute(target) ? target : resolve(dirname(path), target);
    return canonicalPotentialPath(resolvedTarget, depth + 1);
  } catch (error) {
    if (!isNodeError(error, "ENOENT")) throw error;
  }
  const parent = dirname(path);
  if (parent === path) throw new Error("path has no existing ancestor");
  return resolve(await canonicalPotentialPath(parent, depth + 1), basename(path));
}

function isWithin(workspace: string, candidate: string): boolean {
  const child = relative(workspace, candidate);
  const parentPrefix = `..${process.platform === "win32" ? "\\" : "/"}`;
  return (
    child === "" ||
    (!child.startsWith(parentPrefix) && child !== ".." && !isAbsolute(child))
  );
}

function isNodeError(error: unknown, code: string): error is NodeJS.ErrnoException {
  return error instanceof Error && "code" in error && error.code === code;
}
