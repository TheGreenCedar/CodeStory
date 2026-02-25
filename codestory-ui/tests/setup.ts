import "@testing-library/jest-dom/vitest";
import { cleanup } from "@testing-library/react";
import { afterEach } from "vitest";

afterEach(() => {
  cleanup();
});

if (typeof window !== "undefined" && typeof window.requestAnimationFrame !== "function") {
  window.requestAnimationFrame = (callback: FrameRequestCallback): number =>
    window.setTimeout(() => callback(performance.now()), 0);
}

if (typeof window !== "undefined" && typeof window.cancelAnimationFrame !== "function") {
  window.cancelAnimationFrame = (handle: number) => {
    window.clearTimeout(handle);
  };
}
