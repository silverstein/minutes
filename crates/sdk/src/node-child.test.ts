import { describe, expect, it } from "vitest";

import { nodeChildEnvironment } from "./node-child.js";

describe("Node helper environment", () => {
  it("forces Electron hosts into Node mode", () => {
    const environment = nodeChildEnvironment(
      { PATH: "synthetic", ELECTRON_RUN_AS_NODE: "0" },
      "42.9.2"
    );
    expect(environment).toEqual({
      PATH: "synthetic",
      ELECTRON_RUN_AS_NODE: "1",
    });
  });

  it("does not invent Electron mode for an ordinary Node host", () => {
    expect(nodeChildEnvironment({ PATH: "synthetic" }, undefined)).toEqual({
      PATH: "synthetic",
    });
  });

  it("does not mutate the caller's environment object", () => {
    const base = { ELECTRON_RUN_AS_NODE: "0" };
    nodeChildEnvironment(base, "42.9.2");
    expect(base.ELECTRON_RUN_AS_NODE).toBe("0");
  });
});
