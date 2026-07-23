import { join } from "node:path";

import type { Model } from "@earendil-works/pi-ai";
import {
  ModelRuntime,
  SettingsManager,
} from "@earendil-works/pi-coding-agent";

import { ControlStore, type StoredProviderProfile } from "./control-store.js";
import { ApiError } from "./errors.js";
import {
  providerKindForApi,
  reasoningToThinking,
  thinkingToReasoning,
} from "./projection.js";
import type {
  ProviderResponse,
  ProvidersResponse,
  PublicProviderConfig,
  PutProviderRequest,
  ReasoningEffort,
} from "./protocol.js";

const DEFAULT_MAX_RETRIES = 10;
const DEFAULT_REQUEST_TIMEOUT_SECS = 30;
const DEFAULT_STREAM_IDLE_TIMEOUT_SECS = 120;

export interface ProfileRuntime {
  runtime: ModelRuntime;
  settings: SettingsManager;
  effectiveProviderId: string;
  storedProfile?: StoredProviderProfile;
}

export class ProviderManager {
  readonly #agentDir: string;
  readonly #workspace: string;
  readonly #store: ControlStore;

  constructor(agentDir: string, workspace: string, store: ControlStore) {
    this.#agentDir = agentDir;
    this.#workspace = workspace;
    this.#store = store;
  }

