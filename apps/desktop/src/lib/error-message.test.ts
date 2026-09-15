import { describe, expect, it } from "vitest";
import { errorMessage } from "./error-message";

describe("errorMessage", () => {
  it("preserves Tauri string rejections", () => {
    expect(errorMessage("Could not reverse event evt-42; recovery required")).toBe(
      "Could not reverse event evt-42; recovery required",
    );
  });

  it("preserves empty Tauri string rejections", () => {
    expect(errorMessage("")).toBe("");
  });

  it("preserves Error messages", () => {
    expect(errorMessage(new Error("Restore failed after a network interruption"))).toBe(
      "Restore failed after a network interruption",
    );
  });

  it("uses the fallback for an error without a displayable message", () => {
    expect(errorMessage({ reason: "unavailable" }, "Restore failed")).toBe("Restore failed");
  });
});
