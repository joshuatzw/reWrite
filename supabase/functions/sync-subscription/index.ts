import { serve } from "https://deno.land/std@0.208.0/http/server.ts";
import { createClient } from "npm:@supabase/supabase-js@2";
import Stripe from "npm:stripe@17";
import { corsHeaders, handleCors, json } from "../_shared/cors.ts";
import { resolvePlan } from "../_shared/plan.ts";

serve(async (req) => {
  const corsRes = handleCors(req);
  if (corsRes) return corsRes;

  const authHeader = req.headers.get("Authorization");
  if (!authHeader?.startsWith("Bearer ")) return json({ error: "Unauthorized" }, 401);

  // Validate JWT via anon key client
  const supabase = createClient(
    Deno.env.get("SUPABASE_URL")!,
    Deno.env.get("SUPABASE_ANON_KEY")!,
    { global: { headers: { Authorization: authHeader } } },
  );

  const { data: { user }, error: authErr } = await supabase.auth.getUser();
  if (authErr || !user) return json({ error: "Unauthorized" }, 401);

  // Service role for all DB reads/writes (bypasses RLS)
  const admin = createClient(
    Deno.env.get("SUPABASE_URL")!,
    Deno.env.get("SUPABASE_SERVICE_ROLE_KEY")!,
  );

  const { data: profile } = await admin
    .from("profiles")
    .select(
      "stripe_customer_id, rewrite_count, rewrite_month, is_subscribed, subscription_valid_until, plan",
    )
    .eq("id", user.id)
    .single();

  if (!profile?.stripe_customer_id) {
    return json({
      is_subscribed: false,
      subscription_valid_until: null,
      trial_end: null,
      rewrite_count: profile?.rewrite_count ?? 0,
    });
  }

  // Live Stripe is the source of truth when reachable. If Stripe is down or
  // errors, fall back to the last value the `stripe-webhook` function (or a
  // prior successful run of this function) wrote to `profiles` instead of
  // 500ing — a paying user should never be bounced to Free just because a
  // status check failed, since the DB already reflects their real state.
  try {
    const stripe = new Stripe(Deno.env.get("STRIPE_SECRET_KEY")!);

    const subscriptions = await stripe.subscriptions.list({
      customer: profile.stripe_customer_id,
      status: "all",
      limit: 5,
    });

    const active = subscriptions.data.find(
      (s) => s.status === "active" || s.status === "trialing",
    );

    const is_subscribed = !!active;
    const subscription_valid_until = active
      ? new Date(active.current_period_end * 1000).toISOString()
      : null;
    const trial_end = active?.trial_end
      ? new Date((active.trial_end as number) * 1000).toISOString()
      : null;
    const plan = active ? resolvePlan(active.items.data[0]?.price?.id) : null;

    await admin.from("profiles").update({
      is_subscribed,
      subscription_valid_until,
      plan,
      last_synced_at: new Date().toISOString(),
    }).eq("id", user.id);

    return json({
      is_subscribed,
      subscription_valid_until,
      trial_end,
      rewrite_count: profile.rewrite_count ?? 0,
    });
  } catch (err) {
    console.error("sync-subscription: Stripe call failed, falling back to stored profile", err);

    return json({
      is_subscribed: profile.is_subscribed ?? false,
      subscription_valid_until: profile.subscription_valid_until ?? null,
      trial_end: null,
      rewrite_count: profile.rewrite_count ?? 0,
    });
  }
});
