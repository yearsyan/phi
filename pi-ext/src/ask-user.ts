import { defineTool, type ToolDefinition } from "@earendil-works/pi-coding-agent";
import { uuidv7 } from "@earendil-works/pi-ai";
import { Type } from "typebox";

import { CommandError } from "./errors.js";
import type { AskUserAnswer, AskUserQuestion, AskUserRequest } from "./protocol.js";

const AskUserParameters = Type.Object({
  questions: Type.Array(
    Type.Object({
      question: Type.String({ minLength: 1 }),
      header: Type.String({ minLength: 1, maxLength: 12 }),
      options: Type.Array(
        Type.Object({
          label: Type.String({ minLength: 1 }),
          description: Type.String({ minLength: 1 }),
          preview: Type.Optional(Type.String({ minLength: 1 })),
        }),
        { minItems: 2, maxItems: 4 },
      ),
      multiSelect: Type.Optional(Type.Boolean({ default: false })),
    }),
    { minItems: 1, maxItems: 3 },
  ),
});
const MAX_PENDING_ASKS = 64;

interface PendingAsk {
  request: AskUserRequest;
  resolve: (answer: string) => void;
  reject: (error: Error) => void;
  removeAbortListener: () => void;
}

export interface AskUserBrokerEvents {
  requested(request: AskUserRequest): void;
  answered(askId: string): void;
  cancelled(askId: string): void;
}

export class AskUserBroker {
  readonly #pending = new Map<string, PendingAsk>();
  #events: AskUserBrokerEvents | undefined;

  bind(events: AskUserBrokerEvents): void {
    this.#events = events;
  }

  createTool(): ToolDefinition {
    return defineTool({
      name: "askuser",
      label: "Ask user",
      description:
        "Ask the user for one to three decisions that are genuinely theirs to make. The request remains pending across client reconnects.",
      parameters: AskUserParameters,
      execute: async (_toolCallId, parameters, signal) => {
        const questions = normalizeQuestions(parameters.questions);
        const answer = await this.#request(questions, signal);
        return {
          content: [{ type: "text", text: answer }],
          details: {},
        };
      },
    });
  }

  listPending(): AskUserRequest[] {
    return [...this.#pending.values()].map(({ request }) => structuredClone(request));
  }

  answer(askId: string, answers: AskUserAnswer[]): void {
    const pending = this.#pending.get(askId);
    if (pending === undefined) {
      throw new CommandError("askuser_not_pending", `askuser request \`${askId}\` is not pending`);
    }
    const normalized = validateAnswers(pending.request.questions, answers);
    this.#pending.delete(askId);
    pending.removeAbortListener();
    pending.resolve(formatAnswers(pending.request.questions, normalized));
    this.#events?.answered(askId);
  }

  cancelAll(reason = "askuser request was cancelled"): void {
    for (const askId of [...this.#pending.keys()]) this.#cancel(askId, reason);
  }

  #request(questions: AskUserQuestion[], signal?: AbortSignal): Promise<string> {
    if (signal?.aborted) return Promise.reject(new Error("askuser request was cancelled"));
    if (this.#pending.size >= MAX_PENDING_ASKS) {
      return Promise.reject(new Error("too many askuser requests are pending"));
    }
    const askId = uuidv7();
    const request: AskUserRequest = { ask_id: askId, questions };
    return new Promise<string>((resolve, reject) => {
      const onAbort = (): void => this.#cancel(askId, "askuser request was cancelled");
      signal?.addEventListener("abort", onAbort, { once: true });
      this.#pending.set(askId, {
        request,
        resolve,
        reject,
        removeAbortListener: () => signal?.removeEventListener("abort", onAbort),
      });
      this.#events?.requested(structuredClone(request));
    });
  }

  #cancel(askId: string, reason: string): void {
    const pending = this.#pending.get(askId);
    if (pending === undefined) return;
    this.#pending.delete(askId);
    pending.removeAbortListener();
    pending.reject(new Error(reason));
    this.#events?.cancelled(askId);
  }
}

