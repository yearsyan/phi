import { readFile } from "node:fs/promises";
import { createServer as createHttpServer } from "node:http";
import type {
  IncomingMessage,
  Server as HttpServer,
  ServerResponse,
} from "node:http";
import { createServer as createHttpsServer } from "node:https";
import type { AddressInfo } from "node:net";
import type { Duplex } from "node:stream";

import { WebSocket, WebSocketServer, type RawData } from "ws";

import { AuthManager, WS_PROTOCOL } from "./auth.js";
import type { DaemonConfig } from "./config.js";
import { ControlStore } from "./control-store.js";
import { ApiError, CommandError, errorMessage } from "./errors.js";
import { type PreparedSession, validateSkillInvocation } from "./pi-session.js";
import type {
  AgentProfileResponse,
  AgentProfilesResponse,
  CapabilityMode,
  ClientCommand,
  CreateScheduledTaskRequest,
  EventDto,
  PutAgentProfileRequest,
  PutProviderRequest,
  PublicAgentProfile,
  ServerMessage,
  UpdateScheduledTaskRequest,
} from "./protocol.js";
import {
  isCapabilityMode,
  isRecord,
  parseClientCommand,
} from "./protocol.js";
import { ProviderManager } from "./provider-manager.js";
import { ScheduledTaskManager } from "./scheduled-tasks.js";
import { ApplicationService } from "./service.js";
import { SessionActor } from "./session-actor.js";
import { browseWorkspace, resolveWorkspacePath } from "./workspace.js";

const MAX_JSON_BYTES = 1024 * 1024;
const MAX_WS_BUFFER_BYTES = 4 * 1024 * 1024;
const MAX_PENDING_WS_COMMANDS = 64;

export interface DaemonServerDependencies {
  config: DaemonConfig;
  auth: AuthManager;
  store: ControlStore;
  providers: ProviderManager;
  service: ApplicationService;
  scheduledTasks: ScheduledTaskManager;
}

/** Node HTTP/WebSocket transport for the phi-daemon v1 protocol. */
export class DaemonServer {
  readonly #config: DaemonConfig;
  readonly #auth: AuthManager;
  readonly #store: ControlStore;
  readonly #providers: ProviderManager;
  readonly #service: ApplicationService;
  readonly #scheduledTasks: ScheduledTaskManager;
  readonly #webSockets: WebSocketServer;
  #server: HttpServer | undefined;
  #closing = false;

