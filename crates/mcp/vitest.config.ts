import { defineConfig } from "vitest/config";

export default defineConfig({
  test: {
    root: ".",
    include: ["src/**/*.test.ts", "ui/src/**/*.test.ts"],
    // Above every operation deadline the suites exercise, deliberately.
    //
    // DEFAULT_BOUND_READ_TIMEOUT_MS and DEFAULT_AUTHORIZATION_TIMEOUT_MS are
    // both 15_000, so at 15_000 here the harness and the operation it measures
    // shared a deadline and vitest won it, because its clock starts first. A
    // read or lease that ran to its own limit was killed roughly 10ms short of
    // reporting why, and the suite printed "Test timed out in 15000ms" with no
    // cause. 30 of the 49 corpus-lease cases inherited that collision, which is
    // why the case that failed moved around and never explained itself (#617).
    //
    // Demonstrated rather than reasoned about: an operation stalled to its own
    // timeout fails here at 15_000 and passes at 30_000, unchanged otherwise.
    // A harness must outlive what it measures, or it reports itself.
    // The explicit repository test harness may grant corpus authorization up
    // to 60 seconds on a loaded runner. Keep Vitest outside that deadline so
    // the lease, not the harness, still reports any real timeout.
    testTimeout: 75_000,
  },
});
