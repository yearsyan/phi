import { describe, expect, it } from "vitest";

import { projectPiContent } from "../src/projection.js";

describe("Pi wire projection", () => {
  it("does not expose expanded skill bodies or local skill paths in public user history", () => {
    const content = projectPiContent(
      [
        {
          type: "text",
          text: '<skill name="review" location="/private/skills/review/SKILL.md">\nReferences are relative.\n\nsecret instructions\n</skill>\n\ncheck this diff',
        },
      ],
      true,
    );

    expect(content).toEqual({ type: "text", value: "check this diff" });
    expect(JSON.stringify(content)).not.toContain("secret instructions");
    expect(JSON.stringify(content)).not.toContain("/private/skills");
  });
});
