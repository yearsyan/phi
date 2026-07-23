import { describe, expect, it } from "vitest";

import { parseClientCommand } from "../src/protocol.js";

describe("client command parser", () => {
  it("accepts the Phi tool permission decision shape", () => {
    expect(
      parseClientCommand({
        type: "decide_tool_permission",
        request_id: "request-1",
        permission_id: "permission-1",
        decision: {
          type: "allow_for_session",
          rule: { tool_name: "bash", pattern: "git status" },
        },
      }),
    ).toMatchObject({ type: "decide_tool_permission", permission_id: "permission-1" });
  });

  it("rejects malformed commands without trusting their request id", () => {
    expect(() =>
      parseClientCommand({ type: "prompt", request_id: "request-1", content: [] }),
    ).toThrow("content");
    expect(() =>
      parseClientCommand({
        type: "decide_tool_permission",
        request_id: "request-1",
        permission_id: "permission-1",
        decision: { type: "allow_once", extra: true },
      }),
    ).toThrow("decision");
  });

  it("applies the daemon defaults to omitted askuser answer fields", () => {
    expect(
      parseClientCommand({
        type: "answer_askuser",
        request_id: "request-1",
        ask_id: "ask-1",
        answers: [{ question_index: 0 }],
      }),
    ).toMatchObject({
      answers: [{ question_index: 0, selected_options: [], custom_text: null }],
    });
  });

  it("matches serde compatibility for omitted option fields and unknown command fields", () => {
    expect(
      parseClientCommand({
        type: "set_reasoning_effort",
        request_id: "request-1",
        future_field: true,
      }),
    ).toMatchObject({ effort: null });
    expect(
      parseClientCommand({
        type: "prompt",
        request_id: "request-2",
        content: { type: "text", value: "hello", future_field: true },
        future_field: true,
      }),
    ).toMatchObject({ type: "prompt" });
  });
});
