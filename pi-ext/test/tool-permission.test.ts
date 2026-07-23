import type { BeforeToolCallContext } from "@earendil-works/pi-agent-core";
import { describe, expect, it, vi } from "vitest";

import { CommandError } from "../src/errors.js";
import {
  matchesPermissionPattern,
  normalizePermissionRules,
  ToolPermissionBroker,
} from "../src/tool-permission.js";

describe("ToolPermissionBroker", () => {
  it("keeps over-capability calls pending until an attached client decides", async () => {
    const broker = new ToolPermissionBroker("workspace_edit");
    const requested = vi.fn();
    const resolved = vi.fn();
    broker.bind({ requested, resolved, cancelled: vi.fn() });

    const decision = broker.authorize(toolCall("bash", { command: "git status" }));
    expect(requested).toHaveBeenCalledOnce();
    const prompt = broker.listPending()[0];
    expect(prompt).toMatchObject({ effect: "external_side_effect", capability_mode: "workspace_edit" });
    if (prompt === undefined) throw new Error("permission prompt was not created");

    broker.decide(prompt.permission_id, { type: "allow_once" });
    await expect(decision).resolves.toBeUndefined();
    expect(resolved).toHaveBeenCalledWith(prompt.permission_id, true);
  });

  it("validates remembered rules against server suggestions", async () => {
    const broker = new ToolPermissionBroker("read_only");
    broker.bind({ requested: vi.fn(), resolved: vi.fn(), cancelled: vi.fn() });
    const decision = broker.authorize(toolCall("write", { path: "a.txt", content: "x" }));
    const prompt = broker.listPending()[0];
    if (prompt === undefined) throw new Error("permission prompt was not created");

    expect(() =>
      broker.decide(prompt.permission_id, {
        type: "allow_for_session",
        rule: { tool_name: "write" },
      }),
    ).toThrowError(CommandError);
    expect(broker.listPending()).toHaveLength(1);

    const rule = prompt.suggestions[0];
    if (rule === undefined) throw new Error("permission suggestion was not created");
    broker.decide(prompt.permission_id, { type: "allow_for_session", rule });
    await expect(decision).resolves.toBeUndefined();
    await expect(
      broker.authorize(toolCall("write", { path: "a.txt", content: "x" })),
    ).resolves.toBeUndefined();
  });

  it("uses anchored wildcard matching", () => {
    expect(matchesPermissionPattern("git *", "git")).toBe(true);
    expect(matchesPermissionPattern("git *", "git status")).toBe(true);
    expect(matchesPermissionPattern("git *", "github")).toBe(false);
    expect(matchesPermissionPattern("echo \\*", "echo *")).toBe(true);
    expect(matchesPermissionPattern("echo \\*", "echo value")).toBe(false);
  });

  it("does not let remembered Bash wildcards authorize compound commands", async () => {
    const broker = new ToolPermissionBroker("workspace_edit");
    broker.bind({ requested: vi.fn(), resolved: vi.fn(), cancelled: vi.fn() });
    const first = broker.authorize(toolCall("bash", { command: "git status" }));
    const firstPrompt = broker.listPending()[0];
    const prefix = firstPrompt?.suggestions.find(({ pattern }) => pattern === "git status *");
    if (firstPrompt === undefined || prefix === undefined) throw new Error("Bash prefix was not suggested");
    broker.decide(firstPrompt.permission_id, { type: "allow_for_session", rule: prefix });
    await first;

    const compound = broker.authorize(
      toolCall("bash", { command: "git status && echo should-not-auto-run" }),
    );
    const compoundPrompt = broker.listPending()[0];
    expect(compoundPrompt).toBeDefined();
    if (compoundPrompt === undefined) throw new Error("compound command was not held for approval");
    broker.decide(compoundPrompt.permission_id, { type: "deny" });
    await expect(compound).resolves.toMatchObject({ block: true });
  });

  it("only suggests conservative reusable Bash subcommands", async () => {
    const broker = new ToolPermissionBroker("workspace_edit");
    broker.bind({ requested: vi.fn(), resolved: vi.fn(), cancelled: vi.fn() });

    void broker.authorize(toolCall("bash", { command: "git status --short" }));
    expect(broker.listPending()[0]?.suggestions.map(({ pattern }) => pattern)).toEqual([
      "git status *",
      "git status --short",
    ]);
    broker.cancelAll("test complete");

    void broker.authorize(toolCall("bash", { command: "rm -rf build" }));
    expect(broker.listPending()[0]?.suggestions.map(({ pattern }) => pattern)).toEqual([
      "rm -rf build",
    ]);
    broker.cancelAll("test complete");
  });

  it("fails closed without creating approval prompts for unattended sessions", async () => {
    const broker = new ToolPermissionBroker("workspace_edit");
    const requested = vi.fn();
    broker.bind({ requested, resolved: vi.fn(), cancelled: vi.fn() });
    broker.disablePrompts();

    await expect(
      broker.authorize(toolCall("bash", { command: "git status" })),
    ).resolves.toMatchObject({
      block: true,
      reason: expect.stringContaining("unattended session capability"),
    });
    expect(requested).not.toHaveBeenCalled();
    expect(broker.listPending()).toEqual([]);
  });

  it("persists validated allow-for-session rules before releasing the tool", async () => {
    let persisted: readonly import("../src/protocol.js").ToolPermissionRule[] = [];
    const broker = new ToolPermissionBroker("workspace_edit", {
      persistRules: (rules) => {
        persisted = structuredClone(rules);
      },
    });
    const decision = broker.authorize(toolCall("bash", { command: "git status" }));
    const prompt = broker.listPending()[0];
    const rule = prompt?.suggestions.find(({ pattern }) => pattern === "git status *");
    if (prompt === undefined || rule === undefined) throw new Error("permission rule was not suggested");

    broker.decide(prompt.permission_id, { type: "allow_for_session", rule });
    await expect(decision).resolves.toBeUndefined();
    expect(persisted).toEqual([rule]);

    const restored = new ToolPermissionBroker("workspace_edit", {
      rules: normalizePermissionRules(persisted),
    });
    await expect(
      restored.authorize(toolCall("bash", { command: "git status --short" })),
    ).resolves.toBeUndefined();
    expect(restored.listPending()).toEqual([]);
  });

  it("blocks built-in file tools that escape a restricted session workspace", async () => {
    const broker = new ToolPermissionBroker("workspace_edit", { workspace: process.cwd() });

    await expect(
      broker.authorize(toolCall("write", { path: "../outside.txt", content: "x" })),
    ).resolves.toMatchObject({
      block: true,
      reason: expect.stringContaining("escapes the session workspace"),
    });
    expect(broker.listPending()).toEqual([]);
  });
});

function toolCall(name: string, args: unknown): BeforeToolCallContext {
  return {
    toolCall: { type: "toolCall", id: "call-1", name, arguments: args },
    args,
    assistantMessage: {},
    context: {},
  } as unknown as BeforeToolCallContext;
}
