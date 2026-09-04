import { defineConfig } from "vitest/config";

export default defineConfig({
  test: {
    root: ".",
    include: ["src/**/*.test.ts"],
    // Above every operation deadline the suites exercise, deliberately.
    //
    // DEFAULT_BOUND_READ_TIMEOUT_MS was 15_000, and the authorization deadline
    // used to be the same, so at 15_000 here the harness and the operation it
    // measured shared a deadline and vitest won it because its clock starts
    // first. A read or lease that ran to its own limit was killed roughly 10ms
    // short of reporting why, and the suite printed "Test timed out in 15000ms"
    // with no cause. 30 of the 49 corpus-lease cases inherited that collision,
    // which is why the case that failed moved around (#617).
    //
    // Demonstrated rather than reasoned about: an operation stalled to its own
    // timeout fails here at 15_000 and passes at 30_000, unchanged otherwise.
    // A harness must outlive what it measures, or it reports itself.
    // Production corpus authorization is now derived as 60 seconds from the
    // bounded work envelope (#933). The explicit repository test harness grants
    // the same budget on loaded runners. Keep Vitest outside that deadline so
    // the lease, not the harness, still reports any real timeout.
    testTimeout: 75_000,
  },
});
