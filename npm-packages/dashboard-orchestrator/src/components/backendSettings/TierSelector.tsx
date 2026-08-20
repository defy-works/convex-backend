import { cn } from "@ui/cn";
import { useEffect, useState } from "react";
import type { HostCapacity } from "../../lib/orchestratorApi";
import {
  clampCustomTierResources,
  encodeCustomTier,
  formatTierResources,
  parseCustomTier,
  tierResourcesForName,
  TIERS,
  type TierResources,
} from "./tiers";

export function TierSelector({
  value,
  onChange,
  capacity,
}: {
  value: string;
  onChange: (tier: string) => void;
  capacity: HostCapacity | undefined;
}) {
  const valueResources =
    tierResourcesForName(value) ?? tierResourcesForName("S16")!;
  const [customResources, setCustomResources] =
    useState<TierResources>(valueResources);
  const customSelected = parseCustomTier(value) !== undefined;

  useEffect(() => {
    const parsed = parseCustomTier(value);
    if (parsed) {
      setCustomResources(parsed);
    }
  }, [value]);

  const clamp = (resources: TierResources) =>
    capacity ? clampCustomTierResources(resources, capacity) : resources;
  const selectCustom = () => {
    const next = clamp(customSelected ? customResources : valueResources);
    setCustomResources(next);
    onChange(encodeCustomTier(next));
  };
  const updateCustom = (next: TierResources) => {
    const clamped = clamp(next);
    setCustomResources(clamped);
    onChange(encodeCustomTier(clamped));
  };
  // The step ladder on a number input is anchored at `min`, so reachable
  // values are `min + n*step`. With min=0.1/step=0.25 an 8-CPU host topped out
  // at 7.85 (0.1 + 31*0.25) because 8.10 overshoots `max`. Anchoring both
  // inputs at a multiple of their own step makes the host maximum reachable.
  const maxMemoryGb = capacity
    ? // Round down to the step so the ceiling is actually selectable.
      Math.floor((capacity.totalMemoryMb / 1024) * 2) / 2
    : undefined;
  const maxCpus = capacity ? Math.floor(capacity.totalCpus) : undefined;
  return (
    <div className="flex flex-col gap-2">
      <div className="grid grid-cols-2 gap-2 md:grid-cols-4">
        {TIERS.map((tier) => {
          const selected = value === tier.name;
          // A tier bigger than the machine is not a real option: the
          // orchestrator rejects it, and docker would be handed a --cpus
          // larger than the host.
          const tooBig =
            capacity !== undefined &&
            (tier.memoryMb > capacity.totalMemoryMb ||
              tier.cpus > capacity.totalCpus);
          return (
            // eslint-disable-next-line react/forbid-elements -- card-style radio, intentional plain button
            <button
              key={tier.name}
              type="button"
              onClick={() => onChange(tier.name)}
              aria-pressed={selected}
              disabled={tooBig}
              title={
                tooBig
                  ? `This host has ${Math.round(
                      capacity!.totalMemoryMb / 1024,
                    )} GB and ${capacity!.totalCpus} CPUs — too small for ${
                      tier.name
                    }.`
                  : undefined
              }
              className={cn(
                "flex flex-col items-start rounded-md border px-3 py-2 text-left transition-all",
                selected
                  ? "border-content-link bg-background-secondary"
                  : "border-border-transparent bg-background-tertiary/40 hover:bg-background-tertiary",
                tooBig && "cursor-not-allowed opacity-40 hover:bg-transparent",
              )}
            >
              <span className="font-mono text-sm font-semibold">
                {tier.name}
              </span>
              <span className="text-xs text-content-secondary">
                {formatTierResources(tier)}
                {tooBig && " · exceeds host"}
              </span>
            </button>
          );
        })}
        {/* eslint-disable-next-line react/forbid-elements -- card-style radio, intentional plain button */}
        <button
          type="button"
          onClick={selectCustom}
          aria-pressed={customSelected}
          className={cn(
            "flex flex-col items-start rounded-md border px-3 py-2 text-left transition-all",
            customSelected
              ? "border-content-link bg-background-secondary"
              : "border-border-transparent bg-background-tertiary/40 hover:bg-background-tertiary",
          )}
        >
          <span className="font-mono text-sm font-semibold">Custom</span>
          <span className="text-xs text-content-secondary">
            {formatTierResources(customResources)}
          </span>
        </button>
      </div>
      {customSelected && (
        <div className="grid gap-3 rounded-md border border-border-transparent bg-background-tertiary/30 p-3 sm:grid-cols-2">
          <label className="flex flex-col gap-1 text-xs text-content-secondary">
            RAM (GB)
            {}
            <input
              type="number"
              // min is a multiple of step, so every half-GB up to and
              // including maxMemoryGb is reachable from the spinner.
              min={0.5}
              max={maxMemoryGb}
              step={0.5}
              value={Number((customResources.memoryMb / 1024).toFixed(2))}
              onChange={(e) =>
                updateCustom({
                  ...customResources,
                  memoryMb: Number(e.target.value) * 1024,
                })
              }
              className="rounded-sm border border-border-transparent bg-background-primary px-2 py-1 text-sm text-content-primary"
            />
          </label>
          <label className="flex flex-col gap-1 text-xs text-content-secondary">
            CPUs
            {}
            <input
              type="number"
              // Whole cores. Docker accepts fractional --cpus and the preset
              // ladder still uses it for the sub-core S4, but there is no
              // reason to hand-allocate a fraction of a core, and the old
              // min=0.1/step=0.25 ladder made the host maximum unreachable.
              min={1}
              max={maxCpus}
              step={1}
              value={customResources.cpus}
              onChange={(e) =>
                updateCustom({
                  ...customResources,
                  cpus: Number(e.target.value),
                })
              }
              className="rounded-sm border border-border-transparent bg-background-primary px-2 py-1 text-sm text-content-primary"
            />
          </label>
        </div>
      )}
    </div>
  );
}
