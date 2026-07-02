export type Plan = "pro" | "max" | null;

/** Maps a Stripe price id to our internal plan tier via env-configured price ids. */
export function resolvePlan(priceId: string | undefined | null): Plan {
  if (!priceId) return null;
  if (priceId === Deno.env.get("STRIPE_MAX_PRICE_ID")) return "max";
  if (priceId === Deno.env.get("STRIPE_PRO_PRICE_ID")) return "pro";
  return null;
}

export const MONTHLY_LIMITS: Record<"free" | "pro" | "max", number> = {
  free: 3,
  pro: 1000,
  max: 5000,
};

export function monthlyLimitFor(plan: Plan): number {
  if (plan === "max") return MONTHLY_LIMITS.max;
  if (plan === "pro") return MONTHLY_LIMITS.pro;
  return MONTHLY_LIMITS.free;
}