  async list(): Promise<ProvidersResponse> {
    const profiles = await this.#store.listProviderProfiles();
    const storedIds = new Set(profiles.map((profile) => profile.profile_id));
    const runtime = await this.#newRuntime(profiles);
    const settings = SettingsManager.create(this.#workspace, this.#agentDir);
    const providers: PublicProviderConfig[] = profiles.map(publicStoredProvider);

    for (const provider of runtime.getProviders()) {
      if (storedIds.has(provider.id)) continue;
      const model = selectProviderModel(runtime, settings, provider.id);
      if (model === undefined) continue;
      const retry = settings.getRetrySettings();
      const providerRetry = settings.getProviderRetrySettings();
      providers.push({
        profile_id: provider.id,
        provider: providerKindForApi(model.api),
        api_key_configured: runtime.hasConfiguredAuth(provider.id),
        base_url: provider.baseUrl ?? model.baseUrl,
        model: model.id,
        system_prompt: null,
        max_output_tokens: model.maxTokens,
        max_context_tokens: model.contextWindow,
        temperature: null,
        reasoning_effort: model.reasoning
          ? thinkingToReasoning(settings.getDefaultThinkingLevel() ?? "medium")
          : null,
        max_retries: providerRetry.maxRetries ?? retry.maxRetries,
        request_timeout_secs: Math.max(1, Math.round((providerRetry.timeoutMs ?? 600_000) / 1000)),
        stream_idle_timeout_secs: Math.max(1, Math.round(settings.getHttpIdleTimeoutMs() / 1000)),
        revision: 0,
      });
    }
    providers.sort((left, right) => left.profile_id.localeCompare(right.profile_id));
    return { providers };
  }

  async get(profileId: string): Promise<ProviderResponse> {
    let stored: StoredProviderProfile | undefined;
    try {
      stored = await this.#store.getProviderProfile(profileId);
    } catch (error) {
      throw new ApiError(400, "invalid_provider_config", errorMessage(error));
    }
    if (stored !== undefined) return { configured: true, provider: publicStoredProvider(stored) };

    const runtime = await this.#newRuntime([]);
    const settings = SettingsManager.create(this.#workspace, this.#agentDir);
    const effectiveId =
      profileId === "default"
        ? settings.getDefaultProvider() ?? (await runtime.getAvailable())[0]?.provider
        : profileId;
    if (effectiveId === undefined) return { configured: false, provider: null };
    const provider = runtime.getProvider(effectiveId);
    const model = selectProviderModel(runtime, settings, effectiveId);
    if (provider === undefined || model === undefined) return { configured: false, provider: null };
    const retry = settings.getRetrySettings();
    const providerRetry = settings.getProviderRetrySettings();
    return {
      configured: true,
      provider: {
        profile_id: profileId,
        provider: providerKindForApi(model.api),
        api_key_configured: runtime.hasConfiguredAuth(effectiveId),
        base_url: provider.baseUrl ?? model.baseUrl,
        model: model.id,
        system_prompt: null,
        max_output_tokens: model.maxTokens,
        max_context_tokens: model.contextWindow,
        temperature: null,
        reasoning_effort: model.reasoning
          ? thinkingToReasoning(settings.getDefaultThinkingLevel() ?? "medium")
          : null,
        max_retries: providerRetry.maxRetries ?? retry.maxRetries,
        request_timeout_secs: Math.max(1, Math.round((providerRetry.timeoutMs ?? 600_000) / 1000)),
        stream_idle_timeout_secs: Math.max(1, Math.round(settings.getHttpIdleTimeoutMs() / 1000)),
        revision: 0,
      },
    };
  }

  async put(profileId: string, request: PutProviderRequest): Promise<ProviderResponse> {
    let profile: StoredProviderProfile;
    try {
      profile = await this.#store.putProviderProfile(profileId, withProviderDefaults(request));
      await this.#newRuntime(await this.#store.listProviderProfiles());
    } catch (error) {
      throw new ApiError(400, "invalid_provider_config", errorMessage(error));
    }
    return { configured: true, provider: publicStoredProvider(profile) };
  }

  async createRuntime(profileId: string, workspace: string): Promise<ProfileRuntime> {
    const profiles = await this.#store.listProviderProfiles();
    const runtime = await this.#newRuntime(profiles);
    const settings = SettingsManager.create(workspace, this.#agentDir);
    const storedProfile = profiles.find((profile) => profile.profile_id === profileId);
    if (storedProfile !== undefined) applyProfileRuntimeSettings(settings, storedProfile);
    const effectiveProviderId =
      storedProfile?.profile_id ??
      (profileId === "default"
        ? settings.getDefaultProvider() ?? (await runtime.getAvailable())[0]?.provider
        : profileId);
    if (effectiveProviderId === undefined || runtime.getProvider(effectiveProviderId) === undefined) {
      throw new Error(`provider profile \`${profileId}\` is not configured in ${this.#agentDir}`);
    }
    return {
      runtime,
      settings,
      effectiveProviderId,
      ...(storedProfile === undefined ? {} : { storedProfile }),
    };
  }

  async resolveInitialModel(
    profile: ProfileRuntime,
    requested?: string | null,
  ): Promise<Model<any>> {
    const storedModel = profile.storedProfile?.model;
    const configured = requested?.trim() || storedModel;
    if (configured) {
      const model = resolveModel(profile.runtime, profile.effectiveProviderId, configured);
      if (model !== undefined) return model;
      throw new Error(`model \`${configured}\` is not available`);
    }
    const settingsProvider = profile.settings.getDefaultProvider();
    const settingsModel = profile.settings.getDefaultModel();
    if (settingsProvider && settingsModel) {
      const model = profile.runtime.getModel(settingsProvider, settingsModel);
      if (model !== undefined) return model;
    }
    const models = profile.runtime.getModels(profile.effectiveProviderId);
    const model = models[0];
    if (model === undefined) throw new Error(`provider \`${profile.effectiveProviderId}\` has no models`);
    return model;
  }

  async ensureModelAuth(runtime: ModelRuntime, model: Model<any>): Promise<void> {
    const auth = await runtime.getAuth(model);
    if (auth === undefined) throw new Error(`authentication is not configured for provider \`${model.provider}\``);
  }

  #newRuntime(profiles: readonly StoredProviderProfile[]): Promise<ModelRuntime> {
    return ModelRuntime.create({
      authPath: join(this.#agentDir, "auth.json"),
      modelsPath: join(this.#agentDir, "models.json"),
    }).then((runtime) => {
      for (const profile of profiles) registerStoredProvider(runtime, profile);
      return runtime;
    });
  }
}

function applyProfileRuntimeSettings(
  settings: SettingsManager,
  profile: StoredProviderProfile,
): void {
  const originalProviderRetry = settings.getProviderRetrySettings.bind(settings);
  settings.getProviderRetrySettings = () => ({
    ...originalProviderRetry(),
    timeoutMs: (profile.request_timeout_secs ?? DEFAULT_REQUEST_TIMEOUT_SECS) * 1_000,
    maxRetries: profile.max_retries ?? DEFAULT_MAX_RETRIES,
  });
  settings.getHttpIdleTimeoutMs = () =>
    (profile.stream_idle_timeout_secs ?? DEFAULT_STREAM_IDLE_TIMEOUT_SECS) * 1_000;
}

export function resolveModel(
  runtime: ModelRuntime,
  currentProvider: string,
  requested: string,
): Model<any> | undefined {
  const normalized = requested.trim();
  const current = runtime.getModel(currentProvider, normalized);
  if (current !== undefined) return current;
  const separator = normalized.indexOf("/");
  if (separator > 0) {
    const provider = normalized.slice(0, separator);
    const modelId = normalized.slice(separator + 1);
    const qualified = runtime.getModel(provider, modelId);
    if (qualified !== undefined) return qualified;
  }
  const matches = runtime.getModels().filter((model) => model.id === normalized);
  return matches.length === 1 ? matches[0] : undefined;
}

function registerStoredProvider(runtime: ModelRuntime, profile: StoredProviderProfile): void {
  const api = providerApi(profile.provider);
  runtime.registerProvider(profile.profile_id, {
    name: profile.profile_id,
    baseUrl: profile.base_url,
    api,
    apiKey: profile.api_key,
    models: [
      {
        id: profile.model,
        name: profile.model,
        api,
        // A Phi profile describes request defaults, not model capabilities. Keep
        // reasoning available so a later set_reasoning_effort command can opt in.
        reasoning: true,
        input: ["text", "image"],
        cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
        contextWindow: profile.max_context_tokens,
        maxTokens: profile.max_output_tokens ?? 16_384,
      },
    ],
  });
}

function publicStoredProvider(profile: StoredProviderProfile): PublicProviderConfig {
  return {
    profile_id: profile.profile_id,
    provider: profile.provider,
    api_key_configured: true,
    base_url: profile.base_url,
    model: profile.model,
    system_prompt: null,
    max_output_tokens: profile.max_output_tokens ?? null,
    max_context_tokens: profile.max_context_tokens,
    temperature: profile.temperature ?? null,
    reasoning_effort: profile.reasoning_effort ?? null,
    max_retries: profile.max_retries ?? DEFAULT_MAX_RETRIES,
    request_timeout_secs: profile.request_timeout_secs ?? DEFAULT_REQUEST_TIMEOUT_SECS,
    stream_idle_timeout_secs:
      profile.stream_idle_timeout_secs ?? DEFAULT_STREAM_IDLE_TIMEOUT_SECS,
    revision: profile.revision,
  };
}

function selectProviderModel(
  runtime: ModelRuntime,
  settings: SettingsManager,
  providerId: string,
): Model<any> | undefined {
  if (settings.getDefaultProvider() === providerId) {
    const defaultModel = settings.getDefaultModel();
    if (defaultModel) {
      const model = runtime.getModel(providerId, defaultModel);
      if (model !== undefined) return model;
    }
  }
  return runtime.getModels(providerId)[0];
}

function providerApi(provider: PutProviderRequest["provider"]): string {
  switch (provider) {
    case "anthropic":
      return "anthropic-messages";
    case "openai_responses":
      return "openai-responses";
    case "openai_chat":
      return "openai-completions";
  }
}

function withProviderDefaults(request: PutProviderRequest): PutProviderRequest {
  return {
    ...request,
    model: request.model.trim(),
    api_key: request.api_key.trim(),
    base_url: request.base_url.trim().replace(/\/$/, ""),
    system_prompt: null,
    max_output_tokens: request.max_output_tokens ?? null,
    temperature: request.temperature ?? null,
    reasoning_effort: normalizeReasoning(request.reasoning_effort),
    max_retries: request.max_retries ?? DEFAULT_MAX_RETRIES,
    request_timeout_secs: request.request_timeout_secs ?? DEFAULT_REQUEST_TIMEOUT_SECS,
    stream_idle_timeout_secs:
      request.stream_idle_timeout_secs ?? DEFAULT_STREAM_IDLE_TIMEOUT_SECS,
  };
}

function normalizeReasoning(value: ReasoningEffort | null | undefined): ReasoningEffort | null {
  if (value === undefined) return null;
  if (value === "none") return thinkingToReasoning(reasoningToThinking(value));
  return value;
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
