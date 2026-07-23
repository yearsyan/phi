import { describe, expect, it, vi } from "vitest";

import { AskUserBroker } from "../src/ask-user.js";

describe("AskUserBroker", () => {
  it("enforces single-select answers and returns the Phi tool result envelope", async () => {
    const broker = new AskUserBroker();
    broker.bind({ requested: vi.fn(), answered: vi.fn(), cancelled: vi.fn() });
    const tool = broker.createTool();
    const execution = tool.execute(
      "call-1",
      {
        questions: [
          {
            question: "Choose one",
            header: "Choice",
            options: [
              { label: "A", description: "First" },
              { label: "B", description: "Second" },
            ],
          },
        ],
      },
      undefined,
      undefined,
      {} as never,
    );
    await vi.waitFor(() => expect(broker.listPending()).toHaveLength(1));
    const ask = broker.listPending()[0];
    if (ask === undefined) throw new Error("askuser request was not created");

    expect(() =>
      broker.answer(ask.ask_id, [
        { question_index: 0, selected_options: ["A"], custom_text: "also custom" },
      ]),
    ).toThrow("exactly one answer");
    broker.answer(ask.ask_id, [
      { question_index: 0, selected_options: ["A"], custom_text: null },
    ]);

    await expect(execution).resolves.toMatchObject({
      content: [
        {
          type: "text",
          text: JSON.stringify({
            answers: [
              { question: "Choose one", selected_options: ["A"], custom_text: null },
            ],
          }),
        },
      ],
    });
  });
});
