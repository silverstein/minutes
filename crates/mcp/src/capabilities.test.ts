import { describe, expect, it } from "vitest";

import {
  hasFeature,
  parseCapabilityReport,
  type CapabilityProbeResult,
  type CapabilityReport,
} from "./capabilities.js";

describe("parseCapabilityReport", () => {
  it("parses a valid capability report", () => {
    const raw = JSON.stringify({
      version: "0.14.0",
      api_version: 1,
      features: {
        activity_summary: true,
        search_context: true,
        screen_context: true,
        parakeet: false,
      },
    });
    const report = parseCapabilityReport(raw);
    expect(report).not.toBeNull();
    expect(report?.version).toBe("0.14.0");
    expect(report?.api_version).toBe(1);
    expect(report?.features.activity_summary).toBe(true);
    expect(report?.features.screen_context).toBe(true);
    expect(report?.features.parakeet).toBe(false);
  });

  it("tolerates trailing whitespace and newlines", () => {
    const raw = `  ${JSON.stringify({
      version: "0.14.0",
      api_version: 1,
      features: { activity_summary: true },
    })}\n\n`;
    const report = parseCapabilityReport(raw);
    expect(report?.features.activity_summary).toBe(true);
  });

  it("returns null on invalid JSON", () => {
    expect(parseCapabilityReport("not json")).toBeNull();
    expect(parseCapabilityReport("")).toBeNull();
  });

  it("returns null when required fields are missing", () => {
    expect(parseCapabilityReport(JSON.stringify({ version: "0.14.0" }))).toBeNull();
    expect(
      parseCapabilityReport(JSON.stringify({ api_version: 1, features: {} }))
    ).toBeNull();
    expect(
      parseCapabilityReport(JSON.stringify({ version: "0.14.0", api_version: 1 }))
    ).toBeNull();
  });

  it("returns null when version is not a string", () => {
    const raw = JSON.stringify({
      version: 14,
      api_version: 1,
      features: {},
    });
    expect(parseCapabilityReport(raw)).toBeNull();
  });

  it("returns null when api_version is not a number", () => {
    const raw = JSON.stringify({
      version: "0.14.0",
      api_version: "one",
      features: {},
    });
    expect(parseCapabilityReport(raw)).toBeNull();
  });

  it("drops non-boolean feature values instead of enabling tools", () => {
    const raw = JSON.stringify({
      version: "0.14.0",
      api_version: 1,
      features: {
        activity_summary: true,
        suspicious: "yes",
        also_bad: 1,
        legitimately_off: false,
      },
    });
    const report = parseCapabilityReport(raw);
    expect(report?.features.activity_summary).toBe(true);
    expect(report?.features.legitimately_off).toBe(false);
    expect(report?.features.suspicious).toBeUndefined();
    expect(report?.features.also_bad).toBeUndefined();
  });

  it("rejects a report with api_version newer than supported", () => {
    const raw = JSON.stringify({
      version: "9.9.9",
      api_version: 2,
      features: { activity_summary: true },
    });
    expect(parseCapabilityReport(raw)).toBeNull();
  });

  it("rejects a report with api_version less than 1", () => {
    const raw = JSON.stringify({
      version: "0.14.0",
      api_version: 0,
      features: { activity_summary: true },
    });
    expect(parseCapabilityReport(raw)).toBeNull();
  });

  it("rejects a report with non-integer api_version", () => {
    const raw = JSON.stringify({
      version: "0.14.0",
      api_version: 1.5,
      features: { activity_summary: true },
    });
    expect(parseCapabilityReport(raw)).toBeNull();
  });

  it("drops prototype-pollution keys in the features map", () => {
    // Object.fromEntries + JSON.parse preserves the string key `__proto__`
    // as a literal own property rather than touching the prototype, so we
    // have to hand-craft the payload.
    const raw =
      '{"version":"0.14.0","api_version":1,"features":{"__proto__":true,"constructor":true,"prototype":true,"activity_summary":true}}';
    const report = parseCapabilityReport(raw);
    expect(report).not.toBeNull();
    expect(report?.features.activity_summary).toBe(true);
    // Polluted keys must not appear in the features map.
    expect(Object.keys(report!.features)).toEqual(["activity_summary"]);
  });
});

describe("hasFeature", () => {
  it("returns true when the CLI is missing at boot (first-run auto-install path)", () => {
    const probe: CapabilityProbeResult = { kind: "missing-cli" };
    expect(hasFeature(probe, "activity_summary")).toBe(true);
    expect(hasFeature(probe, "anything")).toBe(true);
    expect(hasFeature(probe, "")).toBe(true);
  });

  it("returns false when an installed CLI cannot answer capabilities", () => {
    const probe: CapabilityProbeResult = { kind: "unsupported-cli" };
    expect(hasFeature(probe, "activity_summary")).toBe(false);
    expect(hasFeature(probe, "anything")).toBe(false);
  });

  it("returns true when feature is explicitly true", () => {
    const report: CapabilityReport = {
      version: "0.14.0",
      api_version: 1,
      features: { activity_summary: true },
    };
    const probe: CapabilityProbeResult = { kind: "report", report };
    expect(hasFeature(probe, "activity_summary")).toBe(true);
  });

  it("returns false when feature is explicitly false", () => {
    const report: CapabilityReport = {
      version: "0.14.0",
      api_version: 1,
      features: { parakeet: false },
    };
    const probe: CapabilityProbeResult = { kind: "report", report };
    expect(hasFeature(probe, "parakeet")).toBe(false);
  });

  it("returns false when feature key is missing from a non-null report", () => {
    const report: CapabilityReport = {
      version: "0.13.3",
      api_version: 1,
      features: { start_recording: true },
    };
    // An older CLI that does not know about activity_summary: the MCP
    // must hide the tool rather than optimistically expose it.
    const probe: CapabilityProbeResult = { kind: "report", report };
    expect(hasFeature(probe, "activity_summary")).toBe(false);
  });
});
