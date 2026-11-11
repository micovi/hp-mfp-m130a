import { describe, expect, it } from "vitest";
import { errorMessage } from "./api.ts";

describe("errorMessage", () => {
  it("passes the backend's sentence through", () => {
    expect(errorMessage("The scanner is busy; try again in a moment.")).toBe(
      "The scanner is busy; try again in a moment.",
    );
  });

  it("uses an Error's message", () => {
    expect(errorMessage(new Error("Bad state"))).toBe("Bad state");
  });

  it("explains values that carry no message", () => {
    expect(errorMessage(undefined)).toBe(
      "Unexpected error (undefined). Restart the app and try again.",
    );
    expect(errorMessage("  ")).toBe("Unexpected error (  ). Restart the app and try again.");
    expect(errorMessage(new Error(""))).toBe(
      "Unexpected error (Error). Restart the app and try again.",
    );
  });
});
