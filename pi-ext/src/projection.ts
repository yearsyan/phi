import type { AgentMessage, ThinkingLevel } from "@earendil-works/pi-agent-core";
import type {
  AssistantMessage,
  ImageContent as PiImageContent,
  Model,
  TextContent as PiTextContent,
  ToolResultMessage,
  Usage as PiUsage,
} from "@earendil-works/pi-ai";
import {
  parseSkillBlock,
  type ResourceDiagnostic,
  type Skill,
} from "@earendil-works/pi-coding-agent";

import type {
  AssistantDraft,
  Content,
  ContentPart,
  ContextUsage,
  PublicMessage,
  PublicProviderConfig,
  ReasoningEffort,
  SkillDiagnostic,
  SkillInvocation,
  SkillSummary,
  TokenUsage,
  ToolCall,
} from "./protocol.js";
import { toJsonValue } from "./protocol.js";

export interface PiPrompt {
  text: string;
  images: PiImageContent[];
}

export function contentToPiPrompt(content: Content, skill?: SkillInvocation): PiPrompt {
  const images: PiImageContent[] = [];
  const textParts: string[] = [];
  if (content.type === "text") {
    textParts.push(content.value);
  } else {
    for (const part of content.value) {
      switch (part.type) {
        case "text":
          textParts.push(part.text);
          break;
        case "image_url": {
          const image = parseDataImage(part.image_url.url);
          if (image === undefined) {
            textParts.push(`[Image: ${part.image_url.url}]`);
          } else {
            images.push(image);
          }
          break;
        }
        case "document":
          textParts.push(projectDocument(part.document));
          break;
      }
    }
  }
  const plainText = textParts.join("\n").trim();
  if (skill === undefined) return { text: plainText, images };
  const name = skill.name.trim().replace(/^\/+/, "").replace(/^skill:/, "");
  const argumentsText = skill.arguments?.trim() || plainText;
  return {
    text: argumentsText ? `/skill:${name} ${argumentsText}` : `/skill:${name}`,
    images,
  };
}

export function projectMessage(message: AgentMessage): PublicMessage {
  switch (message.role) {
    case "user":
      return baseMessage("user", projectPiContent(message.content, true));
    case "assistant":
      return projectAssistant(message);
    case "toolResult":
      return projectToolResult(message);
    case "compactionSummary":
    case "branchSummary":
    case "custom":
    case "bashExecution":
      return internalMessage("user");
    default:
      return internalMessage("system");
  }
}

export function projectMessages(messages: readonly AgentMessage[]): PublicMessage[] {
  return messages.map(projectMessage);
}

export function projectAssistant(message: AssistantMessage): PublicMessage {
  const text = message.content
    .filter((part): part is PiTextContent => part.type === "text")
    .map((part) => part.text)
    .join("");
  const reasoning = message.content
    .filter((part) => part.type === "thinking")
    .map((part) => part.thinking)
    .join("");
  const toolCalls: ToolCall[] = message.content
    .filter((part) => part.type === "toolCall")
    .map((part) => ({
      id: part.id,
      name: part.name,
      arguments: toJsonValue(part.arguments),
    }));
  return {
    role: "assistant",
    content: text ? { type: "text", value: text } : null,
    ...(reasoning ? { reasoning } : {}),
    tool_calls: toolCalls,
    tool_call_id: null,
    tool_result_is_error: false,
  };
}

export function projectToolResult(message: ToolResultMessage): PublicMessage {
  const projected = projectPiContent(message.content);
  return {
    role: "tool",
    content: projected,
    tool_calls: [],
    tool_call_id: message.toolCallId,
    tool_result_is_error: message.isError,
    ...(message.details === undefined ? {} : { tool_result_metadata: toJsonValue(message.details) }),
  };
}

export function projectPiContent(
  content: string | readonly (PiTextContent | PiImageContent)[],
  sanitizeSkill = false,
): Content | null {
  if (typeof content === "string") {
    const text = sanitizeSkill ? publicSkillText(content) : content;
    return text ? { type: "text", value: text } : null;
  }
  const parts: ContentPart[] = content.map((part) => {
    if (part.type === "text") {
      return { type: "text", text: sanitizeSkill ? publicSkillText(part.text) : part.text };
    }
    return {
      type: "image_url",
      image_url: { url: `data:${part.mimeType};base64,${part.data}` },
    };
  });
  if (parts.length === 0) return null;
  if (parts.every((part) => part.type === "text")) {
    return {
      type: "text",
      value: parts.map((part) => (part.type === "text" ? part.text : "")).join(""),
    };
  }
  return { type: "parts", value: parts };
}

export function projectToolContent(
  content: readonly (PiTextContent | PiImageContent)[],
): { text: string; parts: ContentPart[] } {
  const parts = content.map((part): ContentPart => {
    if (part.type === "text") return { type: "text", text: part.text };
    return {
      type: "image_url",
      image_url: { url: `data:${part.mimeType};base64,${part.data}` },
    };
  });
  return {
    text: content
      .filter((part): part is PiTextContent => part.type === "text")
      .map((part) => part.text)
      .join("\n"),
    parts,
  };
}

export function emptyTokenUsage(): TokenUsage {
  return {
    input_tokens: 0,
    output_tokens: 0,
    total_tokens: 0,
    cached_input_tokens: 0,
  };
}

