import { serve } from "https://deno.land/std@0.208.0/http/server.ts";
import { createClient } from "npm:@supabase/supabase-js@2";
import Stripe from "npm:stripe@17";
import { handleCors, json } from "../_shared/cors.ts";

serve(async (req) => {
  const corsRes = handleCors(req);
  if (corsRes) return corsRes;

  const authHeader = req.headers.get("Authorization");
  if (!authHeader?.startsWith("Bearer ")) return json({ error: "Unauthorized" }, 401);

  const supabase = createClient(
    Deno.env.get("SUPABASE_URL")!,
    Deno.env.get("SUPABASE_ANON_KEY")!,
    { global: { headers: { Authorization: authHeader } } },
  );

  const { data: { user }, error: authErr } = await supabase.auth.getUser();
  if (authErr || !user) return json({ error: "Unauthorized" }, 401);

  const admin = createClient(
    Deno.env.get("SUPABASE_URL")!,
    Deno.env.get("SUPABASE_SERVICE_ROLE_KEY")!,
  );

  const stripe = new Stripe(Deno.env.get("STRIPE_SECRET_KEY")!);

  const { data: profile } = await admin
    .from("profiles")
    .select("stripe_customer_id")
    .eq("id", user.id)
    .single();

  let customerId = profile?.stripe_customer_id as string | undefined;

  if (!customerId) {
    // Avoid duplicate customers: check Stripe by email first
    const existing = await stripe.customers.list({ email: user.email, limit: 1 });
    if (existing.data.length > 0) {
      customerId = existing.data[0].id;
    } else {
      const customer = await stripe.customers.create({
        email: user.email!,
        metadata: { supabase_user_id: user.id },
      });
      customerId = customer.id;
    }
    // Persist before payment so webhook upsert is idempotent
    await admin.from("profiles").update({ stripe_customer_id: customerId }).eq("id", user.id);
  }

  let plan = "pro";
  try {
    const body = await req.json();
    if (body.plan === "max") plan = "max";
  } catch { /* plan defaults to pro */ }

  const priceId = plan === "max"
    ? Deno.env.get("STRIPE_MAX_PRICE_ID")
    : Deno.env.get("STRIPE_PRO_PRICE_ID");
  if (!priceId) return json({ error: "Price not configured" }, 500);

  const session = await stripe.checkout.sessions.create({
    customer: customerId,
    mode: "subscription",
    line_items: [{ price: priceId, quantity: 1 }],
    // Deep-link back to the desktop app
    success_url: Deno.env.get("CHECKOUT_SUCCESS_URL") ?? "https://example.com",
    cancel_url: Deno.env.get("CHECKOUT_CANCEL_URL") ?? "https://example.com",
    allow_promotion_codes: true,
  });

  return json({ url: session.url });
});