  constructor(dependencies: DaemonServerDependencies) {
    this.#config = dependencies.config;
    this.#auth = dependencies.auth;
    this.#store = dependencies.store;
    this.#providers = dependencies.providers;
    this.#service = dependencies.service;
    this.#scheduledTasks = dependencies.scheduledTasks;
    this.#webSockets = new WebSocketServer({
      noServer: true,
      maxPayload: MAX_JSON_BYTES,
      handleProtocols: (protocols) => (protocols.has(WS_PROTOCOL) ? WS_PROTOCOL : false),
    });
  }

  get address(): AddressInfo | null {
    const address = this.#server?.address();
    return typeof address === "object" ? address : null;
  }

  async start(): Promise<AddressInfo> {
    if (this.#server !== undefined) throw new Error("daemon server is already started");
    const listener = (request: IncomingMessage, response: ServerResponse): void => {
      void this.#handleHttp(request, response);
    };
    if (this.#config.tls === undefined) {
      this.#server = createHttpServer(listener);
    } else {
      const [cert, key] = await Promise.all([
        readFile(this.#config.tls.certificateFile),
        readFile(this.#config.tls.privateKeyFile),
      ]);
      this.#server = createHttpsServer({ cert, key }, listener);
    }
    this.#server.on("upgrade", (request, socket, head) => {
      void this.#handleUpgrade(request, socket, head);
    });
    this.#server.on("clientError", (_error, socket) => {
      if (socket.writable) socket.end("HTTP/1.1 400 Bad Request\r\nConnection: close\r\n\r\n");
    });
    const server = this.#server;
    try {
      await new Promise<void>((resolve, reject) => {
        if (server === undefined) return reject(new Error("server was not created"));
        server.once("error", reject);
        server.listen(this.#config.port, this.#config.host, () => {
          server.off("error", reject);
          resolve();
        });
      });
    } catch (error) {
      this.#server = undefined;
      try {
        server?.close();
      } catch {
        // A listener that failed before binding is already closed.
      }
      throw error;
    }
    const address = this.address;
    if (address === null) throw new Error("daemon did not expose a TCP address");
    return address;
  }

  async close(): Promise<void> {
    if (this.#closing) return;
    this.#closing = true;
    for (const client of this.#webSockets.clients) client.close(1001, "daemon shutting down");
    const server = this.#server;
    this.#server = undefined;
    if (server !== undefined) {
      await new Promise<void>((resolve) => server.close(() => resolve()));
    }
    await this.#scheduledTasks.close();
    await this.#service.shutdown();
    this.#webSockets.close();
  }

  async #handleHttp(request: IncomingMessage, response: ServerResponse): Promise<void> {
    setCommonHeaders(response);
    try {
      if (this.#closing) throw new ApiError(503, "daemon_shutting_down", "daemon is shutting down");
      if (!this.#auth.authorizeRequest(request)) {
        response.setHeader("WWW-Authenticate", "Bearer");
        throw new ApiError(401, "unauthorized", "missing or invalid bearer token");
      }
      const url = requestUrl(request);
      const method = request.method ?? "GET";

      if (url.pathname === "/v1/auth/token") {
        requireNoQuery(url);
        if (method === "POST") return sendJson(response, 200, this.#auth.issueToken());
        return methodNotAllowed(response, ["POST"]);
      }
      if (url.pathname === "/v1/provider") {
        requireNoQuery(url);
        if (method === "GET") return sendJson(response, 200, await this.#providers.get("default"));
        if (method === "PUT") {
          const body = await readJsonObject<PutProviderRequest>(request);
          return sendJson(response, 200, await this.#providers.put("default", body));
        }
        return methodNotAllowed(response, ["GET", "PUT"]);
      }
      if (url.pathname === "/v1/providers") {
        requireNoQuery(url);
        if (method === "GET") return sendJson(response, 200, await this.#providers.list());
        return methodNotAllowed(response, ["GET"]);
      }

      const providerId = pathParameter(url.pathname, "/v1/providers/");
      if (providerId !== undefined) {
        requireNoQuery(url);
        if (method === "GET") return sendJson(response, 200, await this.#providers.get(providerId));
        if (method === "PUT") {
          const body = await readJsonObject<PutProviderRequest>(request);
          return sendJson(response, 200, await this.#providers.put(providerId, body));
        }
        return methodNotAllowed(response, ["GET", "PUT"]);
      }

      if (url.pathname === "/v1/agent-profiles") {
        requireNoQuery(url);
        if (method === "GET") {
          const result: AgentProfilesResponse = {
            agent_profiles: await this.#store.listAgentProfiles(),
          };
          return sendJson(response, 200, result);
        }
        return methodNotAllowed(response, ["GET"]);
      }
      const agentProfileId = pathParameter(url.pathname, "/v1/agent-profiles/");
      if (agentProfileId !== undefined) {
        requireNoQuery(url);
        if (method === "GET") {
          let profile: PublicAgentProfile | undefined;
          try {
            profile = await this.#store.getAgentProfile(agentProfileId);
          } catch (error) {
            throw new ApiError(400, "invalid_agent_profile", errorMessage(error));
          }
          const result: AgentProfileResponse = {
            configured: profile !== undefined,
            agent_profile: profile ?? null,
          };
          return sendJson(response, 200, result);
        }
        if (method === "PUT") {
          const body = await readJsonObject<PutAgentProfileRequest>(request);
          try {
            const profile = await this.#store.putAgentProfile(agentProfileId, body);
            const result: AgentProfileResponse = { configured: true, agent_profile: profile };
            return sendJson(response, 200, result);
          } catch (error) {
            throw new ApiError(400, "invalid_agent_profile", errorMessage(error));
          }
        }
        return methodNotAllowed(response, ["GET", "PUT"]);
      }

      if (url.pathname === "/v1/sessions") {
        requireNoQuery(url);
        if (method === "GET") return sendJson(response, 200, await this.#service.listSessions());
        return methodNotAllowed(response, ["GET"]);
      }
      const skillsSessionId = nestedPathParameter(url.pathname, "/v1/sessions/", "/skills");
      if (skillsSessionId !== undefined) {
        requireNoQuery(url);
        if (method === "GET") return sendJson(response, 200, await this.#service.skills(skillsSessionId));
        return methodNotAllowed(response, ["GET"]);
      }
      const forkSessionId = nestedPathParameter(url.pathname, "/v1/sessions/", "/fork");
      if (forkSessionId !== undefined) {
        requireNoQuery(url);
        if (method !== "POST") return methodNotAllowed(response, ["POST"]);
        const body = await readJsonObject<{
          message_index: number;
          position?: "after" | "before_tool_calls";
        }>(request);
        assertOnlyKeys(body, ["message_index", "position"], "invalid_session_fork");
        if (body.position !== undefined && !["after", "before_tool_calls"].includes(body.position)) {
          throw new ApiError(400, "invalid_session_fork", "position is invalid");
        }
        const result = await this.#service.forkSession(
          forkSessionId,
          body.message_index,
          body.position ?? "after",
        );
        return sendJson(response, 201, result);
      }
      const sessionId = pathParameter(url.pathname, "/v1/sessions/");
      if (sessionId !== undefined) {
        requireNoQuery(url);
        if (method === "GET") return sendJson(response, 200, await this.#service.getSession(sessionId));
        if (method === "PATCH") {
          const body = await readJsonObject<{ pinned: boolean }>(request);
          assertOnlyKeys(body, ["pinned"], "invalid_session_update");
          if (typeof body.pinned !== "boolean") {
            throw new ApiError(400, "invalid_session_update", "pinned must be a boolean");
          }
          return sendJson(response, 200, await this.#service.setPinned(sessionId, body.pinned));
        }
        if (method === "DELETE") {
          await this.#service.deleteSession(sessionId);
          return sendEmpty(response, 204);
        }
        return methodNotAllowed(response, ["GET", "PATCH", "DELETE"]);
      }

      if (url.pathname === "/v1/workspaces/browse") {
        requireQueryKeys(url, ["path"]);
        if (method !== "GET") return methodNotAllowed(response, ["GET"]);
        const requested = url.searchParams.get("path") ?? this.#config.workspaceDir;
        return sendJson(response, 200, await browseWorkspace(requested));
      }

      if (url.pathname === "/v1/scheduled-tasks") {
        requireNoQuery(url);
        if (method === "GET") {
          return sendJson(response, 200, { tasks: await this.#scheduledTasks.list() });
        }
        if (method === "POST") {
          const body = await readJsonObject<CreateScheduledTaskRequest>(request);
          assertOnlyKeys(
            body,
            ["name", "prompt", "workspace", "profile_id", "agent_profile_id", "capability_mode", "schedule"],
            "invalid_scheduled_task",
          );
          if (
            body.workspace !== undefined &&
            body.workspace !== null &&
            typeof body.workspace !== "string"
          ) {
            throw new ApiError(400, "invalid_scheduled_task", "workspace must be a string");
          }
          const workspace = await resolveWorkspacePath(body.workspace ?? this.#config.workspaceDir);
          const task = await this.#scheduledTasks.create({ ...body, workspace });
          return sendJson(response, 201, task);
        }
        return methodNotAllowed(response, ["GET", "POST"]);
      }
      const runTaskId = nestedPathParameter(url.pathname, "/v1/scheduled-tasks/", "/run");
      if (runTaskId !== undefined) {
        requireNoQuery(url);
        if (method !== "POST") return methodNotAllowed(response, ["POST"]);
        await this.#scheduledTasks.runNow(runTaskId);
        return sendEmpty(response, 202);
      }
      const taskId = pathParameter(url.pathname, "/v1/scheduled-tasks/");
      if (taskId !== undefined) {
        requireNoQuery(url);
        if (method === "GET") return sendJson(response, 200, await this.#scheduledTasks.get(taskId));
        if (method === "PATCH") {
          const body = await readJsonObject<UpdateScheduledTaskRequest>(request);
          assertOnlyKeys(body, ["enabled", "expected_revision"], "invalid_scheduled_task");
          return sendJson(response, 200, await this.#scheduledTasks.update(taskId, body));
        }
        if (method === "DELETE") {
          await this.#scheduledTasks.delete(taskId);
          return sendEmpty(response, 204);
        }
        return methodNotAllowed(response, ["GET", "PATCH", "DELETE"]);
      }
      throw new ApiError(404, "not_found", "route was not found");
    } catch (error) {
      sendApiError(response, error);
    }
  }

  async #handleUpgrade(request: IncomingMessage, socket: Duplex, head: Buffer): Promise<void> {
    try {
      if (this.#closing) throw new ApiError(503, "daemon_shutting_down", "daemon is shutting down");
      const url = requestUrl(request);
      const route = websocketRoute(url.pathname);
      if (route === undefined) throw new ApiError(404, "not_found", "WebSocket route was not found");
      if (!this.#auth.authenticateWebSocket(request.headers)) {
        throw new ApiError(401, "unauthorized", "missing or invalid WebSocket token");
      }
      if (route.type === "new") {
        requireQueryKeys(url, ["profile_id", "agent_profile_id", "capability_mode", "workspace"]);
        const capabilityText = url.searchParams.get("capability_mode");
        if (capabilityText !== null && !isCapabilityMode(capabilityText)) {
          throw new ApiError(400, "invalid_capability_mode", "capability_mode is invalid");
        }
        const workspaceText = url.searchParams.get("workspace");
        const workspace =
          workspaceText === null
            ? this.#config.workspaceDir
            : await resolveWorkspacePath(workspaceText);
        this.#acceptUpgrade(request, socket, head, (webSocket) => {
          void this.#handleNewSocket(webSocket, {
            profileId: url.searchParams.get("profile_id") ?? "default",
            agentProfileId: url.searchParams.get("agent_profile_id") ?? "default",
            workspace,
            ...(capabilityText === null ? {} : { capabilityMode: capabilityText as CapabilityMode }),
          });
        });
        return;
      }
      requireNoQuery(url);
      this.#acceptUpgrade(request, socket, head, (webSocket) => {
        if (route.type === "subagent") {
          sendSocketJson(webSocket, {
            type: "fatal_error",
            code: "subagents_disabled",
            message: "subagents are disabled for this Pi-backed session",
          });
          webSocket.close(1000);
        } else {
          void this.#handleAttachSocket(webSocket, route.sessionId);
        }
      });
    } catch (error) {
      rejectUpgrade(socket, error);
    }
  }

  #acceptUpgrade(
    request: IncomingMessage,
    socket: Duplex,
    head: Buffer,
    accepted: (webSocket: WebSocket) => void,
  ): void {
    this.#webSockets.handleUpgrade(request, socket, head, (webSocket) => {
      this.#webSockets.emit("connection", webSocket, request);
      accepted(webSocket);
    });
  }

  async #handleNewSocket(
    webSocket: WebSocket,
    options: {
      profileId: string;
      agentProfileId: string;
      capabilityMode?: CapabilityMode;
      workspace: string;
    },
  ): Promise<void> {
    const commandBuffer = bufferSocketCommands(webSocket);
    sendSocketJson(webSocket, { type: "building" });
    let prepared: PreparedSession;
    try {
      prepared = await this.#service.prepare(options);
    } catch (error) {
      sendSocketJson(webSocket, {
        type: "fatal_error",
        code: "agent_build_failed",
        message: errorMessage(error),
      });
      webSocket.close(1011);
      return;
    }
    if (webSocket.readyState !== WebSocket.OPEN) return;
    sendSocketJson(webSocket, {
      type: "ready",
      config: {
        model: prepared.model.id,
        reasoning_effort: prepared.reasoningEffort,
        revision: prepared.configRevision,
      },
      capability_mode: prepared.capabilityMode,
      agent_profile: {
        agent_profile_id: prepared.agentProfile.agent_profile_id,
        revision: prepared.agentProfile.revision,
      },
      workspace: prepared.workspace,
      ...(prepared.skills.length === 0 ? {} : { skills: prepared.skills }),
    });

    let actor: SessionActor | undefined;
    let preparedSequence = 0;
    const publishPrepared = (event: EventDto): void => {
      preparedSequence += 1;
      sendSocketJson(webSocket, {
        type: "event",
        sequence: preparedSequence,
        session_id: prepared.id,
        event,
      });
    };
    let unsubscribe: (() => void) | undefined;
    const bindActor = (created: SessionActor): void => {
      actor = created;
      unsubscribe = bindActorEvents(webSocket, created);
    };
    commandBuffer.bind(async (command) => {
      if (actor === undefined) {
        if (command.type === "prompt") {
          try {
            validateSkillInvocation(prepared.skills, command.skill);
            const created = await this.#service.activate(prepared, preparedSequence);
            bindActor(created);
            sendSocketJson(webSocket, { type: "session_created", session_id: created.id });
            const queued = created.enqueueInitial(command.content, command.skill);
            this.#service.scheduleTitle(created, command.content, command.skill);
            return acceptCommand(webSocket, command, queued.runId, queued.position);
          } catch (error) {
            if (error instanceof CommandError) {
              return rejectCommand(webSocket, command.request_id, error.code, error.message);
            }
            return rejectCommand(webSocket, command.request_id, "session_activation_failed", errorMessage(error));
          }
        }
        return handlePreparedCommand(webSocket, prepared, command, publishPrepared);
      }
      return handleActorCommand(webSocket, actor, command, this.#service);
    });
    webSocket.once("close", () => unsubscribe?.());
  }

  async #handleAttachSocket(webSocket: WebSocket, sessionId: string): Promise<void> {
    const commandBuffer = bufferSocketCommands(webSocket);
    let actor: SessionActor;
    try {
      actor = await this.#service.attach(sessionId);
    } catch (error) {
      sendSocketJson(webSocket, {
        type: "fatal_error",
        code: "attach_failed",
        message: errorMessage(error),
      });
      webSocket.close(1008);
      return;
    }
    if (webSocket.readyState !== WebSocket.OPEN) return;
    const unsubscribe = bindActorEvents(webSocket, actor);
    sendSocketJson(webSocket, { type: "snapshot", session: actor.snapshot() });
    commandBuffer.bind((command) =>
      handleActorCommand(webSocket, actor, command, this.#service),
    );
    webSocket.once("close", unsubscribe);
  }
}

async function handlePreparedCommand(
  webSocket: WebSocket,
  prepared: PreparedSession,
  command: ClientCommand,
  publish: (event: EventDto) => void,
): Promise<void> {
  try {
    switch (command.type) {
      case "prompt":
        throw new Error("prompt activation must be handled by the session transport");
      case "set_model":
        await prepared.setModel(command.model);
        publish({
          type: "config_changed",
          config: {
            model: prepared.model.id,
            reasoning_effort: prepared.reasoningEffort,
            revision: prepared.configRevision,
          },
        });
        return acceptCommand(webSocket, command);
      case "set_reasoning_effort":
        prepared.setReasoning(command.effort);
        publish({
          type: "config_changed",
          config: {
            model: prepared.model.id,
            reasoning_effort: prepared.reasoningEffort,
            revision: prepared.configRevision,
          },
        });
        return acceptCommand(webSocket, command);
      case "set_capability_mode":
        prepared.setCapabilityMode(command.capability_mode);
        publish({ type: "capability_mode_changed", capability_mode: prepared.capabilityMode });
        return acceptCommand(webSocket, command);
      case "stop":
        return rejectCommand(webSocket, command.request_id, "no_active_run", "session has no active run");
      case "compact":
        return rejectCommand(
          webSocket,
          command.request_id,
          "invalid_command",
          "context compaction is available only on an attached session",
        );
      case "answer_askuser":
        return rejectCommand(
          webSocket,
          command.request_id,
          "askuser_not_pending",
          `ask request \`${command.ask_id}\` is not pending`,
        );
      case "decide_tool_permission":
        return rejectCommand(
          webSocket,
          command.request_id,
          "tool_permission_not_pending",
          `tool permission \`${command.permission_id}\` is not pending`,
        );
      case "ping":
        sendSocketJson(webSocket, { type: "pong", request_id: command.request_id });
    }
  } catch (error) {
    rejectCommand(webSocket, command.request_id, "operation_failed", errorMessage(error));
  }
}

async function handleActorCommand(
  webSocket: WebSocket,
  actor: SessionActor,
  command: ClientCommand,
  service: ApplicationService,
): Promise<void> {
  try {
    switch (command.type) {
      case "prompt": {
        const queued = actor.enqueue(command.content, command.skill);
        service.scheduleTitle(actor, command.content, command.skill);
        return acceptCommand(webSocket, command, queued.runId, queued.position);
      }
      case "stop":
        actor.stop(command.run_id);
        return acceptCommand(webSocket, command, command.run_id);
      case "compact":
        actor.compact(command.instructions);
        return acceptCommand(webSocket, command);
      case "set_model":
        await actor.setModel(command.model);
        return acceptCommand(webSocket, command);
      case "set_reasoning_effort":
        await actor.setReasoning(command.effort);
        return acceptCommand(webSocket, command);
      case "set_capability_mode":
        await actor.setCapabilityMode(command.capability_mode);
        return acceptCommand(webSocket, command);
      case "answer_askuser":
        actor.answerAskUser(command.ask_id, command.answers);
        return acceptCommand(webSocket, command);
      case "decide_tool_permission":
        actor.decideToolPermission(command.permission_id, command.decision);
        return acceptCommand(webSocket, command);
      case "ping":
        sendSocketJson(webSocket, { type: "pong", request_id: command.request_id });
    }
  } catch (error) {
    const code = error instanceof CommandError ? error.code : "operation_failed";
    rejectCommand(webSocket, command.request_id, code, errorMessage(error));
  }
}

interface SocketCommandBuffer {
  bind(handler: (command: ClientCommand) => Promise<void>): void;
}

function bufferSocketCommands(webSocket: WebSocket): SocketCommandBuffer {
  const buffered: string[] = [];
  let handler: ((command: ClientCommand) => Promise<void>) | undefined;
  let chain = Promise.resolve();
  const enqueue = (text: string): void => {
    chain = chain.then(async () => {
      let command: ClientCommand;
      try {
        command = parseClientCommand(JSON.parse(text) as unknown);
      } catch (error) {
        rejectCommand(webSocket, "", "invalid_command", errorMessage(error));
        return;
      }
      try {
        await handler?.(command);
      } catch (error) {
        rejectCommand(webSocket, command.request_id, "operation_failed", errorMessage(error));
      }
    });
  };
  webSocket.on("message", (data: RawData, isBinary: boolean) => {
    if (isBinary) return;
    const text = rawText(data);
    if (handler !== undefined) {
      enqueue(text);
      return;
    }
    if (buffered.length >= MAX_PENDING_WS_COMMANDS) {
      webSocket.close(1008, "too many commands before session readiness");
      return;
    }
    buffered.push(text);
  });
  return {
    bind(nextHandler): void {
      if (handler !== undefined) throw new Error("WebSocket command buffer is already bound");
      handler = nextHandler;
      for (const text of buffered.splice(0)) enqueue(text);
    },
  };
}

function bindActorEvents(webSocket: WebSocket, actor: SessionActor): () => void {
  let skipped = 0;
  let flushTimer: NodeJS.Timeout | undefined;
  const flushResync = (): void => {
    flushTimer = undefined;
    if (skipped === 0 || webSocket.readyState !== WebSocket.OPEN) return;
    if (webSocket.bufferedAmount > MAX_WS_BUFFER_BYTES) {
      scheduleFlush();
      return;
    }
    sendSocketJson(webSocket, { type: "resync_required", skipped, session: actor.snapshot() });
    skipped = 0;
  };
  const scheduleFlush = (): void => {
    if (flushTimer !== undefined) return;
    flushTimer = setTimeout(flushResync, 25);
    flushTimer.unref();
  };
  const unsubscribe = actor.subscribe((event) => {
    if (webSocket.readyState !== WebSocket.OPEN) return;
    if (webSocket.bufferedAmount > MAX_WS_BUFFER_BYTES) {
      skipped += 1;
      scheduleFlush();
      return;
    }
    if (skipped > 0) {
      flushResync();
      return;
    }
    sendSocketJson(webSocket, event);
    if (event.event.type === "state_changed" && event.event.status === "closed") {
      webSocket.close(1000);
    }
  });
  return () => {
    if (flushTimer !== undefined) clearTimeout(flushTimer);
    unsubscribe();
  };
}

function acceptCommand(
  webSocket: WebSocket,
  command: ClientCommand,
  runId?: string,
  queuePosition?: number,
): void {
  sendSocketJson(webSocket, {
    type: "command_accepted",
    request_id: command.request_id,
    command: command.type,
    ...(runId === undefined ? {} : { run_id: runId }),
    ...(queuePosition === undefined ? {} : { queue_position: queuePosition }),
  });
}

function rejectCommand(
  webSocket: WebSocket,
  requestId: string,
  code: string,
  message: string,
): void {
  sendSocketJson(webSocket, {
    type: "command_rejected",
    request_id: requestId,
    code,
    message,
  });
}

function sendSocketJson(webSocket: WebSocket, message: ServerMessage): void {
  if (webSocket.readyState !== WebSocket.OPEN) return;
  try {
    webSocket.send(JSON.stringify(message));
  } catch {
    webSocket.close(1011);
  }
}

function rawText(data: RawData): string {
  if (typeof data === "string") return data;
  if (data instanceof ArrayBuffer) return Buffer.from(data).toString("utf8");
  if (Array.isArray(data)) return Buffer.concat(data).toString("utf8");
  return data.toString("utf8");
}

function requestUrl(request: IncomingMessage): URL {
  try {
    return new URL(request.url ?? "/", "http://localhost");
  } catch {
    throw new ApiError(400, "invalid_uri", "request URI is invalid");
  }
}

function websocketRoute(
  path: string,
):
  | { type: "new" }
  | { type: "attach"; sessionId: string }
  | { type: "subagent"; sessionId: string; agentId: string }
  | undefined {
  if (path === "/v1/ws/new") return { type: "new" };
  const subagent = /^\/v1\/ws\/attach\/([^/]+)\/subagents\/([^/]+)$/.exec(path);
  if (subagent?.[1] !== undefined && subagent[2] !== undefined) {
    return {
      type: "subagent",
      sessionId: decodePath(subagent[1]),
      agentId: decodePath(subagent[2]),
    };
  }
  const attach = /^\/v1\/ws\/attach\/([^/]+)$/.exec(path);
  if (attach?.[1] !== undefined) return { type: "attach", sessionId: decodePath(attach[1]) };
  return undefined;
}

function pathParameter(path: string, prefix: string): string | undefined {
  if (!path.startsWith(prefix)) return undefined;
  const value = path.slice(prefix.length);
  return value && !value.includes("/") ? decodePath(value) : undefined;
}

function nestedPathParameter(path: string, prefix: string, suffix: string): string | undefined {
  if (!path.startsWith(prefix) || !path.endsWith(suffix)) return undefined;
  const value = path.slice(prefix.length, -suffix.length);
  return value && !value.includes("/") ? decodePath(value) : undefined;
}

function decodePath(value: string): string {
  try {
    return decodeURIComponent(value);
  } catch {
    throw new ApiError(400, "invalid_uri", "path contains invalid percent encoding");
  }
}

async function readJsonObject<T extends object>(request: IncomingMessage): Promise<T> {
  const contentType = request.headers["content-type"];
  if (typeof contentType !== "string" || !contentType.toLowerCase().startsWith("application/json")) {
    throw new ApiError(415, "unsupported_media_type", "Content-Type must be application/json");
  }
  const chunks: Buffer[] = [];
  let length = 0;
  for await (const chunk of request) {
    const buffer = Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk as Uint8Array);
    length += buffer.length;
    if (length > MAX_JSON_BYTES) {
      throw new ApiError(413, "payload_too_large", "JSON request body is too large");
    }
    chunks.push(buffer);
  }
  let value: unknown;
  try {
    value = JSON.parse(Buffer.concat(chunks).toString("utf8"));
  } catch {
    throw new ApiError(400, "invalid_json", "request body is not valid JSON");
  }
  if (!isRecord(value)) throw new ApiError(400, "invalid_json", "request body must be an object");
  return value as T;
}

function assertOnlyKeys(value: object, keys: readonly string[], code: string): void {
  const accepted = new Set(keys);
  const unknown = Object.keys(value).find((key) => !accepted.has(key));
  if (unknown !== undefined) throw new ApiError(400, code, `unknown field \`${unknown}\``);
}

function requireNoQuery(url: URL): void {
  requireQueryKeys(url, []);
}

function requireQueryKeys(url: URL, keys: readonly string[]): void {
  const accepted = new Set(keys);
  const seen = new Set<string>();
  for (const key of url.searchParams.keys()) {
    if (!accepted.has(key)) throw new ApiError(400, "invalid_query", `unknown query field \`${key}\``);
    if (seen.has(key)) throw new ApiError(400, "invalid_query", `duplicate query field \`${key}\``);
    seen.add(key);
  }
}

function setCommonHeaders(response: ServerResponse): void {
  response.setHeader("Cache-Control", "no-store");
  response.setHeader("X-Content-Type-Options", "nosniff");
}

function sendJson(response: ServerResponse, status: number, value: unknown): void {
  if (response.headersSent || response.writableEnded) return;
  const body = JSON.stringify(value);
  response.statusCode = status;
  response.setHeader("Content-Type", "application/json; charset=utf-8");
  response.setHeader("Content-Length", Buffer.byteLength(body));
  response.end(body);
}

function sendEmpty(response: ServerResponse, status: number): void {
  if (response.headersSent || response.writableEnded) return;
  response.statusCode = status;
  response.end();
}

function sendApiError(response: ServerResponse, error: unknown): void {
  if (response.headersSent || response.writableEnded) return;
  if (error instanceof ApiError) {
    sendJson(response, error.status, error.toResponse());
    return;
  }
  sendJson(response, 500, { code: "internal_error", message: "internal server error" });
}

function methodNotAllowed(response: ServerResponse, methods: readonly string[]): void {
  response.setHeader("Allow", methods.join(", "));
  sendJson(response, 405, { code: "method_not_allowed", message: "method is not allowed" });
}

function rejectUpgrade(socket: Duplex, error: unknown): void {
  const apiError =
    error instanceof ApiError
      ? error
      : new ApiError(500, "internal_error", "internal server error");
  const body = JSON.stringify(apiError.toResponse());
  const statusText =
    apiError.status === 401
      ? "Unauthorized"
      : apiError.status === 404
        ? "Not Found"
        : apiError.status === 503
          ? "Service Unavailable"
          : apiError.status >= 500
            ? "Internal Server Error"
            : "Bad Request";
  socket.end(
    `HTTP/1.1 ${apiError.status} ${statusText}\r\n` +
      "Content-Type: application/json; charset=utf-8\r\n" +
      "Cache-Control: no-store\r\n" +
      "Connection: close\r\n" +
      `Content-Length: ${Buffer.byteLength(body)}\r\n\r\n${body}`,
  );
}
