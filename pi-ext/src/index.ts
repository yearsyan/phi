import type { AddressInfo } from "node:net";

import { AuthManager } from "./auth.js";
import {
  loadDaemonConfig,
  type DaemonConfig,
  type LoadConfigOptions,
} from "./config.js";
import { ControlStore } from "./control-store.js";
import { PiSessionFactory } from "./pi-session.js";
import { ProviderManager } from "./provider-manager.js";
import { ScheduledTaskManager } from "./scheduled-tasks.js";
import { DaemonServer } from "./server.js";
import { ApplicationService } from "./service.js";

export interface CreatePiDaemonOptions extends LoadConfigOptions {
  config?: DaemonConfig;
}

/** Fully assembled Pi-backed implementation of the phi-daemon v1 transport. */
export class PiDaemon {
  readonly config: DaemonConfig;
  readonly store: ControlStore;
  readonly providers: ProviderManager;
  readonly service: ApplicationService;
  readonly scheduledTasks: ScheduledTaskManager;
  readonly server: DaemonServer;
  #started = false;
  #closed = false;

  constructor(config: DaemonConfig) {
    this.config = config;
    this.store = new ControlStore(config.dataDir);
    this.providers = new ProviderManager(config.agentDir, config.workspaceDir, this.store);
    const factory = new PiSessionFactory(config.agentDir, this.store, this.providers);
    this.service = new ApplicationService(this.store, factory, config.workspaceDir, config.agentDir);
    this.scheduledTasks = new ScheduledTaskManager(
      this.store,
      this.service,
      this.providers,
      config.workspaceDir,
    );
    const auth = new AuthManager(config.authKey, config.wsTokenTtlMs);
    this.server = new DaemonServer({
      config,
      auth,
      store: this.store,
      providers: this.providers,
      service: this.service,
      scheduledTasks: this.scheduledTasks,
    });
  }

  async start(): Promise<AddressInfo> {
    if (this.#closed) throw new Error("Pi daemon is closed");
    if (this.#started) throw new Error("Pi daemon is already started");
    this.#started = true;
    try {
      await this.scheduledTasks.start();
      return await this.server.start();
    } catch (error) {
      await this.server.close();
      this.#started = false;
      this.#closed = true;
      throw error;
    }
  }

  async close(): Promise<void> {
    if (this.#closed) return;
    this.#closed = true;
    await this.server.close();
    this.#started = false;
  }
}

export async function createPiDaemon(options: CreatePiDaemonOptions = {}): Promise<PiDaemon> {
  const config = options.config ?? (await loadDaemonConfig(options));
  return new PiDaemon(config);
}

export * from "./auth.js";
export * from "./config.js";
export * from "./control-store.js";
export * from "./errors.js";
export * from "./pi-session.js";
export * from "./projection.js";
export * from "./protocol.js";
export * from "./provider-manager.js";
export * from "./scheduled-tasks.js";
export * from "./server.js";
export * from "./service.js";
export * from "./session-actor.js";
export * from "./tool-permission.js";
export * from "./workspace.js";
