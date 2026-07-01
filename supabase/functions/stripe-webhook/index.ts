import { serve } from "https://deno.land/std@0.208.0/http/server.ts";
import { createClient } from "npm:@supabase/supabase-js@2";
import Stripe from "npm:stripe@17";

serve(async (req) => {
  if (req.method !== "POST") {
    return new Response("Method not allowed", { status: 405 });
  }

  const webhookSecret = Deno.env.get("STRIPE_WEBHOOK_SECRET");
  if (!webhookSecret) return new Response("Webhook secret not configured", { status: 500 });

  const signature = req.headers.get("stripe-signature");
  if (!signature) return new Response("Missing stripe-signature header", { status: 400 });

  const stripe = new Stripe(Deno.env.get("STRIPE_SECRET_KEY")!);
  const body = await req.text();

  let event: Stripe.Event;
  try {
    event = await stripe.webhooks.constructEventAsync(
      body,
      signature,
      webhookSecret,
      undefined,
      Stripe.createSubtleCryptoProvider(),
    );
  } catch (err) {
    return new Response(`Signature verification failed: ${(err as Error).message}`, { status: 400 });
  }

  const admin = createClient(
    Deno.env.get("SUPABASE_URL")!,
    Deno.env.get("SUPABASE_SERVICE_ROLE_KEY")!,
  );

  switch (event.type) {
    case "checkout.session.completed": {
      const session = event.data.object as Stripe.Checkout.Session;
      if (session.mode !== "subscription" || !session.customer || !session.customer_details?.email) {
        break;
      }
      // Idempotent: upsert stripe_customer_id link (may already be set by create-checkout-session)
      const { data } = await admin.auth.admin.getUserByEmail(session.customer_details.email);
      if (data.user) {
        await admin.from("profiles").update({
          stripe_customer_id: session.customer as string,
        }).eq("id", data.user.id);
      }
      break;
    }

    case "customer.subscription.created":
    case "customer.subscription.updated":
    case "customer.subscription.deleted": {
      const sub = event.data.object as Stripe.Subscription;
      const customerId = sub.customer as string;
      const isActive = sub.status === "active" || sub.status === "trialing";

      await admin.from("profiles").update({
        is_subscribed: isActive,
        subscription_valid_until: isActive && sub.current_period_end
          ? new Date(sub.current_period_end * 1000).toISOString()
          : null,
        last_synced_at: new Date().toISOString(),
      }).eq("stripe_customer_id", customerId);
      break;
    }

    default:
      break;
  }

  return new Response(JSON.stringify({ received: true }), {
    headers: { "Content-Type": "application/json" },
  });
});
