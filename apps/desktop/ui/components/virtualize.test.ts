import { describe, expect, it } from "vitest";
import { computeVisibleRange } from "./virtualize";

describe("computeVisibleRange", () => {
  it("returns the first rows plus overscan when scrolled to the top", () => {
    const range = computeVisibleRange({
      scrollTop: 0,
      containerHeight: 400,
      rowHeight: 20,
      itemCount: 50_000,
      overscan: 5,
    });
    expect(range.start).toBe(0);
    expect(range.end).toBe(20 + 5); // 400/20 = 20 visible rows + overscan
  });

  it("shifts the window as scrollTop increases", () => {
    const range = computeVisibleRange({
      scrollTop: 2000,
      containerHeight: 400,
      rowHeight: 20,
      itemCount: 50_000,
      overscan: 5,
    });
    expect(range.start).toBe(100 - 5); // 2000/20 = 100
    expect(range.end).toBe(100 + 20 + 5);
  });

  it("clamps to the item count near the end of a 10k+ line log", () => {
    const range = computeVisibleRange({
      scrollTop: 199_600,
      containerHeight: 400,
      rowHeight: 20,
      itemCount: 10_000,
      overscan: 5,
    });
    expect(range.end).toBeLessThanOrEqual(10_000);
    expect(range.start).toBeGreaterThanOrEqual(0);
  });

  it("never returns a negative start", () => {
    const range = computeVisibleRange({
      scrollTop: 0,
      containerHeight: 400,
      rowHeight: 20,
      itemCount: 3,
      overscan: 5,
    });
    expect(range.start).toBe(0);
  });
});
