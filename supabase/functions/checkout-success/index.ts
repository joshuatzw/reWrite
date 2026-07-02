import { serve } from "https://deno.land/std@0.208.0/http/server.ts";

// Supabase's edge gateway refuses to render HTML served from *.supabase.co
// (it forces Content-Type: text/plain + a `sandbox` CSP), so an HTML page with
// a JS redirect just shows raw source and never runs. Redirect at the HTTP
// layer instead — the browser hands the custom scheme straight to the OS.
serve(() =>
  new Response(null, {
    status: 302,
    headers: { Location: "rewrite://checkout-success" },
  })
);