export function projectUsage(usage: PiUsage): TokenUsage {
  return {
    input_tokens: usage.input,
    output_tokens: usage.output,
    total_tokens: usage.totalTokens,
    cached_input_tokens: usage.cacheRead,
  };
}

export function addUsage(left: TokenUsage, right: TokenUsage): TokenUsage {
  return {
    input_tokens: left.input_tokens + right.input_tokens,
    output_tokens: left.output_tokens + right.output_tokens,
    total_tokens: left.total_tokens + right.total_tokens,
    cached_input_tokens: left.cached_input_tokens + right.cached_input_tokens,
  };
}

export function cumulativeUsage(messages: readonly AgentMessage[]): TokenUsage {
  return messages.reduce((total, message) => {
    if (message.role !== "assistant") return total;
    return addUsage(total, projectUsage(message.usage));
  }, emptyTokenUsage());
}

export function lastUsage(messages: readonly AgentMessage[]): TokenUsage | null {
  for (let index = messages.length - 1; index >= 0; index -= 1) {
    const message = messages[index];
    if (message?.role === "assistant" && message.stopReason !== "aborted") {
      const usage = projectUsage(message.usage);
      return usage.input_tokens === 0 && usage.output_tokens === 0 && usage.total_tokens === 0
        ? null
        : usage;
    }
  }
  return null;
}

export function contextUsage(
  usage: { tokens: number | null; contextWindow: number } | undefined,
): ContextUsage | null {
  if (usage === undefined || usage.tokens === null) return null;
  const used = Math.max(0, usage.tokens);
  const maximum = Math.max(0, usage.contextWindow);
  return {
    max_tokens: maximum,
    used_tokens: used,
    remaining_tokens: Math.max(0, maximum - used),
  };
}

export function thinkingToReasoning(level: ThinkingLevel): ReasoningEffort {
  return level === "off" ? "none" : level;
}

export function reasoningToThinking(effort: ReasoningEffort | null): ThinkingLevel {
  return effort === null || effort === "none" ? "off" : effort;
}

export function modelDisplayName(model: Model<any>): string {
  return model.id;
}

export function providerKindForApi(api: string): PublicProviderConfig["provider"] {
  if (api === "anthropic-messages") return "anthropic";
  if (api === "openai-responses" || api === "openai-codex-responses") {
    return "openai_responses";
  }
  return "openai_chat";
}

export function projectSkill(skill: Skill): SkillSummary {
  return {
    name: skill.name,
    description: skill.description,
    model_invocable: !skill.disableModelInvocation,
    user_invocable: true,
    source: skill.sourceInfo.source,
  };
}

export function projectSkillDiagnostic(diagnostic: ResourceDiagnostic): SkillDiagnostic {
  return {
    level: diagnostic.type === "error" ? "error" : "warning",
    code: diagnostic.type === "collision" ? "skill_collision" : "skill_diagnostic",
    message: diagnostic.message,
  };
}

export function createDraft(): AssistantDraft {
  return { reasoning: "", text: "", tool_calls: [] };
}

function baseMessage(role: PublicMessage["role"], content: Content | null): PublicMessage {
  return {
    role,
    content,
    tool_calls: [],
    tool_call_id: null,
    tool_result_is_error: false,
  };
}

function internalMessage(role: PublicMessage["role"]): PublicMessage {
  return {
    role,
    visibility: "internal",
    content: null,
    tool_calls: [],
    tool_call_id: null,
    tool_result_is_error: false,
  };
}

function parseDataImage(url: string): PiImageContent | undefined {
  const match = /^data:([^;,]+);base64,([A-Za-z0-9+/=_-]+)$/.exec(url);
  if (match === null) return undefined;
  const mimeType = match[1];
  const data = match[2];
  if (mimeType === undefined || data === undefined || !mimeType.startsWith("image/")) return undefined;
  return { type: "image", mimeType, data };
}

function publicSkillText(text: string): string {
  const skill = parseSkillBlock(text);
  if (skill === null) return text;
  return skill.userMessage ?? `/skill:${skill.name}`;
}

function projectDocument(document: {
  filename: string;
  mime_type: string;
  data: string;
}): string {
  if (document.mime_type.startsWith("text/")) {
    const decoded = decodeDocumentText(document.data);
    if (decoded !== undefined) {
      return `<document filename="${escapeAttribute(document.filename)}" mime_type="${escapeAttribute(document.mime_type)}">\n${decoded}\n</document>`;
    }
  }
  const source = /^https?:\/\//iu.test(document.data)
    ? ` url="${escapeAttribute(document.data)}"`
    : ' attachment_omitted="true"';
  return `<document filename="${escapeAttribute(document.filename)}" mime_type="${escapeAttribute(document.mime_type)}"${source} />`;
}

function decodeDocumentText(data: string): string | undefined {
  const dataUrl = /^data:[^;,]+(?:;charset=[^;,]+)?;base64,([A-Za-z0-9+/=_-]+)$/iu.exec(data);
  const encoded = dataUrl?.[1] ?? (/^[A-Za-z0-9+/=_-]+$/u.test(data) ? data : undefined);
  if (encoded === undefined) return undefined;
  try {
    return Buffer.from(encoded, "base64").toString("utf8");
  } catch {
    return undefined;
  }
}

function escapeAttribute(value: string): string {
  return value.replaceAll("&", "&amp;").replaceAll('"', "&quot;");
}
