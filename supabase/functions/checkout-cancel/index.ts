import { serve } from "https://deno.land/std@0.208.0/http/server.ts";

// See checkout-success: Supabase won't render HTML from this origin, so we
// redirect at the HTTP layer straight to the app's deep link.
serve(() =>
  new Response(null, {
    status: 302,
    headers: { Location: "rewrite://checkout-cancelled" },
  })
);
