import { describe, expect, it } from "vitest";

import { fileNameOf, formatBytes, formatDuration, formatSizeChange } from "./format";

describe("formatBytes", () => {
  it("uses explicit binary units", () => {
    expect(formatBytes(0)).toBe("0 B");
    expect(formatBytes(999)).toBe("999 B");
    expect(formatBytes(1024)).toBe("1.00 KiB");
    expect(formatBytes(1536)).toBe("1.50 KiB");
    expect(formatBytes(1024 * 1024)).toBe("1.00 MiB");
    expect(formatBytes(1024 ** 3)).toBe("1.00 GiB");
  });

  it("drops decimals as the number grows", () => {
    expect(formatBytes(15 * 1024)).toBe("15.0 KiB");
    expect(formatBytes(150 * 1024)).toBe("150 KiB");
  });

  it("refuses to invent a size", () => {
    expect(formatBytes(-1)).toBe("—");
    expect(formatBytes(Number.NaN)).toBe("—");
  });
});

describe("formatSizeChange", () => {
  it("signs growth and shrinkage differently", () => {
    expect(formatSizeChange(-42.34)).toBe("−42.3%");
    expect(formatSizeChange(12.5)).toBe("+12.5%");
  });

  it("says so when nothing changed", () => {
    expect(formatSizeChange(0)).toBe("no change");
    expect(formatSizeChange(0.01)).toBe("no change");
  });
});

describe("formatDuration", () => {
  it("scales the unit to the magnitude", () => {
    expect(formatDuration(340)).toBe("340 ms");
    expect(formatDuration(2500)).toBe("2.5 s");
    expect(formatDuration(95_000)).toBe("1 min 35 s");
  });
});

describe("fileNameOf", () => {
  it("never returns a directory component", () => {
    expect(fileNameOf("/Users/someone/private/holiday.heic")).toBe("holiday.heic");
    expect(fileNameOf("C:\\Users\\someone\\ünïcode tëst.jpg")).toBe("ünïcode tëst.jpg");
    expect(fileNameOf("bare.png")).toBe("bare.png");
  });
});
