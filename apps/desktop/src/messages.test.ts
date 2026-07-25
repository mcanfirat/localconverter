import { readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

import { MESSAGES, operationLabelKey, t, tOr } from "./messages";

const RUST_ROOTS = [
  join(import.meta.dirname, "../../../crates"),
  join(import.meta.dirname, "../src-tauri/src"),
];

/** Message keys look like `error.foo`, `warning.foo.bar`, `progress.foo`. */
const KEY_PATTERN = /"((?:error|warning|progress|operation)\.[A-Za-z0-9_.]+)"/g;

function rustSources(dir: string): string[] {
  return readdirSync(dir).flatMap((entry) => {
    const path = join(dir, entry);
    if (entry === "target" || entry === "bindings") return [];
    if (statSync(path).isDirectory()) return rustSources(path);
    return path.endsWith(".rs") ? [path] : [];
  });
}

describe("message catalogue", () => {
  it("covers every key the Rust sources can emit", () => {
    const found = new Set<string>();
    for (const root of RUST_ROOTS) {
      for (const file of rustSources(root)) {
        const source = readFileSync(file, "utf8");
        for (const match of source.matchAll(KEY_PATTERN)) {
          if (match[1]) found.add(match[1]);
        }
      }
    }

    expect(found.size).toBeGreaterThan(20);
    const missing = [...found].filter((key) => !(key in MESSAGES)).sort();
    expect(missing, "keys used in Rust but absent from messages.ts").toEqual([]);
  });

  it("has no message that is merely its own key", () => {
    const lazy = Object.entries(MESSAGES).filter(([key, value]) => key === value);
    expect(lazy).toEqual([]);
  });

  it("falls back to the key when one is unknown", () => {
    expect(t("error.unsupportedFormat")).toBe(MESSAGES["error.unsupportedFormat"]);
    expect(t("error.doesNotExist")).toBe("error.doesNotExist");
    expect(tOr("error.doesNotExist", "fallback")).toBe("fallback");
  });

  it("derives operation label keys from operation ids", () => {
    expect(operationLabelKey("diagnostics.selftest")).toBe("operation.selftest.label");
    expect(t(operationLabelKey("diagnostics.selftest"))).toBe("Pipeline self-test");
  });
});
