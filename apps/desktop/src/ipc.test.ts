import { describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));

const { toConversionError } = await import("./ipc");

describe("toConversionError", () => {
  it("passes a real backend error through untouched", () => {
    const backend = {
      code: "outputValidationFailed",
      messageKey: "error.outputValidationFailed",
      detail: "failed checks: output.nonEmpty",
      sourceSafe: true,
      partialOutputRemoved: true,
    };
    expect(toConversionError(backend)).toBe(backend);
  });

  it("wraps anything else rather than swallowing it", () => {
    const wrapped = toConversionError(new TypeError("bridge broke"));
    expect(wrapped.code).toBe("internalError");
    expect(wrapped.messageKey).toBe("error.internalError");
    expect(wrapped.detail).toContain("bridge broke");
    // An unexpected throw happened before anything touched the user's files.
    expect(wrapped.sourceSafe).toBe(true);
  });

  it("does not mistake an arbitrary object for a backend error", () => {
    expect(toConversionError({ code: 500 }).code).toBe("internalError");
    expect(toConversionError(null).code).toBe("internalError");
  });
});
