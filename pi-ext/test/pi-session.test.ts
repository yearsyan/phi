import { mkdtemp, mkdir, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import type { Model } from "@earendil-works/pi-ai";
import { afterEach, describe, expect, it } from "vitest";

import { ControlStore, defaultAgentProfile } from "../src/control-store.js";
import type { StoredProviderProfile } from "../src/control-store.js";
import { initialReasoning, PiSessionFactory } from "../src/pi-session.js";
import { ProviderManager, type ProfileRuntime } from "../src/provider-manager.js";

const temporaryDirectories: string[] = [];

afterEach(async () => {
  await Promise.all(
    temporaryDirectories.splice(0).map((path) => rm(path, { recursive: true, force: true })),
  );
});

describe("initialReasoning", () => {
  it("uses Pi's medium default and clamps it to model capabilities", () => {
    const runtime = {
      settings: { getDefaultThinkingLevel: () => undefined },
    } as unknown as ProfileRuntime;

    expect(initialReasoning(model(true), undefined, defaultAgentProfile(), runtime)).toEqual({
      thinking: "medium",
      effort: "medium",
    });
    expect(initialReasoning(model(false), undefined, defaultAgentProfile(), runtime)).toEqual({
      thinking: "off",
      effort: "none",
    });
  });

  it("keeps an unset Phi compatibility profile distinct from Pi's default", () => {
    const runtime = {
      settings: { getDefaultThinkingLevel: () => "medium" },
    } as unknown as ProfileRuntime;
    const provider = { reasoning_effort: null } as unknown as StoredProviderProfile;

    expect(initialReasoning(model(true), provider, defaultAgentProfile(), runtime)).toEqual({
      thinking: "off",
      effort: null,
    });
  });

  it("activates a real Pi AgentSession from an isolated Pi config directory", async () => {
    const root = await mkdtemp(join(tmpdir(), "pi-ext-sdk-"));
    temporaryDirectories.push(root);
    const agentDir = join(root, "agent");
    const workspace = join(root, "workspace");
    await mkdir(workspace);
    const store = new ControlStore(join(agentDir, "daemon"));
    await store.putProviderProfile("default", {
      provider: "openai_chat",
      api_key: "test-key",
      base_url: "http://127.0.0.1:9/v1",
      model: "test-model",
      max_context_tokens: 16_384,
      reasoning_effort: null,
    });
    const providers = new ProviderManager(agentDir, workspace, store);
    const factory = new PiSessionFactory(agentDir, store, providers);

    const prepared = await factory.prepare({
      profileId: "default",
      agentProfileId: "default",
      workspace,
    });
    const runtimeSession = await prepared.activate();

    expect(runtimeSession.model.id).toBe("test-model");
    expect(runtimeSession.thinkingLevel).toBe("off");
    expect(runtimeSession.allAllowedTools).toEqual(
      expect.arrayContaining(["read", "bash", "edit", "write", "askuser"]),
    );
    runtimeSession.dispose();

    await store.putAgentProfile("restricted", {
      tools: { allow: [], deny: ["askuser"] },
    });
    const restricted = await factory.prepare({
      profileId: "default",
      agentProfileId: "restricted",
      workspace,
    });
    const restrictedRuntime = await restricted.activate();
    expect(restrictedRuntime.allAllowedTools).toEqual(["askuser"]);
    restrictedRuntime.dispose();
  });
});

function model(reasoning: boolean): Model<any> {
  return {
    id: "test-model",
    name: "test-model",
    provider: "test-provider",
    api: "openai-completions",
    baseUrl: "http://127.0.0.1:9/v1",
    reasoning,
    input: ["text"],
    cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
    contextWindow: 16_384,
    maxTokens: 4_096,
  } as Model<any>;
}