function normalizeQuestions(
  questions: Array<Omit<AskUserQuestion, "multiSelect"> & { multiSelect?: boolean }>,
): AskUserQuestion[] {
  if (questions.length < 1 || questions.length > 3) {
    throw new Error("askuser requires between 1 and 3 questions");
  }
  return questions.map((question, index) => {
    const normalized: AskUserQuestion = {
      question: question.question,
      header: question.header,
      options: question.options.map((option) => ({
        label: option.label,
        description: option.description,
        ...(option.preview === undefined ? {} : { preview: option.preview }),
      })),
      multiSelect: question.multiSelect ?? false,
    };
    if (!normalized.question.trim()) throw new Error(`question ${index} must not be empty`);
    const headerLength = Array.from(normalized.header).length;
    if (headerLength < 1 || headerLength > 12) {
      throw new Error(`question ${index} header must contain between 1 and 12 characters`);
    }
    if (normalized.options.length < 2 || normalized.options.length > 4) {
      throw new Error(`question ${index} requires between 2 and 4 options`);
    }
    const labels = new Set<string>();
    for (const option of normalized.options) {
      if (!option.label.trim() || !option.description.trim()) {
        throw new Error(`question ${index} option labels and descriptions must not be empty`);
      }
      if (option.preview === "") {
        throw new Error(`question ${index} option previews must not be empty`);
      }
      if (labels.has(option.label)) {
        throw new Error(`question ${index} has duplicate option label \`${option.label}\``);
      }
      labels.add(option.label);
      if (normalized.multiSelect && option.preview !== undefined) {
        throw new Error(`question ${index} cannot use previews when multiSelect is true`);
      }
    }
    return normalized;
  });
}

function validateAnswers(
  questions: readonly AskUserQuestion[],
  answers: readonly AskUserAnswer[],
): AskUserAnswer[] {
  if (answers.length !== questions.length) {
    throw new CommandError(
      "invalid_askuser_answer",
      `expected ${questions.length} answers, received ${answers.length}`,
    );
  }
  const byIndex = new Map<number, AskUserAnswer>();
  for (const answer of answers) {
    if (
      !Number.isSafeInteger(answer.question_index) ||
      answer.question_index < 0 ||
      answer.question_index >= questions.length ||
      byIndex.has(answer.question_index) ||
      !Array.isArray(answer.selected_options) ||
      (answer.custom_text !== null && typeof answer.custom_text !== "string")
    ) {
      throw new CommandError("invalid_askuser_answer", "askuser answer shape is invalid");
    }
    const question = questions[answer.question_index];
    if (question === undefined) {
      throw new CommandError("invalid_askuser_answer", "askuser question index is invalid");
    }
    const labels = new Set(question.options.map((option) => option.label));
    if (answer.selected_options.some((option) => !labels.has(option))) {
      throw new CommandError("invalid_askuser_answer", "askuser answer selected an unknown option");
    }
    if (new Set(answer.selected_options).size !== answer.selected_options.length) {
      throw new CommandError("invalid_askuser_answer", "askuser answer repeated an option");
    }
    if (answer.custom_text !== null && !answer.custom_text.trim()) {
      throw new CommandError("invalid_askuser_answer", "askuser custom answer must not be empty");
    }
    const answerCount = answer.selected_options.length + (answer.custom_text === null ? 0 : 1);
    if (answerCount === 0) {
      throw new CommandError("invalid_askuser_answer", "askuser answer must not be empty");
    }
    if (!question.multiSelect && answerCount !== 1) {
      throw new CommandError(
        "invalid_askuser_answer",
        "a single-select askuser question requires exactly one answer",
      );
    }
    byIndex.set(answer.question_index, {
      question_index: answer.question_index,
      selected_options: [...answer.selected_options],
      custom_text: answer.custom_text,
    });
  }
  return [...byIndex.values()].sort((left, right) => left.question_index - right.question_index);
}

function formatAnswers(
  questions: readonly AskUserQuestion[],
  answers: readonly AskUserAnswer[],
): string {
  const rendered = answers.map((answer) => ({
    question: questions[answer.question_index]?.question ?? "",
    selected_options: answer.selected_options,
    custom_text: answer.custom_text,
  }));
  return JSON.stringify({ answers: rendered });
}
