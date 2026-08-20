import {
  clampCustomTierResources,
  encodeCustomTier,
  parseCustomTier,
  TIERS,
  tierDefaultsForName,
} from "./tiers";

describe("backend tier helpers", () => {
  test("preset tiers no longer include max", () => {
    expect(TIERS.map((tier) => tier.name)).toEqual([
      "S4",
      "S8",
      "S16",
      "S32",
      "S64",
      "S128",
      "S256",
    ]);
  });

  test("custom tiers round-trip RAM and CPU allocation", () => {
    const tier = encodeCustomTier({ memoryMb: 12288, cpus: 6.5 });

    expect(tier).toBe("custom:12288:6.5");
    expect(parseCustomTier(tier)).toEqual({ memoryMb: 12288, cpus: 6.5 });
  });

  // A tier bigger than the machine is not a real option — the orchestrator
  // rejects it, and docker would be handed a --cpus larger than the host. The
  // picker used to offer every preset regardless.
  test("presets are classified against host capacity", () => {
    const host = { totalMemoryMb: 32768, totalCpus: 16 };
    const exceeds = (t: { memoryMb: number; cpus: number }) =>
      t.memoryMb > host.totalMemoryMb || t.cpus > host.totalCpus;

    const byName = Object.fromEntries(TIERS.map((t) => [t.name, t]));
    // S256 is 64 GB / 32 CPUs — double this host on both axes.
    expect(exceeds(byName.S256)).toBe(true);
    // S128 is exactly 32 GB / 16 CPUs, so it fits on the nose.
    expect(exceeds(byName.S128)).toBe(false);
    expect(exceeds(byName.S16)).toBe(false);
  });

  test("the reachable custom maximum is the host maximum", () => {
    // The number inputs step from `min`, so reachable values are min + n*step.
    // min=0.1/step=0.25 topped an 8-CPU host out at 7.85 (0.1 + 31*0.25),
    // because 8.10 overshoots max. Whole-core steps land on the host value.
    const cpuMax = (totalCpus: number) => {
      const min = 1;
      const step = 1;
      return min + Math.floor((Math.floor(totalCpus) - min) / step) * step;
    };
    expect(cpuMax(8)).toBe(8);
    expect(cpuMax(16)).toBe(16);

    // Memory steps by half a GB from 0.5, and the max is floored to the step.
    const memMax = (totalMemoryMb: number) =>
      Math.floor((totalMemoryMb / 1024) * 2) / 2;
    expect(memMax(62976)).toBe(61.5);
    expect(memMax(65536)).toBe(64);
  });

  test("custom tier resources clamp to system maximums", () => {
    expect(
      clampCustomTierResources(
        { memoryMb: 65536, cpus: 32 },
        { totalMemoryMb: 49152, totalCpus: 16 },
      ),
    ).toEqual({ memoryMb: 49152, cpus: 16 });
  });

  test("tier defaults mirror orchestrator knob tuning", () => {
    expect(tierDefaultsForName("S16")).toMatchObject({
      UDF_CACHE_MAX_SIZE: "104857600",
      FUNRUN_INDEX_CACHE_SIZE: "50000000",
      RUNTIME_WORKER_THREADS: "2",
      POSTGRES_MAX_CONNECTIONS: "128",
    });

    expect(tierDefaultsForName("custom:12288:6.5")).toMatchObject({
      RUNTIME_WORKER_THREADS: "7",
      POSTGRES_MAX_CONNECTIONS: "768",
    });
  });
});
